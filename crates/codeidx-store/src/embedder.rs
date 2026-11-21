//! Embedder 抽象与 fastembed 生产实现（Issue #5；ARCHITECTURE §3 / §5.7）。
//!
//! - `Embedder` trait 是向量层唯一入口：`replace_file` 的写入缓存与
//!   `search_vector` / `search_hybrid` 的查询嵌入都只面向该抽象，
//!   单测注入确定性 MockEmbedder（tests.rs），不碰真模型；
//! - `FastembedEmbedder` 为生产实现：BAAI/bge-small-zh-v1.5（512 维，
//!   输出已归一化），进程内 `OnceLock` 懒加载全局单例，初始化失败
//!   返回 `Err` 交由上层降级（F2.1），绝不 panic；
//! - 模型缓存目录固定 `~/.cache/codeidx-mcp`（HOME 下全局；绝不落在
//!   用户项目目录，ARCHITECTURE §1 "目标项目目录不残留任何缓存"）。
//!   不读 `FASTEMBED_CACHE_DIR` / `HF_HOME`，保证目录约束可静态审计。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

/// bge-small-zh-v1.5 的向量维度（`vec_chunks` 表 `float[512]` 与之对齐）。
pub const EMBED_DIM: usize = 512;

/// 向量嵌入抽象（实现方必须 `Send + Sync`：Store 跨线程共享）。
///
/// 约定：实现输出归一化向量（L2 范数 ≈ 1）。vec0 虚表的默认 L2 KNN
/// 与 cosine 排序此时单调一致；fastembed 的 bge 系列满足该约定
/// （探针实测 L2 范数 = 1.000000）。
pub trait Embedder: Send + Sync {
    /// 批量嵌入文本：返回向量数与 `texts` 等长、维度等于 `dim()`。
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
    /// 向量维度。
    fn dim(&self) -> usize;
}

/// 生产实现：fastembed + BAAI/bge-small-zh-v1.5（经 Xenova ONNX 版）。
///
/// fastembed 的 `embed()` 要求 `&mut self`，内部 `Mutex` 满足 trait 的
/// `&self` 签名；onnxruntime 会话本身线程安全（静态链接，见探针画像）。
pub struct FastembedEmbedder {
    model: Mutex<TextEmbedding>,
}

/// ONNX intra-op 线程数上限。
///
/// dogfooding 实测：默认线程数=物理核数时，多线程各自的分配 arena 与
/// memory-pattern 缓存会显著放大常驻内存（配合 main.rs 的 M_ARENA_MAX
/// 与本模块的长度分桶策略）；bge-small-zh 是轻量小模型，4 线程足够。
const ONNX_INTRA_THREADS: usize = 4;

impl FastembedEmbedder {
    /// 以显式缓存目录初始化模型（首次约数百 ms 级缓存命中；空缓存时
    /// 触发 90.5MB ONNX 下载，由调用方决定时机——见 mount 懒加载）。
    pub fn new() -> Result<Self, String> {
        let opts = TextInitOptions::new(EmbeddingModel::BGESmallZHV15)
            .with_cache_dir(cache_dir())
            .with_intra_threads(ONNX_INTRA_THREADS);
        let model = TextEmbedding::try_new(opts)
            .map_err(|e| format!("fastembed init failed (model BAAI/bge-small-zh-v1.5): {e}"))?;
        Ok(FastembedEmbedder {
            model: Mutex::new(model),
        })
    }

    /// 惰性全局单例（ARCHITECTURE §5.7：进程内单例复用）。
    ///
    /// 初始化失败同样被 `OnceLock` 缓存：本进程内不再重试（失败多为
    /// 环境性，重试只会重复数十秒级开销；每次 mount 拿到同一份 `Err`
    /// 并降级为纯 FTS5）。
    pub fn global() -> Result<std::sync::Arc<FastembedEmbedder>, String> {
        static GLOBAL: OnceLock<Result<std::sync::Arc<FastembedEmbedder>, String>> =
            OnceLock::new();
        GLOBAL
            .get_or_init(|| FastembedEmbedder::new().map(std::sync::Arc::new))
            .clone()
    }
}

/// 我们层的固定批大小：所有 embed 调用形状统一为 (≤16, ≤prefix token)，
/// 避免多样 batch 形状各自触发一份 ONNX memory-pattern arena 缓存
/// （dogfooding 实测多样形状使 RSS 膨胀 ~3 倍并拖慢 plan 编译）。
const EMBED_FIXED_BATCH: usize = 16;

/// 嵌入文本前缀截断（≈512 token，与 tokenizer 的 max_length 截断语义
/// 等价：反正超长部分会被丢弃，提前截让批内 pad 长度统一）。
const EMBED_TEXT_PREFIX_CHARS: usize = 1600;

impl Embedder for FastembedEmbedder {
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut model = self
            .model
            .lock()
            .map_err(|_| "fastembed model mutex poisoned".to_string())?;
        let truncated: Vec<String> = texts
            .iter()
            .map(|t| {
                if t.chars().count() <= EMBED_TEXT_PREFIX_CHARS {
                    t.clone()
                } else {
                    t.chars().take(EMBED_TEXT_PREFIX_CHARS).collect()
                }
            })
            .collect();
        // 长度分桶：按文本长度排序后切批，批内 pad 长度相近，ONNX
        // memory-pattern 形状数从「每文件一种」收敛到「每桶一种」，
        // 避免逐批重新编译执行计划（dogfooding 实测该项占挂载耗时大头）。
        let mut order: Vec<usize> = (0..truncated.len()).collect();
        order.sort_by_key(|&i| truncated[i].chars().count());
        let mut all: Vec<Option<Vec<f32>>> = vec![None; truncated.len()];
        for chunk in order.chunks(EMBED_FIXED_BATCH) {
            let batch: Vec<String> = chunk.iter().map(|&i| truncated[i].clone()).collect();
            let vecs = model
                .embed(batch, None)
                .map_err(|e| format!("fastembed embed failed: {e}"))?;
            for (&i, v) in chunk.iter().zip(vecs) {
                all[i] = Some(v);
            }
        }
        let all: Result<Vec<Vec<f32>>, String> = all
            .into_iter()
            .map(|v| v.ok_or_else(|| "fastembed batch index desync".to_string()))
            .collect();
        all
    }

    fn dim(&self) -> usize {
        EMBED_DIM
    }
}

/// 模型缓存目录：`~/.cache/codeidx-mcp`（HOME 下全局）。
///
/// HOME 缺失（极端环境）回退系统 temp 目录，避免落到进程 CWD 的
/// `.fastembed_cache`（fastembed 默认行为，违反目录约束）。
fn cache_dir() -> PathBuf {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => PathBuf::from(home).join(".cache/codeidx-mcp"),
        _ => std::env::temp_dir().join("codeidx-mcp-model-cache"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 缓存目录必须位于 HOME 之下（不落用户项目目录的静态审计锚点）。
    #[test]
    fn cache_dir_is_home_global() {
        let dir = cache_dir();
        let shown = dir.display().to_string();
        assert!(
            shown.ends_with(".cache/codeidx-mcp") || shown.contains("codeidx-mcp-model-cache"),
            "unexpected cache dir: {shown}"
        );
        assert!(
            !shown.starts_with(".fastembed_cache"),
            "绝不允许落到 CWD 默认缓存"
        );
    }
}
