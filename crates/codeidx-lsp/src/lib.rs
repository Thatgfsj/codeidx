//! codeidx-lsp —— 通用 LSP 子进程桥（Phase 3 / GitHub Issue #6）。
//!
//! 职责：按 (项目根, 语言) 池化外部语言服务器子进程，向 MCP 工具层
//! （lsp_definition / lsp_references / lsp_hover）提供单文件语义查询，
//! 并负责子进程的按需拉起、idle 回收与 unmount 清理。
//!
//! - 行号契约（ARCHITECTURE §4）：对外 1-based line、0-based character
//!   （UTF-16 code unit）；LSP wire 0-based，双向转换只发生在桥内；
//! - wire 协议：stdio 上的 JSON-RPC 2.0，`Content-Length` 头分帧（与
//!   MCP 的行分帧不同，头解析独立实现）；
//! - 生命周期：某 (root, lang) 首次调用时拉起子进程并完成 initialize
//!   握手，之后复用；空闲进程由 reaper 后台任务 SIGKILL 回收，unmount
//!   走同步清理（[`LspBridge::kill_for_root`]）。
//!
//! 依赖方向说明：ARCHITECTURE §2 将 codeidx-lsp 标注为无内部依赖的叶子
//! crate；本 crate 经任务书（Issue #6）授权消费 codeidx-ast 的冻结
//! `Lang` 枚举，不触碰其余任何 crate。

mod bridge;
mod client;
mod detect;
mod proto;

pub use bridge::LspBridge;
pub use proto::{Location, LspError};

#[cfg(test)]
mod tests;
