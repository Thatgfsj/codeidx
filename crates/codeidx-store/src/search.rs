//! 检索：FTS5 BM25 关键词检索（vector / hybrid 路见 vec.rs，Issue #5）。

use codeidx_protocol::{SearchHit, ToolError};
use rusqlite::params;

use crate::store::{poisoned, sqlite_err};
use crate::tokens;
use crate::Store;

/// 精确标识符提升：完整 query 原样出现在块内（或 symbol 名等于 query）视为
/// 强相关，得分加该常量置顶——抵消 bm25 长度归一化对短块的偏置
/// （已知问题：3 行 use 兜底块在 OR 拆词下压过符号定义块，见 F1.7）。
const EXACT_QUERY_BOOST: f64 = 10.0;

impl Store {
    /// FTS5 BM25 检索，按分数降序返回 top_k。
    ///
    /// 查询拆词（与写入侧 tokens 列同一规则），词间 OR（优先召回）；
    /// 空 query / 纯标点 / top_k 为 0 返回空结果。结果从 chunks 反查
    /// file / symbol / 行号，snippet 取剥头后前 3 行。bm25() 返回负值
    /// 且越相关越负，取负后 score 与"越高越相关"的 DTO 语义对齐。
    pub fn search_keyword(&self, query: &str, top_k: usize) -> Result<Vec<SearchHit>, ToolError> {
        Ok(self
            .search_keyword_hits(query, top_k)?
            .into_iter()
            .map(|(_, hit)| hit)
            .collect())
    }

    /// search_keyword 的内部形态：额外携带 chunk_id（RRF 融合的精确标识，
    /// 比 (file, symbol, 行号) 组合键更稳：同文件同键冲突不可能出现）。
    pub(crate) fn search_keyword_hits(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<(i64, SearchHit)>, ToolError> {
        if top_k == 0 {
            return Ok(Vec::new());
        }
        let Some(expr) = tokens::match_expr(query) else {
            return Ok(Vec::new());
        };
        let conn = self.conn.lock().map_err(|_| poisoned())?;
        // 取件放宽到 max(64, top_k*4)：精确标识符提升（EXACT_QUERY_BOOST）
        // 在取件后进行，若按 bm25 原序直接 LIMIT top_k，高价值定义块可能
        // 因短块偏置被截在窗口外；放宽后重排再截回 top_k。
        let fetch = (top_k.saturating_mul(4)).max(64) as i64;
        let mut stmt = conn
            .prepare(
                "SELECT rowid, bm25(fts) FROM fts
                 WHERE fts MATCH ?1
                 ORDER BY bm25(fts) ASC
                 LIMIT ?2",
            )
            .map_err(sqlite_err)?;
        let scored = stmt
            .query_map(params![expr, fetch], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(sqlite_err)?;
        let mut raw = Vec::new();
        for pair in scored {
            raw.push(pair.map_err(sqlite_err)?);
        }
        drop(stmt);
        let mut hits: Vec<(i64, SearchHit)> = Vec::with_capacity(raw.len());
        for (chunk_id, bm) in raw {
            let (rel_path, symbol, start_line, end_line, text) = conn
                .query_row(
                    "SELECT rel_path, symbol, start_line, end_line, text FROM chunks
                     WHERE id = ?1",
                    params![chunk_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, u32>(2)?,
                            row.get::<_, u32>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .map_err(sqlite_err)?;
            // 精确标识符提升（见 EXACT_QUERY_BOOST）：仅当块所属 symbol 名
            // 精确等于完整 query（即 query 本身是符号名、该块是其定义体）时
            // 提升；引用/兜底块不提升，保持结果按 score 降序。
            let exact = symbol.as_deref() == Some(query);
            hits.push((
                chunk_id,
                SearchHit {
                    file: rel_path,
                    symbol,
                    start_line,
                    end_line,
                    score: -bm + if exact { EXACT_QUERY_BOOST } else { 0.0 },
                    snippet: tokens::snippet_of(&text, 3),
                },
            ));
        }
        // 按 score 降序输出（DTO 契约），并截回请求的 top_k。
        hits.sort_by(|a, b| {
            b.1.score
                .partial_cmp(&a.1.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(top_k);
        Ok(hits)
    }
}
