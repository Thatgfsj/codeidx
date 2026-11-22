//! 单个 LSP 子进程的生命周期：spawn、initialize 握手、请求 / 通知往返、
//! didOpen / didChange 文档同步、stderr 采集与 kill。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use codeidx_ast::Lang;
use dashmap::DashMap;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{oneshot, Mutex as AsyncMutex};
use tokio::time::{sleep, timeout};

use crate::detect::{self, language_id};
use crate::proto::{self, is_meaningful, LspError, REQUEST_TIMEOUT_SECS};

/// 每个在途请求的回填通道：Ok = 响应 result，Err = 服务器 error 文案。
type Reply = Result<Value, String>;

struct PendingReply<'a> {
    pending: &'a DashMap<i64, oneshot::Sender<Reply>>,
    id: i64,
}

impl Drop for PendingReply<'_> {
    fn drop(&mut self) {
        self.pending.remove(&self.id);
    }
}

/// 冷启动"暂无结果"的重试预算与轮询间隔。
///
/// rust-analyzer 在 workspace（crate graph）加载完成前对 definition /
/// references 返回空数组、对 hover 返回 null，且状态切换瞬间可能以
/// `content modified` 取消在途请求（实机探针：didOpen 后约 10s 内就绪）。
/// 注意 didOpen 后的语法级 publishDiagnostics 会更早到达（毫秒级），
/// 不代表 workspace 就绪，不能当作 warm 信号（实测曾因此把加载中的
/// 空结果误判为真实答案）。warm 只由"取得过实质响应"置位；didOpen /
/// didChange 使分析状态失效并重新清 cold。
const WARMUP_BUDGET: Duration = Duration::from_secs(20);
const WARMUP_POLL: Duration = Duration::from_millis(500);
const CONTENT_MODIFIED_RETRY: Duration = Duration::from_millis(200);
/// SIGKILL 后同步收割的轮询参数（上限 600 × 5ms = 3s）。
const REAP_POLL_INTERVAL: Duration = Duration::from_millis(5);
const REAP_POLL_MAX: u32 = 600;

pub(crate) struct LspProcess {
    /// 展示名（`active_lsps` 的返回值来源）。
    pub(crate) binary: &'static str,
    /// 项目根（canonicalize 后；URI 构造与相对化基准）。
    root: PathBuf,
    child: Mutex<Child>,
    stdin: Arc<AsyncMutex<ChildStdin>>,
    pending: Arc<DashMap<i64, oneshot::Sender<Reply>>>,
    next_id: AtomicI64,
    last_used: Mutex<Instant>,
    /// 已同步文档：rel_path → (version, text)。
    docs: Mutex<HashMap<String, (u64, String)>>,
    /// 服务器分析状态与文档集一致后为 true：跳过空结果重试。
    warm: Arc<AtomicBool>,
}

