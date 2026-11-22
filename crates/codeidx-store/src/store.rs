//! Store 主体：结构定义、生命周期与除检索外的全部读写方法。
//!
//! 并发模型：单 `Mutex<Connection>`，每个方法"拿锁 → 完成 → 放锁"，
//! 读嵌入器时先取快照并释放读锁，再获取连接锁。注入时持有连接锁至发布嵌入器。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use codeidx_ast::{CallSite, ParsedFile, SymbolKind};
use codeidx_protocol::{OutlineEntry, SymbolCode, ToolError};
use rusqlite::{params, Connection};

use crate::call_graph;
use crate::embedder::Embedder;
use crate::schema;
use crate::tokens;
use crate::vec;

/// 内存索引库（Issue #4 契约）。全部方法 `&self`，可跨线程共享。
pub struct Store {
    pub(crate) conn: Mutex<Connection>,
    /// 向量开关：true 时首次注入 embedder 后建 vec0 虚表并维护向量列。
    pub(crate) enable_vector: bool,
    /// vec_chunks 是否已按 `vec_dim` 建成（延迟建表：维度来自首次注入
    /// 的 embedder，CODEIDX_EMBED_MODEL 切换模型时自适应）。
    vec_ready: AtomicBool,
    /// 已建 vec_chunks 的维度（0 = 未建）。
    vec_dim: AtomicUsize,
    /// 嵌入器注入点（None = 纯 FTS5）。写入与检索只做快照式读取。
    pub(crate) embedder: RwLock<Option<Arc<dyn Embedder>>>,
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
    ///
    /// `enable_vector=true` 时做 vec0 扩展的进程级幂等注册
    /// （`vec::ensure_vec_registered`，探针路径 A）。vec_chunks 虚表
    /// 延迟到首次注入 embedder 时按 `Embedder::dim()` 动态创建
    /// （CODEIDX_EMBED_MODEL 模型切换的维度自适应）。
    pub fn new(enable_vector: bool) -> Result<Store, ToolError> {
        if enable_vector {
            vec::ensure_vec_registered();
        }
        let conn = Connection::open_in_memory().map_err(sqlite_err)?;
        schema::init(&conn).map_err(sqlite_err)?;
        Ok(Store {
            conn: Mutex::new(conn),
            enable_vector,
            vec_ready: AtomicBool::new(false),
            vec_dim: AtomicUsize::new(0),
            embedder: RwLock::new(None),
        })
    }

    /// 向量开关（mount 降级后为 false，F2.1）。
    pub fn is_vector_enabled(&self) -> bool {
        self.enable_vector
    }

    /// 向量检索是否实际可用（开关开启且已注入 embedder）。
    /// handlers 层据此降级并把 mode 回显为实际执行模式（F2.5 可见性契约）。
    pub fn is_vector_ready(&self) -> bool {
        self.enable_vector && self.embedder_snapshot().is_some()
    }

    /// 注入或撤除嵌入器。首次注入时建表，后续仅接受同维度的嵌入器。
    /// 撤除后禁用向量检索，残留向量在后续替换或删除文件时清理。
    pub fn set_embedder(&self, e: Option<Arc<dyn Embedder>>) {
        let Some(embedder) = e else {
            // 撤除：仅清空注入点，已建表与残留向量行的 purge 语义不变。
            if let Ok(mut guard) = self.embedder.write() {
                *guard = None;
            }
            return;
        };
        if !self.enable_vector {
            // 开关关闭：注入无行为意义，仅记录（不建表、无向量写入）。
            if let Ok(mut guard) = self.embedder.write() {
                *guard = Some(embedder);
            }
            return;
        }
        let dim = embedder.dim();
        // 在同一连接锁下判断、首次建表并发布，避免并发首次注入重复建表。
        let Ok(conn) = self.conn.lock() else {
            return;
        };
        if self.vec_ready.load(Ordering::Acquire) {
            let want = self.vec_dim.load(Ordering::Acquire);
            if dim != want {
                tracing::warn!(
                    got = dim,
                    want,
                    "embedder dim mismatch with existing vec_chunks; keeping previous embedder"
                );
                return;
            }
        } else {
            if let Err(err) = vec::init_vec_table(&conn, dim) {
                tracing::warn!(
                    error = %sqlite_err(err).message,
                    "vec_chunks table creation failed; embedder not injected"
                );
                return; // 注入被拒：向量层保持 disabled（FTS 不受影响）。
            }
            self.vec_dim.store(dim, Ordering::Release);
            self.vec_ready.store(true, Ordering::Release);
            tracing::debug!(dim, "vec_chunks table created");
        }
        if let Ok(mut guard) = self.embedder.write() {
            *guard = Some(embedder);
        }
    }

    /// 嵌入器快照（读锁 + Arc 克隆，调用方持快照期间不受撤除影响）。
    pub(crate) fn embedder_snapshot(&self) -> Option<Arc<dyn Embedder>> {
        self.embedder.read().ok().and_then(|guard| guard.clone())
    }

    /// 单事务替换一个文件的全部索引数据：先删该 rel_path 的旧
    /// symbols/refs/chunks/fts（及向量启用时的 vec_chunks）行，再写入
    /// 新数据。同文件重复调用幂等。
    ///
    /// chunk 哈希缓存（F2.4 核心）：事务前读出旧 chunk 的向量，新 chunk
    /// 的 (symbol, 行区间, content_hash) 与旧值一致时直接复用旧向量，
    /// 不重 embed；只有新 / 变更 chunk 才进入待嵌入批。无 embedder 时
    /// 跳过向量列（纯 FTS5 模式）。嵌入在事务提交后批量执行（锁内、
    /// 事务外）：失败仅 stderr 警告，FTS 已入库且下次替换自愈。
    pub fn replace_file(&self, parsed: &ParsedFile) -> Result<(), ToolError> {
        self.replace_file_inner(parsed, None)
    }

