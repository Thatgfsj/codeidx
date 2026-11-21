//! codeidx-mcp —— 全局按需挂载的代码索引 MCP 服务（stdio JSON-RPC）。
//!
//! Phase 3 接线：lsp_* 三工具走真 LSP 桥（本文件 LSP 单例）；
//! 其余 8 个工具接线真 ProjectManager（本文件 MANAGER 单例）。
//!
//! 红线（ARCHITECTURE §4）：stdout 只允许 JSON-RPC 协议字节，
//! 一切日志 / tracing 一律走 stderr。

mod handlers;
mod router;
mod server;

use std::sync::{Arc, LazyLock};

use codeidx_lsp::LspBridge;
use codeidx_project::ProjectManager;
use tracing_subscriber::EnvFilter;

/// 全局唯一项目管理器：全部工具 handler 共享（Issue #2 接线契约）。
/// LazyLock 首次访问时才构建空挂载表，不增加空闲 RSS（F1.13 门）。
pub(crate) static MANAGER: LazyLock<ProjectManager> = LazyLock::new(ProjectManager::new);

/// 全局唯一 LSP 桥：lsp_* 三工具与 unmount / list 生命周期共享（Phase 3
/// 接线契约）。idle 阈值来自环境变量 CODEIDX_LSP_IDLE_SECS（默认 300
/// 秒）；空桥只有一张空 DashMap，不增加空闲 RSS（F1.13 门）。
pub(crate) static LSP: LazyLock<Arc<LspBridge>> = LazyLock::new(|| Arc::new(LspBridge::from_env()));

fn main() {
    // 日志初始化：显式绑定 stderr writer，绝不触碰 stdout。
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // IO 密集型服务：2 个 worker 足够，同时压低空闲 RSS（F1.13 门）。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build tokio runtime");
    runtime.block_on(server::serve());
    // 显式限时关闭：tokio::io::stdin() 底层的常驻 spawn_blocking 读线程在
    // 客户端仍握着 stdin 时会永远阻塞在 read 上，而 Runtime 的 Drop 会无限
    // 等待 spawn_blocking 任务，导致 SIGTERM/SIGINT 后进程挂死不退。
    // shutdown_timeout 丢弃该等待，保证信号路径最多 1 秒内干净退出（exit 0）。
    runtime.shutdown_timeout(std::time::Duration::from_secs(1));
}
