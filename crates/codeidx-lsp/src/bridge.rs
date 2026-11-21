//! [`LspBridge`]：按 (项目根, 语言) 池化语言服务器子进程的对外门面。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use codeidx_ast::Lang;
use dashmap::DashMap;
use serde_json::{json, Value};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::client::LspProcess;
use crate::proto::{self, Location, LspError};

/// 池化键：项目根（canonicalize 后）× 语言。
type ProcKey = (PathBuf, Lang);

/// 通用 LSP 桥。
///
/// 用法：`Arc<LspBridge>` 全局单例共享；进程按需拉起并复用，空闲进程
/// 由 [`LspBridge::spawn_reaper`] 后台回收，unmount 时用
/// [`LspBridge::kill_for_root`] 同步清理。
pub struct LspBridge {
    procs: DashMap<ProcKey, Arc<LspProcess>>,
    /// 串行化 spawn：并发首发不双拉起。
    spawn_lock: AsyncMutex<()>,
    idle: Duration,
}

impl LspBridge {
    /// 构造桥；`idle_secs` 为空闲回收阈值（秒）。
    ///
    /// 生产入口应从环境变量 `CODEIDX_LSP_IDLE_SECS` 读取（默认 300），
    /// 见 [`LspBridge::from_env`]；测试传小值（如 2）。
    pub fn new(idle_secs: u64) -> LspBridge {
        LspBridge {
            procs: DashMap::new(),
            spawn_lock: AsyncMutex::new(()),
            idle: Duration::from_secs(idle_secs),
        }
    }

    /// 生产入口便捷构造：读取 `CODEIDX_LSP_IDLE_SECS`（非法或缺省回退 300）。
    pub fn from_env() -> LspBridge {
        let secs = std::env::var("CODEIDX_LSP_IDLE_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(300);
        LspBridge::new(secs)
    }

    fn normalize_root(root: &Path) -> PathBuf {
        std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
    }

    /// 按 (root, lang) 取进程：首次调用拉起子进程并完成 initialize 握手，
    /// 之后复用。
    async fn process_for(&self, root: &Path, lang: Lang) -> Result<Arc<LspProcess>, LspError> {
        let key = (Self::normalize_root(root), lang);
        if let Some(proc) = self.procs.get(&key) {
            return Ok(Arc::clone(proc.value()));
        }
        // 双检 + 串行 spawn：并发首发也只拉起一个子进程。
        let _guard = self.spawn_lock.lock().await;
        if let Some(proc) = self.procs.get(&key) {
            return Ok(Arc::clone(proc.value()));
        }
        let proc = LspProcess::spawn(&key.0, lang).await?;
        tracing::info!(binary = proc.binary, root = ?key.0, "language server started");
        self.procs.insert(key, Arc::clone(&proc));
        Ok(proc)
    }

    /// textDocument/definition（对外 1-based line、0-based character）。
    pub async fn definition(
        &self,
        root: &Path,
        lang: Lang,
        rel_path: &str,
        line: u32,
        character: u32,
        file_text: &str,
    ) -> Result<Vec<Location>, LspError> {
        let proc = self.process_for(root, lang).await?;
        proc.note_use();
        proc.ensure_doc(rel_path, lang, file_text).await?;
        let params = position_params(&proc.doc_uri(rel_path), line, character);
        let result = proc
            .request_until_meaningful("textDocument/definition", params)
            .await?;
        Ok(proto::parse_locations(&result, proc.root()))
    }

    /// textDocument/references（includeDeclaration=false）。
    pub async fn references(
        &self,
        root: &Path,
        lang: Lang,
        rel_path: &str,
        line: u32,
        character: u32,
        file_text: &str,
    ) -> Result<Vec<Location>, LspError> {
        let proc = self.process_for(root, lang).await?;
        proc.note_use();
        proc.ensure_doc(rel_path, lang, file_text).await?;
        let mut params = position_params(&proc.doc_uri(rel_path), line, character);
        params["context"] = json!({ "includeDeclaration": false });
        let result = proc
            .request_until_meaningful("textDocument/references", params)
            .await?;
        Ok(proto::parse_locations(&result, proc.root()))
    }

    /// textDocument/hover → 纯文本 contents（无信息时为空串）。
    pub async fn hover(
        &self,
        root: &Path,
        lang: Lang,
        rel_path: &str,
        line: u32,
        character: u32,
        file_text: &str,
    ) -> Result<String, LspError> {
        let proc = self.process_for(root, lang).await?;
        proc.note_use();
        proc.ensure_doc(rel_path, lang, file_text).await?;
        let params = position_params(&proc.doc_uri(rel_path), line, character);
        let result = proc
            .request_until_meaningful("textDocument/hover", params)
            .await?;
        Ok(proto::parse_hover(&result))
    }

    /// unmount 用：SIGKILL 该 root 全部语言服务器并同步收割，返回 kill 数。
    pub fn kill_for_root(&self, root: &Path) -> usize {
        let root = Self::normalize_root(root);
        let keys: Vec<ProcKey> = self
            .procs
            .iter()
            .filter(|entry| entry.key().0 == root)
            .map(|entry| entry.key().clone())
            .collect();
        let mut killed = 0;
        for key in keys {
            if let Some((_, proc)) = self.procs.remove(&key) {
                tracing::info!(binary = proc.binary, "language server killed for unmount");
                proc.kill_now();
                killed += 1;
            }
        }
        killed
    }

    /// list_mounted_projects 用：该 root 当前活跃的语言服务器名（去重排序）。
    pub fn active_lsps(&self, root: &Path) -> Vec<String> {
        let root = Self::normalize_root(root);
        let mut names: Vec<String> = self
            .procs
            .iter()
            .filter(|entry| entry.key().0 == root)
            .map(|entry| entry.binary.to_owned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// idle 回收：后台任务周期扫描，最后使用距今超过 idle_secs 的进程
    /// SIGKILL + 收割；扫描周期 `max(1, idle_secs / 4)` 秒。
    ///
    /// 返回的 JoinHandle 供测试 abort 控制；生产入口可放任常驻（任务
    /// 持有 `Arc<LspBridge>`，与桥共存亡）。
    pub fn spawn_reaper(self: &Arc<Self>) -> JoinHandle<()> {
        let bridge = Arc::clone(self);
        let period = Duration::from_secs((bridge.idle.as_secs() / 4).max(1));
        tokio::spawn(async move {
            tracing::debug!(period_secs = period.as_secs(), "LSP idle reaper started");
            loop {
                sleep(period).await;
                let mut idle_keys = Vec::new();
                for entry in bridge.procs.iter() {
                    if entry.value().idle_for() >= bridge.idle {
                        idle_keys.push(entry.key().clone());
                    }
                }
                for key in idle_keys {
                    if let Some((_, proc)) = bridge.procs.remove(&key) {
                        tracing::info!(
                            binary = proc.binary,
                            idle_secs = bridge.idle.as_secs(),
                            "reaping idle language server"
                        );
                        proc.kill_now();
                    }
                }
            }
        })
    }
}

/// wire 位置参数：对外 1-based line → wire 0-based；character 原样
/// （本就 0-based）。
fn position_params(uri: &str, line: u32, character: u32) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line.saturating_sub(1), "character": character },
    })
}
