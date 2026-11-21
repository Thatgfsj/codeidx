//! Store 主体：结构定义、生命周期与除检索外的全部读写方法。
//!
//! 并发模型：单 `Mutex<Connection>`，每个方法"拿锁 → 完成 → 放锁"，
//! 无嵌套锁，跨线程共享无死锁（并发冒烟测试见 tests.rs）。

use std::sync::Mutex;

use codeidx_ast::{ParsedFile, SymbolKind};
use codeidx_protocol::{OutlineEntry, SymbolCode, ToolError};
use rusqlite::{params, Connection};

use crate::schema;
use crate::tokens;

/// 内存索引库（Issue #4 契约）。全部方法 `&self`，可跨线程共享。
pub struct Store {
    pub(crate) conn: Mutex<Connection>,
    /// Phase 1 仅记录开关（Issue #4）；Phase 2 用于决定向量表初始化。
    enable_vector: bool,
}

/// 库内统计计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreStats {
    /// 已索引文件数（chunks 表 distinct rel_path 数）。
    pub files: u64,
    /// 符号定义总数（symbols 表行数）。
    pub symbols: u64,
}

/// rusqlite 错误 → ToolError（孤儿规则不允许跨 crate impl From，走辅助函数）。
pub(crate) fn sqlite_err(e: rusqlite::Error) -> ToolError {
    ToolError::new(format!("sqlite: {e}"))
}

/// Mutex 中毒（仅当持锁线程 panic 后才会发生）。
pub(crate) fn poisoned() -> ToolError {
    ToolError::new("store mutex poisoned")
}

/// SymbolKind → outline 的小写字符串形态（protocol tools.rs 契约：
/// "fn"、"class" 等；Phase 1-C 的映射对齐此表）。
fn kind_to_lower(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Fn => "fn",
        SymbolKind::Method => "method",
        SymbolKind::Struct => "struct",
        SymbolKind::Class => "class",
        SymbolKind::Trait => "trait",
        SymbolKind::Interface => "interface",
        SymbolKind::Enum => "enum",
        SymbolKind::Const => "const",
        SymbolKind::Type => "type",
        SymbolKind::Module => "module",
        SymbolKind::Other => "other",
    }
}

impl Store {
    /// 新建内存库（硬约束：`Connection::open_in_memory`，不落任何磁盘文件）。
    /// `enable_vector` Phase 1 仅记录开关，向量表留给 Phase 2。
    pub fn new(enable_vector: bool) -> Result<Store, ToolError> {
        let conn = Connection::open_in_memory().map_err(sqlite_err)?;
        schema::init(&conn).map_err(sqlite_err)?;
        Ok(Store {
            conn: Mutex::new(conn),
            enable_vector,
        })
    }

    /// Phase 1 记录的向量开关（Phase 2 决定向量表与降级策略）。
    pub fn is_vector_enabled(&self) -> bool {
        self.enable_vector
    }

