//! stdio 主循环：tokio 异步逐行读 stdin、单写者写 stdout、消息分发。
//!
//! stdout 纯净是验收红线：本模块是进程内唯一向 stdout 写字节的路径，
//! 且只写 JSON-RPC 响应行；一切日志由 tracing 落 stderr。

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter, Stdout};
use tokio::signal::unix::{signal, SignalKind};

use codeidx_protocol::jsonrpc::{self, Incoming, Outgoing, Request};
use codeidx_protocol::mcp::{self, ToolsCallParams};
use codeidx_protocol::schema::TOOL_SPECS;

use crate::router;

/// 服务入口：循环读行分发，直到 stdin EOF / SIGINT / SIGTERM / 写失败。
pub async fn serve() {
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = BufWriter::new(tokio::io::stdout());

    let mut sigint = install_signal(SignalKind::interrupt(), "SIGINT");
    let mut sigterm = install_signal(SignalKind::terminate(), "SIGTERM");

    let mut serving = true;
    while serving {
        tokio::select! {
            line = lines.next_line() => {
                match line {
                    Ok(Some(raw)) => {
                        if let Err(e) = handle_line(&raw, &mut stdout).await {
                            tracing::error!(error = %e, "stdout write failed; stopping");
                            serving = false;
                        }
                    }
                    Ok(None) => {
                        tracing::info!("stdin EOF; shutting down");
                        serving = false;
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "stdin read failed");
                        serving = false;
                    }
                }
            }
            _ = sigint.recv() => {
                tracing::info!("SIGINT received; shutting down gracefully");
                serving = false;
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received; shutting down gracefully");
                serving = false;
            }
        }
    }

    // 优雅清理：Phase 3 在此 kill 全部 LSP 子进程；当前 stub 无子进程需要回收。
    tracing::info!("shutdown complete");
}

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
        Ok(Incoming::Request(request)) => Some(dispatch_request(&request)),
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

/// RPC 方法分发（协议层）。
fn dispatch_request(request: &Request) -> Outgoing {
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
        mcp::method::TOOLS_CALL => handle_tools_call(request),
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

/// tools/list 响应：11 个工具的描述符（schema 表为唯一事实来源）。
fn tools_list_result() -> mcp::ToolsListResult {
    mcp::ToolsListResult {
        tools: TOOL_SPECS.iter().map(|spec| spec.to_descriptor()).collect(),
    }
}

/// tools/call 分发：params 结构非法走协议级 INVALID_PARAMS，
/// 工具执行失败走 isError（契约见 ARCHITECTURE §4）。
fn handle_tools_call(request: &Request) -> Outgoing {
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
    let result = router::dispatch(&params.name, &params.arguments);
    Outgoing::ok(request.id.clone(), result)
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
