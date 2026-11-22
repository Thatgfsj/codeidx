//! stdio 主循环：tokio 异步逐行读 stdin、单写者写 stdout、消息分发。
//!
//! stdout 纯净是验收红线：本模块是进程内唯一向 stdout 写字节的路径，
//! 且只写 JSON-RPC 响应行；一切日志由 tracing 落 stderr。

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter, Stdout};
// SIGINT/SIGTERM 流仅 unix 存在；Windows 走 tokio::signal::ctrl_c 事件降级。
#[cfg(unix)]
use tokio::signal::unix::{signal, SignalKind};

use codeidx_protocol::jsonrpc::{self, Incoming, Outgoing, Request};
use codeidx_protocol::mcp::{self, ToolsCallParams};
use codeidx_protocol::schema::TOOL_SPECS;
use codeidx_protocol::tools::tool_name;

use crate::{callflow, handlers, router};

/// stdin 逐行流类型（两平台主循环共用）。
type LineStream = tokio::io::Lines<BufReader<tokio::io::Stdin>>;

/// 服务入口：循环读行分发，直到 stdin EOF / 信号 / 写失败。
pub async fn serve() {
    // Phase 3：LSP idle reaper 与服务同生命周期（JoinHandle 可丢弃，
    // 任务随 shutdown_timeout 终结）。spawn_reaper 需要 tokio 上下文，
    // 故置于 serve() 主循环前首行；空桥不影响 F1.13 空闲内存门。
    let _reaper = Arc::clone(&crate::LSP).spawn_reaper();

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = BufWriter::new(tokio::io::stdout());

    // 平台信号源：unix 安装 SIGINT/SIGTERM 流；Windows 走 CTRL_C 事件
    // （SIGTERM 无系统等价物，支持矩阵见 docs/COMPAT.md）。
    #[cfg(unix)]
    run_loop_unix(&mut lines, &mut stdout).await;
    #[cfg(not(unix))]
    run_loop_ctrl_c(&mut lines, &mut stdout).await;

    // 优雅清理：SIGKILL 全部 LSP 子进程并同步收割（进程退出兜底，
    // 防孤儿；F1.12 SIGTERM 路径同样经过这里）。
    let killed = crate::LSP.kill_all();
    tracing::info!(killed, "shutdown: killed all LSP child processes");
    tracing::info!("shutdown complete");
}

/// 行读取结果的循环控制（两平台主循环共用）。
enum LineFlow {
    /// 继续服务。
    Continue,
    /// 停止（stdin EOF / 读失败 / stdout 写失败）。
    Stop,
}

/// 处理一条入站行读取结果。
async fn step_line(
    line: std::io::Result<Option<String>>,
    stdout: &mut BufWriter<Stdout>,
) -> LineFlow {
    match line {
        Ok(Some(raw)) => {
            if let Err(e) = handle_line(&raw, stdout).await {
                tracing::error!(error = %e, "stdout write failed; stopping");
                return LineFlow::Stop;
            }
            LineFlow::Continue
        }
        Ok(None) => {
            tracing::info!("stdin EOF; shutting down");
            LineFlow::Stop
        }
        Err(e) => {
            tracing::error!(error = %e, "stdin read failed");
            LineFlow::Stop
        }
    }
}

/// unix 主循环：stdin 逐行 + SIGINT / SIGTERM 信号流。
#[cfg(unix)]
async fn run_loop_unix(lines: &mut LineStream, stdout: &mut BufWriter<Stdout>) {
    let mut sigint = install_signal(SignalKind::interrupt(), "SIGINT");
    let mut sigterm = install_signal(SignalKind::terminate(), "SIGTERM");
    loop {
        tokio::select! {
            line = lines.next_line() => {
                if matches!(step_line(line, stdout).await, LineFlow::Stop) {
                    return;
                }
            }
            _ = sigint.recv() => {
                tracing::info!("SIGINT received; shutting down gracefully");
                return;
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received; shutting down gracefully");
                return;
            }
        }
    }
}

