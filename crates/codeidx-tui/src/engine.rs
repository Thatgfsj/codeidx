//! 引擎桥：TUI ↔ codeidx 引擎层（ProjectManager / ast / store / LspBridge）。
//!
//! 线程模型（Lead 任务书 §架构纪律1）：
//! - mount / sync / search 等全部引擎调用都在独立 std::thread 执行，
//!   结果经 mpsc 回注主循环——UI 线程永不被秒级~分钟级 CPU 工作阻塞；
//! - LSP 共用常驻运行时，请求线程 block_on；服务器 IO 泵与 idle 回收持续运行。
//! - 本层不 import handlers：直接消费引擎层结构化数据。

use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;

use codeidx_ast::{detect_lang, repo_map};
use codeidx_lsp::LspBridge;
use codeidx_project::callflow::{trace_calls, TraceLocation, TraceOptions};
use codeidx_project::ProjectManager;
use codeidx_protocol::tools::{Location, SearchHit, SearchMode, SymbolCode};
use codeidx_protocol::ToolError;

use crate::app::{Cmd, Ev, LspKind, SearchOutcome};

/// 引擎句柄：Arc 共享、`dispatch` 派发后立即返回。
pub struct Engine {
    manager: Arc<ProjectManager>,
    bridge: Arc<LspBridge>,
    runtime: tokio::runtime::Runtime,
    tx: Sender<Ev>,
}

impl Engine {
    /// 构建引擎。`tx` 为引擎结果回注通道（main 循环持有 rx）。
    /// LSP 空闲阈值从 `CODEIDX_LSP_IDLE_SECS` 读取（默认 300）。
    pub fn new(tx: Sender<Ev>) -> std::io::Result<Engine> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let bridge = Arc::new(LspBridge::from_env());
        {
            let _guard = runtime.enter();
            bridge.spawn_reaper();
        }
        Ok(Engine {
            manager: Arc::new(ProjectManager::new()),
            bridge,
            runtime,
            tx,
        })
    }

    /// 退出清理：回收全部 LSP 子进程（terminal teardown 之后调用）。
    pub fn shutdown(self) {
        self.runtime
            .shutdown_timeout(std::time::Duration::from_secs(1));
        self.bridge.kill_all();
    }

    /// 派发命令：立即返回，工作线程完成或失败后回注 Ev。
    /// Quit 为纯 UI 退出标记，main 循环不派发，此分支不可达（防御保留）。
    pub fn dispatch(&self, cmd: Cmd) {
        if matches!(cmd, Cmd::Quit) {
            return;
        }
        let manager = Arc::clone(&self.manager);
        let bridge = Arc::clone(&self.bridge);
        let runtime = self.runtime.handle().clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let ev = run_cmd(&manager, &bridge, &runtime, cmd);
            let _ = tx.send(ev);
        });
    }
}

