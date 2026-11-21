//! mount 引擎：walker 遍历 → rayon 并行解析 → 全量入库。
//!
//! 单文件失败（读取 / NUL / 超 512KB / 入库）一律 tracing 警告后跳过，
//! 不中断整库挂载（ARCHITECTURE §5.5）。

use std::collections::HashMap;
use std::path::Path;

use rayon::prelude::*;

use crate::manager::{vm_rss_kb, FileRecord, ProjectContext};
use crate::walker;
use codeidx_ast::ParsedFile;
use codeidx_protocol::ToolError;
use codeidx_store::Store;

/// build_context 的构建产物。
pub(crate) struct BuiltContext {
    pub context: ProjectContext,
    /// store.stats().symbols（MountReport 的符号数）。
    pub symbols: u64,
    /// 挂载前后 /proc/self/status VmRSS 差值（MB）；读取失败为 0.0。
    pub mem_est_mb: f64,
}

/// 全量构建一个项目的索引上下文。
///
/// 流程：VmRSS 采样 → walker 收集 → rayon 并行加载（读+哈希+解析）→
/// Store 建库并逐文件 replace_file（单文件失败跳过）→ 汇总。
pub(crate) fn build_context(root: &Path, enable_vector: bool) -> Result<BuiltContext, ToolError> {
    let rss_before = vm_rss_kb();
    let sources = walker::collect(root);
    // 并行阶段：纯 CPU / IO 工作（读文件 + blake3 + tree-sitter）。
    let loaded = sources
        .par_iter()
        .filter_map(walker::load_file)
        .collect::<Vec<_>>();

    let store = Store::new(enable_vector)?;
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
    })
}