impl LspProcess {
    /// 拉起子进程并完成 initialize → initialized 握手。
    pub(crate) async fn spawn(root: &Path, lang: Lang) -> Result<Arc<Self>, LspError> {
        let (spec, program) = detect::detect_server(lang)?;
        let mut command = Command::new(program);
        command
            .args(spec.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            // Drop 兜底：桥被丢弃时由 tokio 补发 SIGKILL，防孤儿。
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|e| LspError(format!("failed to spawn {}: {e}", spec.name)))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError("language server stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError("language server stdout unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| LspError("language server stderr unavailable".into()))?;

        let pending: Arc<DashMap<i64, oneshot::Sender<Reply>>> = Arc::new(DashMap::new());
        let stdin = Arc::new(AsyncMutex::new(stdin));
        let warm = Arc::new(AtomicBool::new(false));
        // stderr 一律走 tracing（stdout 纯净红线），stdout 泵分发响应。
        tokio::spawn(pump_stderr(stderr));
        tokio::spawn(pump_stdout(
            stdout,
            Arc::clone(&pending),
            Arc::clone(&stdin),
        ));

        let proc = Arc::new(Self {
            binary: spec.name,
            root: root.to_owned(),
            child: Mutex::new(child),
            stdin,
            pending,
            next_id: AtomicI64::new(1),
            last_used: Mutex::new(Instant::now()),
            docs: Mutex::new(HashMap::new()),
            warm,
        });

        let root_uri = proto::path_to_uri(root);
        let folder_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "root".to_owned());
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            // gopls 需要显式 workspaceFolders 才建立 view（实测缺省报
            // no views）；rust-analyzer / basedpyright 按 LSP 规范兼容。
            "workspaceFolders": [{ "uri": root_uri, "name": folder_name }],
            // capabilities 留空：桥不声明任何客户端能力，对服务器保持最小约定。
            "capabilities": {},
            "clientInfo": {
                "name": "codeidx-mcp",
                "version": env!("CARGO_PKG_VERSION"),
            },
        });
        let _server_capabilities = proc.request_raw("initialize", params).await?;
        proc.notify("initialized", json!({})).await?;
        Ok(proc)
    }

    /// 单次 JSON-RPC 往返（带 30s 超时；pending 表按 id 回填）。
    async fn request_raw(&self, method: &str, params: Value) -> Result<Value, LspError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);
        let _pending = PendingReply {
            pending: &self.pending,
            id,
        };
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let write = {
            let mut w = self.stdin.lock().await;
            proto::write_frame(&mut *w, &msg).await
        };
        if let Err(e) = write {
            return Err(LspError(format!("failed to write request {method}: {e}")));
        }
        let outcome = timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS), rx).await;
        match outcome {
            Err(_) => Err(LspError(format!(
                "{method} timed out after {REQUEST_TIMEOUT_SECS}s"
            ))),
            Ok(Err(_canceled)) => Err(LspError(format!(
                "language server closed connection during {method}"
            ))),
            Ok(Ok(Err(server_error))) => Err(LspError(format!("{method} failed: {server_error}"))),
            Ok(Ok(Ok(result))) => Ok(result),
        }
    }

    /// 冷启动容错的语义请求：空结果 / `content modified` 在预算内重试。
    ///
    /// - 空数组 / null 且进程尚冷：按 `WARMUP_POLL` 轮询至预算耗尽，
    ///   防止把"尚未就绪"误报成"无结果"；
    /// - `content modified`（服务器状态切换取消在途请求）：短暂延迟后
    ///   重发，不占用 warm 判定；
    /// - warm 进程的空结果、超时与其它错误原样返回。
    pub(crate) async fn request_until_meaningful(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, LspError> {
        let deadline = Instant::now() + WARMUP_BUDGET;
        loop {
            match self.request_raw(method, params.clone()).await {
                Ok(value) => {
                    if is_meaningful(&value) {
                        self.warm.store(true, Ordering::SeqCst);
                        return Ok(value);
                    }
                    if self.warm() || Instant::now() >= deadline {
                        return Ok(value);
                    }
                    sleep(WARMUP_POLL).await;
                }
                Err(e) => {
                    let retryable = e.0.contains("content modified") && Instant::now() < deadline;
                    if retryable {
                        sleep(CONTENT_MODIFIED_RETRY).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    fn warm(&self) -> bool {
        self.warm.load(Ordering::SeqCst)
    }

    /// 发送通知（无响应，按 wire 顺序排队）。
    async fn notify(&self, method: &str, params: Value) -> Result<(), LspError> {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let mut w = self.stdin.lock().await;
        proto::write_frame(&mut *w, &msg)
            .await
            .map_err(|e| LspError(format!("failed to write notification {method}: {e}")))
    }

    /// 同步文档内容：首次 didOpen；内容变化时 didChange 全量替换。
    ///
    /// 文档集变化会使服务器分析状态失效，故同时清 cold（空结果重新走
    /// warmup 重试，直到新诊断批次 / 实质响应到达）。
    pub(crate) async fn ensure_doc(
        &self,
        rel_path: &str,
        lang: Lang,
        text: &str,
    ) -> Result<(), LspError> {
        enum DocAction {
            Open,
            Change(u64),
            Unchanged,
        }
        let action = {
            let mut docs = self.docs.lock().expect("docs lock poisoned");
            match docs.get_mut(rel_path) {
                None => {
                    docs.insert(rel_path.to_owned(), (1, text.to_owned()));
                    DocAction::Open
                }
                Some((version, prev)) if *prev != text => {
                    *version += 1;
                    *prev = text.to_owned();
                    DocAction::Change(*version)
                }
                Some(_) => DocAction::Unchanged,
            }
        };
        if matches!(action, DocAction::Unchanged) {
            return Ok(());
        }
        self.warm.store(false, Ordering::SeqCst);
        let uri = self.doc_uri(rel_path);
        match action {
            DocAction::Open => {
                self.notify(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": language_id(lang),
                            "version": 1,
                            "text": text,
                        }
                    }),
                )
                .await
            }
            DocAction::Change(version) => {
                self.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri, "version": version },
                        // 全量替换（TextDocumentSyncKind.Full）。
                        "contentChanges": [{ "text": text }],
                    }),
                )
                .await
            }
            DocAction::Unchanged => Ok(()),
        }
    }

    /// rel_path → 文档 URI。
    pub(crate) fn doc_uri(&self, rel_path: &str) -> String {
        proto::path_to_uri(&self.root.join(rel_path))
    }

    /// 项目根（canonicalize 后）。
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// 记录一次使用（idle 计时起点）。
    pub(crate) fn note_use(&self) {
        *self.last_used.lock().expect("last_used lock poisoned") = Instant::now();
    }

    /// 距最近一次使用的时长（idle 判定用）。
    pub(crate) fn idle_for(&self) -> Duration {
        self.last_used
            .lock()
            .expect("last_used lock poisoned")
            .elapsed()
    }

    /// SIGKILL + 同步收割（防孤儿 / 僵尸）。
    ///
    /// SIGKILL 生效后通常毫秒级退出；轮询上限 3s，兜底由 tokio
    /// kill_on_drop 与孤儿队列接管。
    pub(crate) fn kill_now(&self) {
        let mut child = self.child.lock().expect("child lock poisoned");
        // start_kill：同步发送 SIGKILL（tokio 1.54 的 kill 是 async，不适合同步路径）。
        let _ = child.start_kill();
        for _ in 0..REAP_POLL_MAX {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => std::thread::sleep(REAP_POLL_INTERVAL),
                Err(_) => break,
            }
        }
    }
}

