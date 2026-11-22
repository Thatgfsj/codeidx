//! 内存库 schema（ARCHITECTURE §5.7：SQLite 一律 `:memory:`，不落盘）。
//!
//! 表设计对齐 Issue #4 任务书：
//! - `symbols`：符号定义（file_outline / stats 数据源）；
//! - `refs`：符号引用（Phase 2 向量化与 RepoMap 权重数据源）；
//! - `chunks`：代码块原文（read_symbol / 检索反查数据源）；
//! - `fts`：FTS5 全文索引，`tokens` 为分词辅助列（camelCase / snake_case
//!   拆词后的小写词流），弥补 unicode61 分词器无法拆分驼峰标识符的缺口。
//!
//! `chunks.id` / `symbols.id` / `refs.id` 为 rowid 别名；`fts.rowid` 与
//! `chunks.id` 对齐，按文件清理可走 rowid 快路径（见 store.rs）。

use rusqlite::Connection;

/// 建库 DDL。
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE symbols (
    id          INTEGER PRIMARY KEY,
    rel_path    TEXT NOT NULL,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,
    start_line  INTEGER NOT NULL,
    end_line    INTEGER NOT NULL,
    signature   TEXT NOT NULL,
    parent      TEXT,
    source_lang TEXT NOT NULL
);
CREATE INDEX idx_symbols_rel ON symbols(rel_path);
CREATE INDEX idx_symbols_call_name ON symbols(source_lang, name) WHERE kind IN ('fn', 'method');

CREATE TABLE call_sites (
    id INTEGER PRIMARY KEY,
    caller_id INTEGER,
    source_lang TEXT NOT NULL,
    name TEXT NOT NULL,
    rel_path TEXT NOT NULL,
    line INTEGER NOT NULL,
    character INTEGER,
    kind TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    qualifier TEXT,
    candidate_count INTEGER NOT NULL DEFAULT 0,
    confidence TEXT NOT NULL DEFAULT 'unresolved'
);
CREATE INDEX idx_call_sites_rel ON call_sites(rel_path);
CREATE INDEX idx_call_sites_name ON call_sites(source_lang, name);

CREATE TABLE call_edges (
    site_id INTEGER NOT NULL,
    callee_id INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (site_id, ordinal)
) WITHOUT ROWID;
CREATE INDEX idx_call_edges_target ON call_edges(callee_id);

CREATE TABLE refs (
    id       INTEGER PRIMARY KEY,
    rel_path TEXT NOT NULL,
    name     TEXT NOT NULL,
    line     INTEGER NOT NULL
);
CREATE INDEX idx_refs_rel ON refs(rel_path);

CREATE TABLE chunks (
    id           INTEGER PRIMARY KEY,
    rel_path     TEXT NOT NULL,
    symbol       TEXT,
    start_line   INTEGER NOT NULL,
    end_line     INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    text         TEXT NOT NULL
);
CREATE INDEX idx_chunks_rel ON chunks(rel_path);
CREATE INDEX idx_chunks_sym ON chunks(rel_path, symbol);

CREATE VIRTUAL TABLE fts USING fts5(chunk_id UNINDEXED, body, tokens);
"#;

/// 在内存连接上初始化 schema。
pub(crate) fn init(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(SCHEMA)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_initializes_cleanly() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(init(&conn).is_ok());
        // 重复初始化应因表已存在而失败（幂等由调用方保证只执行一次）。
        assert!(init(&conn).is_err());
    }
}
