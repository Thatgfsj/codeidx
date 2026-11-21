//! sync 引擎：三分类（added / removed / modified）增量重建。
//!
//! 快筛规则：mtime（秒）或 size 任一变化 → 变更候选；候选读文件做
//! blake3 复核，内容哈希确实变化才算 modified（仅 touch / mtime 精度
//! 抖动不计），随后单文件事务 replace_file 并同步 parsed / files 表。

use std::collections::HashMap;
use std::time::Instant;

use rayon::prelude::*;

use crate::manager::ProjectContext;
use crate::walker::{self, SourceFile};
use codeidx_protocol::tools::SyncReport;

/// 对已挂载上下文执行一次增量同步。
pub(crate) fn reconcile(
    ctx: &mut ProjectContext,
    present: Vec<SourceFile>,
    started: Instant,
) -> SyncReport {
    // ---- 三分类（纯内存比对，无 IO）----
    let present_map: HashMap<&str, &SourceFile> = present
        .iter()
        .map(|sf| (sf.rel_path.as_str(), sf))
        .collect();

    let mut added: Vec<&SourceFile> = Vec::new();
    let mut candidates: Vec<&SourceFile> = Vec::new(); // mtime/size 变更候选
    for sf in &present {
        match ctx.files.get(&sf.rel_path) {
            None => added.push(sf),
            Some(rec) => {
                if rec.mtime != sf.mtime || rec.size != sf.size {
                    candidates.push(sf);
                }
                // mtime/size 均未变 → 快筛通过，不做 blake3 复核（增量语义）。
            }
        }
    }
    let removed: Vec<String> = ctx
        .files
        .keys()
        .filter(|rel| !present_map.contains_key(rel.as_str()))
        .cloned()
        .collect();

    // ---- 删除先行（单文件事务）----
    let mut removed_n = 0u64;
    for rel in &removed {
        if let Err(err) = ctx.store.remove_file(rel) {
            tracing::warn!(rel = %rel, error = %err.message, "remove_file failed; keeping record");
            continue;
        }
        ctx.files.remove(rel);
        ctx.parsed.remove(rel);
        removed_n += 1;
    }

    // ---- 新增 + 变更候选：rayon 并行加载（读 → blake3 → 解析一把梭）----
    let to_load: Vec<&SourceFile> = added.iter().chain(candidates.iter()).copied().collect();
    let loaded: Vec<Option<(&SourceFile, walker::LoadedFile)>> = to_load
        .par_iter()
        .map(|sf| walker::load_file(sf).map(|lf| (*sf, lf)))
        .collect();

    let mut added_n = 0u64;
    let mut modified_n = 0u64;
    for item in loaded {
        let Some((sf, lf)) = item else {
            continue; // 读取失败已在 load_file 内警告；保留旧记录不动。
        };
        let is_new = !ctx.files.contains_key(&sf.rel_path);
        // blake3 复核：mtime/size 变但内容未变（touch）→ 只刷新记录，不重入库。
        if let Some(rec) = ctx.files.get(&sf.rel_path) {
            if rec.hash == lf.record.hash {
                ctx.files.insert(sf.rel_path.clone(), lf.record);
                continue;
            }
        }
        if let Err(err) = ctx.store.replace_file(&lf.parsed) {
            tracing::warn!(
                rel = %sf.rel_path,
                error = %err.message,
                "replace_file failed; keeping old index"
            );
            continue;
        }
        ctx.files.insert(sf.rel_path.clone(), lf.record);
        ctx.parsed.insert(sf.rel_path.clone(), lf.parsed);
        if is_new {
            added_n += 1;
        } else {
            modified_n += 1;
        }
    }

    SyncReport {
        added: added_n,
        modified: modified_n,
        removed: removed_n,
        files: ctx.files.len() as u64,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}