/// stdout 泵：按 id 回填 pending；服务器请求一律回 null；通知仅记录。
///
/// 注意：publishDiagnostics（尤其 didOpen 后毫秒级到达的语法级诊断）
/// 不能作为 workspace 就绪信号，warm 只由实质响应置位（见
/// request_until_meaningful）。
async fn pump_stdout(
    stdout: ChildStdout,
    pending: Arc<DashMap<i64, oneshot::Sender<Reply>>>,
    stdin: Arc<AsyncMutex<ChildStdin>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        let msg = match proto::read_frame(&mut reader).await {
            Ok(Some(msg)) => msg,
            _ => break,
        };
        if let Some(method) = msg.get("method").and_then(Value::as_str) {
            match msg.get("id") {
                Some(id) => {
                    // capabilities 留空 ⇒ 服务器请求极少见；一律回 null。
                    let reply = json!({ "jsonrpc": "2.0", "id": id, "result": null });
                    let mut w = stdin.lock().await;
                    let _ = proto::write_frame(&mut *w, &reply).await;
                }
                None => {
                    tracing::debug!(method, "server notification");
                }
            }
        } else if let Some(id) = msg.get("id").and_then(Value::as_i64) {
            if let Some((_, tx)) = pending.remove(&id) {
                let reply = match msg.get("error") {
                    Some(err) => Err(err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown server error")
                        .to_owned()),
                    None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(reply);
            }
        }
    }
    // 连接关闭：让所有在途等待者立刻失败，而不是干等超时。
    let keys: Vec<i64> = pending.iter().map(|entry| *entry.key()).collect();
    for key in keys {
        if let Some((_, tx)) = pending.remove(&key) {
            let _ = tx.send(Err("language server connection closed".into()));
        }
    }
}

/// stderr 泵 → tracing（stderr 红线：绝不写 stdout）。
async fn pump_stderr(stderr: ChildStderr) {
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let trimmed = line.trim_end();
                if !trimmed.is_empty() {
                    tracing::debug!(target: "codeidx_lsp::stderr", "{}", trimmed);
                }
            }
        }
    }
}
