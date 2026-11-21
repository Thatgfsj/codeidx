//! sqlite-vec 向量层（Issue #5）：vec0 虚表初始化、写入侧 chunk 哈希
//! 缓存、KNN 检索与 RRF 混合排序。
//!
//! 集成方式（探针 /tmp/phase2-probe 路径 A 实证）：sqlite-vec 0.1.9 只
//! 导出 C 符号 `sqlite3_vec_init`（build.rs 以 SQLITE_CORE 静态编译），
//! 经 `rusqlite::ffi::sqlite3_auto_extension` 在进程启动早期注册——进程
//! 级全局一次，之后每个 `:memory:` 连接都直接具备 vec0 虚表能力。

use std::collections::HashMap;
use std::sync::Once;

use codeidx_protocol::{SearchHit, ToolError};
use rusqlite::{params, Connection};

use crate::embedder::Embedder;
use crate::store::{poisoned, sqlite_err, Store};
use crate::tokens;

/// RRF 融合常数（探针实验四：k=60 与 k=1000 结果一致；k=1 会放大双路
/// 命中导致名次翻转；取常规值 60）。
const RRF_K: f64 = 60.0;

/// vec0 虚表 DDL：显式主键列 `chunk_id` 与 `chunks.id` 对齐。
///
/// 维度 `dim` 来自 `Embedder::dim()`（CODEIDX_EMBED_MODEL 切换模型时
/// 自适应：bge-small-zh 512 / bge-large-zh 1024，不固定 512）。
pub(crate) fn vec_table_ddl(dim: usize) -> String {
    format!(
        "CREATE VIRTUAL TABLE vec_chunks USING vec0(chunk_id integer primary key, embedding float[{dim}])"
    )
}

/// 进程级一次注册 sqlite3_vec 扩展（`Once` 保证幂等）。
///
/// 必须先于任何期望 vec0 可用的 Connection 打开；`Store::new` 在
/// enable_vector 时统一调用（任务书第一步要求：注册放 store 内部幂等 init）。
#[allow(clippy::missing_transmute_annotations)]
pub(crate) fn ensure_vec_registered() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    });
}

/// 在内存连接上建 vec0 虚表（首次注入 embedder 时由 Store::set_embedder
/// 调用，维度取自嵌入器声明）。
pub(crate) fn init_vec_table(conn: &Connection, dim: usize) -> Result<(), rusqlite::Error> {
    conn.execute_batch(vec_table_ddl(dim).as_str())
}

/// f32 向量 → 小端字节 blob（vec0 存储格式，探针路径 A 实证）。
pub(crate) fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut b = Vec::with_capacity(v.len() * 4);
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b
}

/// blob → f32 向量（小端解析）。
pub(crate) fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// cosine 相似度（[-1, 1]，越高越相关；零向量安全返回 0）。
pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    dot / (na * nb + f32::EPSILON)
}

/// 复用键：同 rel_path 下 (symbol, 行区间, content_hash) 相同即视为
/// 同一 chunk（F2.4 chunk 哈希缓存的判定依据）。
pub(crate) type ReuseKey = (Option<String>, u32, u32, String);

/// 事务删除前读出旧 chunk 的向量（可复用集）。
///
/// 查询失败（表不存在等异常态）返回空 map：安全降级为全量重 embed，
/// 绝不阻塞 replace_file 主流程。
pub(crate) fn read_reusable_vectors(
    conn: &Connection,
    rel_path: &str,
) -> HashMap<ReuseKey, Vec<u8>> {
    let mut out = HashMap::new();
    let Ok(mut stmt) = conn.prepare(
        "SELECT c.symbol, c.start_line, c.end_line, c.content_hash, v.embedding
         FROM chunks c JOIN vec_chunks v ON v.chunk_id = c.id
         WHERE c.rel_path = ?1",
    ) else {
        return out;
    };
    let Ok(rows) = stmt.query_map(params![rel_path], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, u32>(1)?,
            row.get::<_, u32>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Vec<u8>>(4)?,
        ))
    }) else {
        return out;
    };
    for row in rows.flatten() {
        let (symbol, start, end, hash, blob) = row;
        out.insert((symbol, start, end, hash), blob);
    }
    out
}