/// Windows 主循环：stdin 逐行 + CTRL_C 事件（tokio 跨平台 ctrl_c）。
///
/// Windows 信号模型无 SIGTERM 等价物；CTRL_C_EVENT 由 tokio::signal::ctrl_c
/// 统一处理，服务退出仍走共享的 LSP 清理路径（见 docs/COMPAT.md）。
#[cfg(not(unix))]
async fn run_loop_ctrl_c(lines: &mut LineStream, stdout: &mut BufWriter<Stdout>) {
    loop {
        tokio::select! {
            line = lines.next_line() => {
                if matches!(step_line(line, stdout).await, LineFlow::Stop) {
                    return;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("CTRL_C event; shutting down gracefully");
                return;
            }
        }
    }
}

#[cfg(unix)]
fn install_signal(kind: SignalKind, name: &str) -> tokio::signal::unix::Signal {
    signal(kind).unwrap_or_else(|e| panic!("install {name} handler: {e}"))
}

/// 处理一行入站消息：解析 → 分发 → 写出（解析失败回 ParseError）。
async fn handle_line(raw: &str, stdout: &mut BufWriter<Stdout>) -> std::io::Result<()> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    tracing::debug!(incoming = trimmed, "recv");

    let outgoing = match serde_json::from_str::<Incoming>(trimmed) {
        Ok(Incoming::Request(request)) => Some(dispatch_request(&request).await),
        Ok(Incoming::Notification(notification)) => {
            if notification.method == mcp::method::NOTIFICATIONS_INITIALIZED {
                tracing::info!("client completed initialization");
            } else {
                tracing::debug!(method = %notification.method, "ignore notification");
            }
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "malformed JSON-RPC message");
            Some(Outgoing::parse_error())
        }
    };

    if let Some(msg) = outgoing {
        write_msg(stdout, &msg).await?;
    }
    Ok(())
}

/// RPC 方法分发。LSP 与调用链请求走异步路径。
async fn dispatch_request(request: &Request) -> Outgoing {
    match request.method.as_str() {
        mcp::method::INITIALIZE => {
            tracing::info!(method = "initialize", "handshake");
            Outgoing::ok(request.id.clone(), initialize_result())
        }
        mcp::method::PING => {
            tracing::debug!("ping");
            Outgoing::ok(request.id.clone(), serde_json::json!({}))
        }
        mcp::method::TOOLS_LIST => {
            tracing::debug!("tools/list");
            Outgoing::ok(request.id.clone(), tools_list_result())
        }
        mcp::method::TOOLS_CALL => handle_tools_call(request).await,
        other => {
            tracing::warn!(method = other, "method not found");
            Outgoing::error(
                request.id.clone(),
                jsonrpc::METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )
        }
    }
}

/// initialize 响应：宣告协议版本与 tools 能力。
fn initialize_result() -> mcp::InitializeResult {
    mcp::InitializeResult {
        protocol_version: mcp::PROTOCOL_VERSION.to_owned(),
        capabilities: mcp::ServerCapabilities {
            tools: mcp::ToolsCapability {
                list_changed: Some(false),
            },
        },
        server_info: mcp::Implementation {
            name: env!("CARGO_PKG_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    }
}

/// tools/list 响应：12 个工具的描述符（schema 表为唯一事实来源）。
fn tools_list_result() -> mcp::ToolsListResult {
    mcp::ToolsListResult {
        tools: TOOL_SPECS.iter().map(|spec| spec.to_descriptor()).collect(),
    }
}

/// tools/call 分发：params 结构非法走协议级 INVALID_PARAMS，
/// 工具执行失败走 isError（契约见 ARCHITECTURE §4）。
///
/// `trace_calls` 和 `lsp_*` 可以异步等待 LSP，其余工具走同步路由。
async fn handle_tools_call(request: &Request) -> Outgoing {
    let params: ToolsCallParams =
        match serde_json::from_value(request.params.clone().unwrap_or(Value::Null)) {
            Ok(params) => params,
            Err(e) => {
                return Outgoing::error(
                    request.id.clone(),
                    jsonrpc::INVALID_PARAMS,
                    format!("invalid tools/call params: {e}"),
                );
            }
        };
    tracing::debug!(tool = %params.name, "tools/call");
    let result = if params.name == tool_name::TRACE_CALLS {
        match callflow::dispatch(&params.arguments).await {
            Ok(payload) => payload.into(),
            Err(err) => err.into(),
        }
    } else if is_lsp_tool(&params.name) {
        match handlers::lsp_dispatch(&params.name, &params.arguments).await {
            Ok(payload) => payload.into(),
            Err(err) => err.into(),
        }
    } else {
        router::dispatch(&params.name, &params.arguments)
    };
    Outgoing::ok(request.id.clone(), result)
}

/// lsp_* 语义工具名（async 分流名单，与 router 表的工具名常量一致）。
pub(crate) fn is_lsp_tool(name: &str) -> bool {
    matches!(
        name,
        tool_name::LSP_DEFINITION | tool_name::LSP_REFERENCES | tool_name::LSP_HOVER
    )
}

/// 序列化为一行紧凑 JSON 并立即 flush（行 = 消息边界）。
async fn write_msg<T>(stdout: &mut BufWriter<Stdout>, msg: &T) -> std::io::Result<()>
where
    T: Serialize + ?Sized,
{
    let mut line = serde_json::to_string(msg).expect("outgoing message serializes");
    line.push('\n');
    stdout.write_all(line.as_bytes()).await?;
    stdout.flush().await
}
