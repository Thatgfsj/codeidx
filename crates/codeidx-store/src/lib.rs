//! codeidx-store —— SQLite `:memory:` + FTS5 +（Phase 2）向量 + RRF 混合检索。
//!
//! Phase 1 交付（Issue #4）：内存 SQLite schema 与 FTS5 BM25 关键词检索。
//! - 库只存在于进程内存（`Connection::open_in_memory`），不落任何磁盘文件；
//! - 全部方法 `&self`，内部 `Mutex<rusqlite::Connection>` 保证跨线程共享；
//! - `replace_file` 单事务替换整文件索引数据，同文件重复调用幂等；
//! - 向量层（Issue #5）：vec0 虚表 + chunk 哈希缓存 + cosine KNN +
//!   RRF(k=60) 混合排序；未启用 / 无 embedder 时 `search_vector` 统一报
//!   `vector search disabled (Phase 2)`，`search_hybrid` 降级为关键词检索。
//!
//! 依赖说明：除 ARCHITECTURE §2 的 `store → ast` 外，本 crate 另依赖叶子
//! crate `codeidx-protocol`（Issue #4 契约规定的 `SearchHit` / `SymbolCode` /
//! `OutlineEntry` DTO 与 `ToolError`）。protocol 无内部依赖，不产生环。

mod call_graph;
mod embedder;
mod schema;
mod search;
mod store;
mod tokens;
mod vec;

pub use call_graph::{CallConfidence, CallGraph, CallSiteKind, CallSymbol, StoredCallSite};
pub use embedder::{Embedder, FastembedEmbedder, EMBED_DIM};
pub use store::{Store, StoreStats};

#[cfg(test)]
mod tests;