/// 批量嵌入待写向量（事务提交后调用：失败仅 stderr 警告，FTS 已入库；
/// 缺失向量的 chunk 下次 replace_file 因哈希匹配仍会重新嵌入，自愈）。
pub(crate) fn write_pending_vectors(
    conn: &Connection,
    embedder: &dyn Embedder,
    pending: &[(i64, String)],
) {
    if pending.is_empty() {
        return;
    }
    let texts: Vec<String> = pending.iter().map(|(_, t)| t.clone()).collect();
    match embedder.embed_batch(&texts) {
        Ok(vectors) => {
            for ((chunk_id, _), v) in pending.iter().zip(vectors.iter()) {
                if v.len() != embedder.dim() {
                    tracing::warn!(
                        chunk_id,
                        got = v.len(),
                        want = embedder.dim(),
                        "embedding dim mismatch; skipping vector row"
                    );
                    continue;
                }
                if let Err(err) = conn.execute(
                    "INSERT INTO vec_chunks(chunk_id, embedding) VALUES (?1, ?2)",
                    params![chunk_id, vec_to_blob(v)],
                ) {
                    tracing::warn!(chunk_id, error = %err, "vector row insert failed");
                }
            }
        }
        Err(err) => {
            tracing::warn!(
                chunks = pending.len(),
                error = %err,
                "embed failed; chunks indexed without vectors (self-heals on next replace)"
            );
        }
    }
}

/// "vector search disabled (Phase 2)" 统一错误文案（F2.5 断言锚点；
/// enable_vector=false、未注入 embedder、CODEIDX_DISABLE_VECTOR 同走此路径）。
pub(crate) fn disabled_err() -> ToolError {
    ToolError::new("vector search disabled (Phase 2)")
}

impl Store {
    /// 向量检索：query 嵌入 → vec0 KNN → 候选读回向量算 cosine 重排 →
    /// SearchHit（score 即 cosine，越高越相关）。
    ///
    /// 无 embedder / 向量未启用 → 统一 "vector search disabled (Phase 2)"。
    /// 空 / 纯标点 query 与 top_k=0 与 keyword 路语义一致返回空结果。
    pub fn search_vector(&self, query: &str, top_k: usize) -> Result<Vec<SearchHit>, ToolError> {
        Ok(self
            .search_vector_hits(query, top_k)?
            .into_iter()
            .map(|(_, hit)| hit)
            .collect())
    }