    /// Replace a file and its syntax-backed call sites in the same transaction.
    pub fn replace_file_with_calls(
        &self,
        parsed: &ParsedFile,
        calls: &[CallSite],
    ) -> Result<(), ToolError> {
        self.replace_file_inner(parsed, Some(calls))
    }

    fn replace_file_inner(
        &self,
        parsed: &ParsedFile,
        calls: Option<&[CallSite]>,
    ) -> Result<(), ToolError> {
        let embedder = self.embedder_snapshot();
        let vector_active = self.enable_vector && embedder.is_some();
        let mut conn = self.conn.lock().map_err(|_| poisoned())?;
        // 复用集必须先于 DELETE chunks 读出（F2.4：未变 chunk 不重 embed）。
        let reusable: HashMap<vec::ReuseKey, Vec<u8>> = if vector_active {
            vec::read_reusable_vectors(&conn, &parsed.rel_path)
        } else {
            HashMap::new()
        };
        let tx = conn.transaction().map_err(sqlite_err)?;
        let dirty_calls = call_graph::old_definitions(&tx, &parsed.rel_path).map_err(sqlite_err)?;
        let source_lang = format!("{:?}", parsed.lang);
        // 1) 清旧：fts → vec_chunks → chunks（DELETE 依赖 chunks 子查询）。
        tx.execute(
            "DELETE FROM fts WHERE rowid IN (SELECT id FROM chunks WHERE rel_path = ?1)",
            params![parsed.rel_path],
        )
        .map_err(sqlite_err)?;
        if self.vec_ready.load(Ordering::Acquire) {
            tx.execute(
                "DELETE FROM vec_chunks WHERE chunk_id IN (SELECT id FROM chunks WHERE rel_path = ?1)",
                params![parsed.rel_path],
            )
            .map_err(sqlite_err)?;
        }
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
                "INSERT INTO symbols(rel_path, name, kind, start_line, end_line, signature, parent, source_lang)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    parsed.rel_path,
                    s.name,
                    kind_to_lower(s.kind),
                    s.start_line,
                    s.end_line,
                    s.signature,
                    s.parent,
                    source_lang
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
        call_graph::replace_sites(&tx, parsed, calls, &source_lang, dirty_calls)
            .map_err(sqlite_err)?;
        // 3) 写新：chunks + fts +（向量激活时）复用或收集待嵌入。
        //    fts.rowid 与 chunk.id 对齐；tokens 为分词辅助列。
        let mut pending: Vec<(i64, String)> = Vec::new();
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
            if vector_active {
                let key = (
                    c.symbol.clone(),
                    c.start_line,
                    c.end_line,
                    c.content_hash.clone(),
                );
                match reusable.get(&key) {
                    // 哈希缓存命中：旧向量直接复用，零 embed 开销（F2.4）。
                    Some(blob) => {
                        tx.execute(
                            "INSERT INTO vec_chunks(chunk_id, embedding) VALUES (?1, ?2)",
                            params![chunk_id, blob],
                        )
                        .map_err(sqlite_err)?;
                    }
                    // 新 / 变更 / 曾嵌入失败的 chunk：待嵌入批。
                    None => pending.push((chunk_id, c.text.clone())),
                }
            }
        }
        tx.commit().map_err(sqlite_err)?;
        // 4) 事务提交后批嵌入（锁内、事务外）：失败仅警告不回滚 FTS。
        if let Some(em) = &embedder {
            vec::write_pending_vectors(&conn, em.as_ref(), &pending);
        }
        Ok(())
    }

    /// 删除一个文件的全部索引数据（symbols/refs/chunks/fts 行）。
    /// 文件不存在时为无操作（幂等）。
    pub fn remove_file(&self, rel: &str) -> Result<(), ToolError> {
        let mut conn = self.conn.lock().map_err(|_| poisoned())?;
        let tx = conn.transaction().map_err(sqlite_err)?;
        let dirty_calls = call_graph::old_definitions(&tx, rel).map_err(sqlite_err)?;
        call_graph::remove_sites(&tx, rel).map_err(sqlite_err)?;
        tx.execute(
            "DELETE FROM fts WHERE rowid IN (SELECT id FROM chunks WHERE rel_path = ?1)",
            params![rel],
        )
        .map_err(sqlite_err)?;
        if self.vec_ready.load(Ordering::Acquire) {
            // 向量行同步删除（先于 chunks：子查询依赖 chunks 行）。
            tx.execute(
                "DELETE FROM vec_chunks WHERE chunk_id IN (SELECT id FROM chunks WHERE rel_path = ?1)",
                params![rel],
            )
            .map_err(sqlite_err)?;
        }
        tx.execute("DELETE FROM chunks WHERE rel_path = ?1", params![rel])
            .map_err(sqlite_err)?;
        tx.execute("DELETE FROM symbols WHERE rel_path = ?1", params![rel])
            .map_err(sqlite_err)?;
        tx.execute("DELETE FROM refs WHERE rel_path = ?1", params![rel])
            .map_err(sqlite_err)?;
        call_graph::rebuild_names(&tx, dirty_calls).map_err(sqlite_err)?;
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