/// 单命令执行：全部错误就地转字符串回注（UI 展示，不 panic）。
fn run_cmd(
    manager: &ProjectManager,
    bridge: &LspBridge,
    runtime: &tokio::runtime::Handle,
    cmd: Cmd,
) -> Ev {
    match cmd {
        Cmd::Mount {
            path,
            enable_vector,
        } => match manager.mount(&path, enable_vector) {
            Ok(report) => Ev::Mount(Ok(report)),
            Err(e) => Ev::Mount(Err(e.message)),
        },
        Cmd::Unmount(path) => {
            let killed = bridge.kill_for_root(&path) as u32;
            match manager.unmount(&path) {
                Ok(report) => {
                    let _ = report;
                    Ev::Unmount(Ok((path, killed)))
                }
                Err(e) => Ev::Unmount(Err(e.message)),
            }
        }
        Cmd::Sync(path) => match manager.sync(&path) {
            Ok(report) => Ev::Sync(Ok(report)),
            Err(e) => Ev::Sync(Err(e.message)),
        },
        Cmd::RefreshProjects => Ev::Projects(manager.list()),
        Cmd::RepoMap { path, budget } => {
            let result = manager.with_project(&path, |ctx| {
                // 聚合 parsed 全集（与 handlers::get_repo_map 同源逻辑，但直连
                // ast 纯函数而非 handler 文本产物）。
                let mut symbols = Vec::new();
                let mut refs = Vec::new();
                for pf in ctx.parsed.values() {
                    symbols.extend(pf.symbols.iter().cloned());
                    refs.extend(pf.refs.iter().cloned());
                }
                Ok(repo_map(&symbols, &refs, &[], budget as usize))
            });
            match result {
                Ok(text) => Ev::RepoMap(Ok((budget, text))),
                Err(e) => Ev::RepoMap(Err(e.message)),
            }
        }
        Cmd::Search {
            path,
            query,
            mode,
            top_k,
        } => {
            let started = Instant::now();
            let result = manager.with_project(&path, |ctx| {
                // 降级语义对齐 handlers::search_code：vector 未就绪时
                // vector/hybrid 一律降级 keyword，原因回显。
                let ready = ctx.store.is_vector_ready();
                let (effective, degraded, reason) = if ready || mode == SearchMode::Keyword {
                    (mode, false, String::new())
                } else {
                    let reason = if ctx.vector_degraded && !ctx.vector_degraded_reason.is_empty() {
                        ctx.vector_degraded_reason.clone()
                    } else {
                        "vector not enabled for this mount".to_string()
                    };
                    (SearchMode::Keyword, true, reason)
                };
                let hits: Vec<SearchHit> = match effective {
                    SearchMode::Keyword => ctx.store.search_keyword(&query, top_k as usize)?,
                    SearchMode::Vector => ctx.store.search_vector(&query, top_k as usize)?,
                    SearchMode::Hybrid => ctx.store.search_hybrid(&query, top_k as usize)?,
                };
                Ok(SearchOutcome {
                    hits,
                    effective_mode: effective,
                    degraded,
                    degraded_reason: reason,
                    elapsed_ms: started.elapsed().as_millis() as u64,
                })
            });
            match result {
                Ok(outcome) => Ev::Search(Ok(outcome)),
                Err(e) => Ev::Search(Err(e.message)),
            }
        }
        Cmd::Outline { path, file } => {
            let result = manager.with_project(&path, |ctx| {
                if !ctx.files.contains_key(&file) {
                    return Err(ToolError::new(format!("file not indexed: {file}")));
                }
                ctx.store.file_outline(&file)
            });
            match result {
                Ok(entries) => Ev::Outline(Ok(entries)),
                Err(e) => Ev::Outline(Err(e.message)),
            }
        }
        Cmd::ReadSymbol { path, file, name } => {
            let result = manager.with_project(&path, |ctx| ctx.store.read_symbol(&file, &name));
            match result {
                Ok(opt) => Ev::Symbol(Ok(opt)),
                Err(e) => Ev::Symbol(Err(e.message)),
            }
        }
        Cmd::LspDefinition {
            path,
            file,
            line,
            character,
        } => lsp_run(
            bridge,
            runtime,
            &path,
            &file,
            line,
            character,
            LspKind::Definition,
        ),
        Cmd::LspReferences {
            path,
            file,
            line,
            character,
        } => lsp_run(
            bridge,
            runtime,
            &path,
            &file,
            line,
            character,
            LspKind::References,
        ),
        Cmd::TraceCalls {
            path,
            id,
            from,
            direction,
            refine,
        } => {
            let options = TraceOptions {
                from: from.clone(),
                direction,
                refine_lsp: refine,
                ..TraceOptions::default()
            };
            let result = runtime
                .block_on(trace_calls(
                    manager,
                    Some(bridge),
                    &path.to_string_lossy(),
                    options,
                ))
                .map_err(|e| e.message);
            Ev::TraceCalls(id, from, result)
        }
        Cmd::ReadTraceSymbol { path, location } => {
            let result = read_trace_symbol(manager, &path, &location).map_err(|e| e.message);
            Ev::TraceSymbol(path, result)
        }
        Cmd::Quit => Ev::Projects(Vec::new()),
    }
}

/// Read the indexed definition at exactly the selected location. Symbols can
/// share a name even within the same file, so a name-only lookup is insufficient.
fn read_trace_symbol(
    manager: &ProjectManager,
    path: &Path,
    at: &TraceLocation,
) -> Result<SymbolCode, ToolError> {
    manager.with_project(path, |ctx| {
        let mut matches = ctx
            .parsed
            .get(&at.file)
            .into_iter()
            .flat_map(|file| &file.chunks)
            .filter(|chunk| {
                chunk.start_line == at.line && chunk.symbol.as_deref() == Some(at.symbol.as_str())
            });
        let chunk = matches
            .next()
            .ok_or_else(|| ToolError::new("调用链位置已失效，按 r 刷新"))?;
        if matches.next().is_some() {
            return Err(ToolError::new("同一行包含多个同名定义，无法定位唯一源码"));
        }
        let (_, code) = chunk.text.split_once('\n').unwrap_or(("", &chunk.text));
        Ok(SymbolCode {
            file: at.file.clone(),
            name: at.symbol.clone(),
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            code: code.to_owned(),
        })
    })
}

/// LSP 请求：语言探测 → 读源文件文本 → 使用常驻 runtime。
fn lsp_run(
    bridge: &LspBridge,
    runtime: &tokio::runtime::Handle,
    root: &Path,
    rel_path: &str,
    line: u32,
    character: u32,
    kind: LspKind,
) -> Ev {
    let Some(lang) = detect_lang(rel_path) else {
        return Ev::Lsp(Err(format!("unsupported language for LSP: {rel_path}")));
    };
    let Ok(text) = std::fs::read_to_string(root.join(rel_path)) else {
        return Ev::Lsp(Err(format!("cannot read file {rel_path}")));
    };
    let result = runtime.block_on(async {
        match kind {
            LspKind::Definition => {
                bridge
                    .definition(root, lang, rel_path, line, character, &text)
                    .await
            }
            LspKind::References => {
                bridge
                    .references(root, lang, rel_path, line, character, &text)
                    .await
            }
        }
    });
    match result {
        Ok(locations) => Ev::Lsp(Ok((
            kind,
            locations
                .into_iter()
                .map(|loc| Location {
                    file: loc.file,
                    line: loc.line,
                    character: loc.character,
                })
                .collect(),
        ))),
        Err(e) => Ev::Lsp(Err(e.0)),
    }
}

/// HOME 目录列表（未挂载页选择列表；IO 在调用方线程执行）。
pub fn list_home_dirs() -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let mut dirs: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&home) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    dirs.push(format!("{home}/{name}"));
                }
            }
        }
    }
    dirs.sort();
    dirs
}

/// 读 /proc/self/status 的 VmRSS（KB）；不可用返回 None。
pub fn vm_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .ok();
        }
    }
    None
}
