//! 每个挂载持有独立监听器。事件只决定哪些文件必须复核哈希，
//! 文件是否可索引及新增、删除判断仍以 walker 扫描结果为准。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use codeidx_protocol::ToolError;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{manager::ProjectContext, sync, walker};

const BATCH_DELAY: Duration = Duration::from_millis(150);
const MAX_PENDING_PATHS: usize = 4096;

#[derive(Default)]
pub(crate) struct Changes {
    all: bool,
    paths: HashSet<PathBuf>,
}

impl Changes {
    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.all || path.ancestors().any(|p| self.paths.contains(p))
    }

    fn record(&mut self, event: Event) {
        if self.all {
            return;
        }
        if event.need_rescan() || event.paths.is_empty() {
            self.rescan();
            return;
        }
        self.paths.extend(event.paths);
        if self.paths.len() > MAX_PENDING_PATHS {
            self.rescan();
        }
    }

    fn rescan(&mut self) {
        self.all = true;
        self.paths.clear();
    }
}

/// 先注册监听，挂载扫描期间收到的事件会在上下文就绪后处理。
pub(crate) struct Subscription {
    backend: RecommendedWatcher,
    wake: SyncSender<()>,
    events: mpsc::Receiver<()>,
    pending: Arc<Mutex<Changes>>,
}

impl Subscription {
    pub(crate) fn register(root: &Path) -> Result<Self, ToolError> {
        let (wake, events) = mpsc::sync_channel(1);
        let pending = Arc::new(Mutex::new(Changes::default()));
        let callback_pending = Arc::clone(&pending);
        let callback_wake = wake.clone();
        let watcher_error = |err| ToolError::new(format!("cannot watch {}: {err}", root.display()));
        let mut backend = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let mut changes = callback_pending.lock().unwrap();
            match result {
                Ok(event) if !event.need_rescan() && matches!(event.kind, EventKind::Access(_)) => {
                    return; // 读取源码产生的 access 事件不能触发下一轮扫描。
                }
                Ok(event) => changes.record(event),
                Err(error) => {
                    tracing::warn!(%error, "filesystem watcher error; rescanning project");
                    changes.rescan();
                }
            }
            let _ = callback_wake.try_send(());
        })
        .map_err(watcher_error)?;
        backend
            .watch(root, RecursiveMode::Recursive)
            .map_err(watcher_error)?;
        Ok(Self {
            backend,
            wake,
            events,
            pending,
        })
    }

    pub(crate) fn start(
        self,
        context: Weak<RwLock<ProjectContext>>,
    ) -> Result<ProjectWatcher, ToolError> {
        let Self {
            backend,
            wake,
            events,
            pending,
        } = self;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let thread = thread::Builder::new().name("codeidx-watch".into()).spawn(move || {
            while events.recv().is_ok() && !worker_stop.load(Ordering::Acquire) {
                // 固定窗口合并事件，持续写入时也能按批推进。
                let deadline = Instant::now() + BATCH_DELAY;
                while !worker_stop.load(Ordering::Acquire) && Instant::now() < deadline {
                    if events.recv_timeout(deadline.saturating_duration_since(Instant::now())).is_err() {
                        break;
                    }
                }
                if worker_stop.load(Ordering::Acquire) {
                    break;
                }
                let changes = std::mem::take(&mut *pending.lock().unwrap());
                let Some(ctx) = context.upgrade() else { break };
                let mut ctx = ctx.write().unwrap();
                if worker_stop.load(Ordering::Acquire) || !ctx.root.is_dir() {
                    continue;
                }
                let started = Instant::now();
                let present = walker::collect_with_counts(&ctx.root);
                ctx.callflow_skipped = present.skipped_oversize as usize;
                let report = sync::reconcile_changes(&mut ctx, present.files, started, |path| changes.contains(path));
                if report.failed > 0 {
                    tracing::warn!(root = %ctx.root.display(), failed = report.failed, "automatic sync incomplete");
                }
            }
        }).map_err(|err| ToolError::new(format!("cannot start automatic sync: {err}")))?;
        Ok(ProjectWatcher {
            backend: Some(backend),
            wake,
            stop,
            thread: Some(thread),
        })
    }
}

pub(crate) struct ProjectWatcher {
    backend: Option<RecommendedWatcher>,
    wake: SyncSender<()>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for ProjectWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake.try_send(());
        drop(self.backend.take());
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::warn!("automatic sync thread panicked");
            }
        }
    }
}
