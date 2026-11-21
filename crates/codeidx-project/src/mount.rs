//! mount 引擎：walker 遍历 → rayon 并行解析 → 全量入库。
//!
//! 单文件失败（读取 / NUL / 超 512KB / 入库）一律 tracing 警告后跳过，
//! 不中断整库挂载（ARCHITECTURE §5.5）。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rayon::prelude::*;

use crate::manager::{vm_rss_kb, FileRecord, ProjectContext};
use crate::walker;
use codeidx_ast::ParsedFile;
use codeidx_protocol::ToolError;
use codeidx_store::{Embedder, FastembedEmbedder, Store};

/// build_context 的构建产物。
pub(crate) struct BuiltContext {
    pub context: ProjectContext,
    /// store.stats().symbols（MountReport 的符号数）。
    pub symbols: u64,
    /// 挂载前后 /proc/self/status VmRSS 差值（MB）；读取失败为 0.0。
    pub mem_est_mb: f64,
    /// 真实生效的向量开关（降级后为 false，MountReport.vector 数据源）。
    pub vector: bool,
}

/// CODEIDX_DISABLE_VECTOR 的 truthy 判定（ARCHITECTURE §5.8）。
fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// mount 的真实向量开关与嵌入器注入（F2.1 / F2.5）：
/// - 请求 enable_vector=false → 纯 FTS5，不碰模型；
/// - CODEIDX_DISABLE_VECTOR truthy → 同上，绝不触碰模型单例；
/// - fastembed 全局单例初始化失败 → tracing 警告 + vector=false，
///   mount 继续（F2.1 降级语义）。
fn resolve_vector(enable_vector: bool) -> (bool, Option<Arc<dyn Embedder>>) {
    if !enable_vector {
        return (false, None);
    }
    if let Ok(v) = std::env::var("CODEIDX_DISABLE_VECTOR") {
        if is_truthy(&v) {
            tracing::warn!(
                value = %v,
                "CODEIDX_DISABLE_VECTOR set; vector disabled (F2.5)"
            );
            return (false, None);
        }
    }
    match FastembedEmbedder::global() {
        Ok(embedder) => (true, Some(embedder as Arc<dyn Embedder>)),
        Err(err) => {
            tracing::warn!(
                error = %err,
                "fastembed init failed; mounting with vector disabled (F2.1)"
            );
            (false, None)
        }
    }
}

/// 全量构建一个项目的索引上下文。
///
/// 流程：VmRSS 采样 → walker 收集 → rayon 并行加载（读+哈希+解析）→
/// Store 建库并逐文件 replace_file（单文件失败跳过）→ 汇总。
pub(crate) fn build_context(root: &Path, enable_vector: bool) -> Result<BuiltContext, ToolError> {
    // 真实向量开关（F2.1 / F2.5）：请求 + 环境变量 + 模型单例三重决定。
    let (vector, embedder) = resolve_vector(enable_vector);
    let rss_before = vm_rss_kb();
    let sources = walker::collect(root);
    // 并行阶段：纯 CPU / IO 工作（读文件 + blake3 + tree-sitter）。
    let loaded = sources
        .par_iter()
        .filter_map(walker::load_file)
        .collect::<Vec<_>>();

    let store = Store::new(vector)?;
    // 注入嵌入器（Store::new 建表后立即注入：逐文件 replace_file
    // 的向量写入依赖它；None = 纯 FTS5）。
    if let Some(e) = &embedder {
        store.set_embedder(Some(Arc::clone(e)));
    }
    let mut parsed: HashMap<String, ParsedFile> = HashMap::new();
    let mut files: HashMap<String, FileRecord> = HashMap::new();
    for lf in loaded {
        if let Err(err) = store.replace_file(&lf.parsed) {
            tracing::warn!(
                rel = %lf.parsed.rel_path,
                error = %err.message,
                "replace_file failed; skipping file"
            );
            continue;
        }
        files.insert(lf.parsed.rel_path.clone(), lf.record);
        parsed.insert(lf.parsed.rel_path.clone(), lf.parsed);
    }
    let symbols = store.stats().symbols;
    let mem_est_mb = match (rss_before, vm_rss_kb()) {
        (Some(before), Some(after)) => (after as f64 - before as f64) / 1024.0,
        _ => 0.0,
    };
    Ok(BuiltContext {
        context: ProjectContext {
            root: root.to_path_buf(),
            store,
            parsed,
            files,
        },
        symbols,
        mem_est_mb,
        vector,
    })
}
