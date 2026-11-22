//! codeidx-project —— 多项目挂载管理器与增量索引引擎（Phase 1-B）。
//!
//! 对外契约（Issue #2 精确签名）：
//! - [`ProjectManager`]：DashMap 挂载表。mount 全量建库（ignore 遍历 +
//!   rayon 并行解析），unmount 即时释放，list 汇总，sync 增量重建，
//!   with_project 是全部查询工具的统一入口；
//! - [`ProjectContext`]：单项目内存索引上下文（store + parsed 缓存 +
//!   文件记录表）；
//! - [`FileRecord`]：mtime / size / blake3 三元组，增量 sync 的快筛依据。
//!
//! 红线（ARCHITECTURE §4/§5）：本 crate 不向 stdout 输出任何字节；
//! 单文件解析失败一律 tracing（stderr）警告后跳过，绝不 panic 中断整库。

pub mod callflow;
mod callflow_types;
mod manager;
mod mount;
mod sync;
mod walker;
mod watcher;

#[cfg(test)]
mod tests;

pub use manager::{FileRecord, ProjectContext, ProjectManager};