    /// search_vector 的内部形态：额外携带 chunk_id（RRF 融合的精确标识）。
    pub(crate) fn search_vector_hits(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<(i64, SearchHit)>, ToolError> {
        if !self.enable_vector {
            return Err(disabled_err());
        }
        let Some(embedder) = self.embedder_snapshot() else {
            return Err(disabled_err());
        };
        if top_k == 0 {
            return Ok(Vec::new());
        }
        // 空 / 纯标点 query：与 keyword 路一致直接空结果（不浪费 embed 调用）。
        if tokens::match_expr(query).is_none() {
            return Ok(Vec::new());
        }
        // query 嵌入在锁外完成（无 conn 依赖；真模型单条 2-7ms）。
        let qv = embedder
            .embed_batch(&[query.to_string()])
            .map_err(|e| ToolError::new(format!("embed query failed: {e}")))?
            .into_iter()
            .next()
            .ok_or_else(|| ToolError::new("embed query failed: empty result"))?;

        let conn = self.conn.lock().map_err(|_| poisoned())?;
        // KNN 两步走：先 vec0 MATCH 拿候选（虚表专用查询计划，不做 JOIN），
        // 再逐候选取 chunk 元数据 + 向量算 cosine（score 精确且不依赖
        // 归一化假设；归一化时 L2 候选序与 cosine 序一致，探针已证）。
        let mut knn = conn
            .prepare(
                "SELECT chunk_id, distance FROM vec_chunks
                 WHERE embedding MATCH ?1 AND k = ?2",
            )
            .map_err(sqlite_err)?;
        let candidates: Vec<(i64, f64)> = knn
            .query_map(params![vec_to_blob(&qv), top_k as i64], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })
            .map_err(sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;
        drop(knn);

        let mut hits: Vec<(i64, SearchHit)> = Vec::with_capacity(candidates.len());
        for (chunk_id, _distance) in candidates {
            let row = conn.query_row(
                "SELECT v.embedding, c.rel_path, c.symbol, c.start_line, c.end_line, c.text
                 FROM vec_chunks v JOIN chunks c ON c.id = v.chunk_id
                 WHERE v.chunk_id = ?1",
                params![chunk_id],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, u32>(3)?,
                        row.get::<_, u32>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            );
            let Ok((blob, rel_path, symbol, start_line, end_line, text)) = row else {
                continue; // chunk 行已删（并发 remove）；跳过候选。
            };
            let score = cosine(&qv, &blob_to_vec(&blob));
            hits.push((
                chunk_id,
                SearchHit {
                    file: rel_path,
                    symbol,
                    start_line,
                    end_line,
                    score: score as f64,
                    snippet: tokens::snippet_of(&text, 3),
                },
            ));
        }
        // 按 score 降序（DTO 契约）；cosine 平局按 file/line 稳定序。
        hits.sort_by(|a, b| {
            b.1.score
                .partial_cmp(&a.1.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.file.cmp(&b.1.file))
                .then_with(|| a.1.start_line.cmp(&b.1.start_line))
        });
        hits.truncate(top_k);
        Ok(hits)
    }

    /// 混合检索：keyword 与 vector 各取宽 top_k → RRF(k=60) 融合 →
    /// 降序截 top_k（score 即 RRF 值）。
    ///
    /// vector 路任何失败（未启用 / 无 embedder / 单次 embed 异常）降级为
    /// 纯 keyword 结果 + stderr 警告（F2.5 降级语义在 store 层落实）；
    /// keyword 路失败直接上抛（FTS 是不可降级的基础能力）。
    pub fn search_hybrid(&self, query: &str, top_k: usize) -> Result<Vec<SearchHit>, ToolError> {
        let kw = self.search_keyword_hits(query, top_k)?;
        let vec_hits = match self.search_vector_hits(query, top_k) {
            Ok(hits) => hits,
            Err(err) => {
                tracing::warn!(
                    error = %err.message,
                    "vector path unavailable; hybrid degrades to keyword-only"
                );
                return Ok(kw.into_iter().map(|(_, hit)| hit).collect());
            }
        };
        Ok(rrf_fuse(kw, vec_hits, top_k))
    }
}

/// Reciprocal Rank Fusion：两路 1-based 名次融合分 =
/// Σ 1/(RRF_K + rank)，双路都命中者两项相加；降序截 top_k。
/// 平局按 (file, start_line) 稳定排序保证确定性。
pub(crate) fn rrf_fuse(
    kw: Vec<(i64, SearchHit)>,
    vec: Vec<(i64, SearchHit)>,
    top_k: usize,
) -> Vec<SearchHit> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for (rank, (id, _)) in kw.iter().enumerate() {
        *scores.entry(*id).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
    }
    for (rank, (id, _)) in vec.iter().enumerate() {
        *scores.entry(*id).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
    }
    // chunk 元数据任取一路（两路同源于 chunks 表，内容一致）。
    let mut meta: HashMap<i64, SearchHit> = HashMap::new();
    for (id, hit) in kw.into_iter().chain(vec) {
        meta.entry(id).or_insert(hit);
    }
    let mut fused: Vec<SearchHit> = meta
        .into_iter()
        .map(|(id, mut hit)| {
            hit.score = scores[&id]; // RRF 值即 DTO score（任务书契约）。
            hit
        })
        .collect();
    fused.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
    fused.truncate(top_k);
    fused
}