    /// 单事务替换一个文件的全部索引数据：先删该 rel_path 的旧
    /// symbols/refs/chunks/fts 行，再写入新数据。同文件重复调用幂等。
    pub fn replace_file(&self, parsed: &ParsedFile) -> Result<(), ToolError> {
        let mut conn = self.conn.lock().map_err(|_| poisoned())?;
        let tx = conn.transaction().map_err(sqlite_err)?;
        // 1) 清旧：fts 先于 chunks（DELETE 依赖 chunks 子查询）。
        tx.execute(
            "DELETE FROM fts WHERE rowid IN (SELECT id FROM chunks WHERE rel_path = ?1)",
            params![parsed.rel_path],
        )
        .map_err(sqlite_err)?;
        tx.execute(
            "DELETE FROM chunks WHERE rel_path = ?1",
            params![parsed.rel_path],
        )
        .map_err(sqlite_err)?;
        tx.execute(
            "DELETE FROM symbols WHERE rel_path = ?1",
            params![parsed.rel_path],
        )
        .map_err(sqlite_err)?;
        tx.execute(
            "DELETE FROM refs WHERE rel_path = ?1",
            params![parsed.rel_path],
        )
        .map_err(sqlite_err)?;
        // 2) 写新：symbols / refs。
        for s in &parsed.symbols {
            tx.execute(
                "INSERT INTO symbols(rel_path, name, kind, start_line, end_line, signature, parent)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    parsed.rel_path,
                    s.name,
                    kind_to_lower(s.kind),
                    s.start_line,
                    s.end_line,
                    s.signature,
                    s.parent
                ],
            )
            .map_err(sqlite_err)?;
        }
        for r in &parsed.refs {
            tx.execute(
                "INSERT INTO refs(rel_path, name, line) VALUES (?1, ?2, ?3)",
                params![parsed.rel_path, r.name, r.line],
            )
            .map_err(sqlite_err)?;
        }
        // 3) 写新：chunks + fts（fts.rowid 与 chunk.id 对齐；tokens 为分词辅助列）。
        for c in &parsed.chunks {
            tx.execute(
                "INSERT INTO chunks(rel_path, symbol, start_line, end_line, content_hash, text)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    parsed.rel_path,
                    c.symbol,
                    c.start_line,
                    c.end_line,
                    c.content_hash,
                    c.text
                ],
            )
            .map_err(sqlite_err)?;
            let chunk_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO fts(rowid, chunk_id, body, tokens) VALUES (?1, ?1, ?2, ?3)",
                params![chunk_id, c.text, tokens::tokens_column(&c.text)],
            )
            .map_err(sqlite_err)?;
        }
        tx.commit().map_err(sqlite_err)
    }

    /// 删除一个文件的全部索引数据（symbols/refs/chunks/fts 行）。
    /// 文件不存在时为无操作（幂等）。
    pub fn remove_file(&self, rel: &str) -> Result<(), ToolError> {
        let mut conn = self.conn.lock().map_err(|_| poisoned())?;
        let tx = conn.transaction().map_err(sqlite_err)?;
        tx.execute(
            "DELETE FROM fts WHERE rowid IN (SELECT id FROM chunks WHERE rel_path = ?1)",
            params![rel],
        )
        .map_err(sqlite_err)?;
        tx.execute("DELETE FROM chunks WHERE rel_path = ?1", params![rel])
            .map_err(sqlite_err)?;
        tx.execute("DELETE FROM symbols WHERE rel_path = ?1", params![rel])
            .map_err(sqlite_err)?;
        tx.execute("DELETE FROM refs WHERE rel_path = ?1", params![rel])
            .map_err(sqlite_err)?;
        tx.commit().map_err(sqlite_err)
    }

    /// 精确读取符号完整代码块：chunk 表 rel_path+symbol 精确匹配首个命中；
    /// `code` 为去掉注入上下文头后的完整文本；未命中返回 `Ok(None)`。
    pub fn read_symbol(&self, file: &str, name: &str) -> Result<Option<SymbolCode>, ToolError> {
        let conn = self.conn.lock().map_err(|_| poisoned())?;
        let mut stmt = conn
            .prepare(
                "SELECT start_line, end_line, text FROM chunks
                 WHERE rel_path = ?1 AND symbol = ?2
                 ORDER BY id ASC LIMIT 1",
            )
            .map_err(sqlite_err)?;
        let mut rows = stmt.query(params![file, name]).map_err(sqlite_err)?;
        match rows.next().map_err(sqlite_err)? {
            Some(row) => {
                let start_line: u32 = row.get(0).map_err(sqlite_err)?;
                let end_line: u32 = row.get(1).map_err(sqlite_err)?;
                let text: String = row.get(2).map_err(sqlite_err)?;
                Ok(Some(SymbolCode {
                    file: file.to_string(),
                    name: name.to_string(),
                    start_line,
                    end_line,
                    code: tokens::strip_context_header(&text),
                }))
            }
            None => Ok(None),
        }
    }

    /// 文件 outline：symbols 按 (start_line, id) 升序输出小写 kind。
    pub fn file_outline(&self, file: &str) -> Result<Vec<OutlineEntry>, ToolError> {
        let conn = self.conn.lock().map_err(|_| poisoned())?;
        let mut stmt = conn
            .prepare(
                "SELECT kind, name, start_line, end_line, signature FROM symbols
                 WHERE rel_path = ?1
                 ORDER BY start_line ASC, id ASC",
            )
            .map_err(sqlite_err)?;
        let entries = stmt
            .query_map(params![file], |row| {
                Ok(OutlineEntry {
                    kind: row.get(0)?,
                    name: row.get(1)?,
                    start_line: row.get(2)?,
                    end_line: row.get(3)?,
                    sig: row.get(4)?,
                })
            })
            .map_err(sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;
        Ok(entries)
    }

    /// 库内统计：files = chunks 表 distinct rel_path 数；symbols = symbols 表行数。
    ///
    /// 契约限定无 Result 返回；锁中毒或计数查询失败（正常初始化后不可能
    /// 发生）兜底返回全零计数，绝不 panic。
    pub fn stats(&self) -> StoreStats {
        let Ok(conn) = self.conn.lock() else {
            return StoreStats {
                files: 0,
                symbols: 0,
            };
        };
        // rusqlite 的 FromSql 未覆盖 u64，按 i64 读取后转换。
        let files: i64 = conn
            .query_row("SELECT COUNT(DISTINCT rel_path) FROM chunks", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        let symbols: i64 = conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
            .unwrap_or(0);
        StoreStats {
            files: u64::try_from(files).unwrap_or(0),
            symbols: u64::try_from(symbols).unwrap_or(0),
        }
    }
}
