//! sync 引擎：三分类（added / removed / modified）增量重建。
//!
//! 快筛规则：mtime（秒）或 size 任一变化 → 变更候选；候选读文件做
//! blake3 复核，内容哈希确实变化才算 modified（仅 touch / mtime 精度
//! 抖动不计），随后单文件事务 replace_file 并同步 parsed / files 表。
//!
//! 产物语义硬化：部分失败真实列出——读取失败 / 二进制 / 超限 / 入库
//! 失败的文件计入 failed 并列出（failed_files 上限 20，超出置
//! failed_files_truncated=true）。

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;

use crate::manager::ProjectContext;
use crate::walker::{self, SourceFile};
use codeidx_protocol::tools::SyncReport;

/// failed_files 列名上限（超出的文件仍计入 failed 总数）。
const FAILED_FILES_MAX: usize = 20;

/// 追加失败文件名；超上限置 truncated 标记。
fn push_failed(list: &mut Vec<String>, truncated: &mut bool, rel: String) {
    if list.len() < FAILED_FILES_MAX {
        list.push(rel);
    } else {
        *truncated = true;
    }
}

/// 对已挂载上下文执行一次增量同步。
pub(crate) fn reconcile(
    ctx: &mut ProjectContext,
    present: Vec<SourceFile>,
    started: Instant,
) -> SyncReport {
    reconcile_changes(ctx, present, started, |_| false)
}

/// 监听点名的路径也需要复核内容，覆盖同秒同大小的原子替换。
pub(crate) fn reconcile_changes(
    ctx: &mut ProjectContext,
    present: Vec<SourceFile>,
    started: Instant,
    changed: impl Fn(&Path) -> bool,
) -> SyncReport {
    // ---- 三分类（纯内存比对，无 IO）----
    let present_map: HashMap<&str, &SourceFile> = present
        .iter()
        .map(|sf| (sf.rel_path.as_str(), sf))
        .collect();

    let mut added: Vec<&SourceFile> = Vec::new();
    let mut candidates: Vec<&SourceFile> = Vec::new();
    for sf in &present {
        match ctx.files.get(&sf.rel_path) {
            None => added.push(sf),
            Some(rec) => {
                if rec.mtime != sf.mtime || rec.size != sf.size || changed(&sf.abs) {
                    candidates.push(sf);
                }
            }
        }
    }
    let removed: Vec<String> = ctx
        .files
        .keys()
        .filter(|rel| !present_map.contains_key(rel.as_str()))
        .cloned()
        .collect();

    // ---- 部分失败记账（产物语义硬化）----
    let mut failed: u64 = 0;
    let mut failed_files: Vec<String> = Vec::new();
    let mut failed_truncated = false;

    // ---- 删除先行（单文件事务）----
    let mut removed_n = 0u64;
    for rel in &removed {
        if let Err(err) = ctx.store.remove_file(rel) {
            tracing::warn!(rel = %rel, error = %err.message, "remove_file failed; keeping record");
            failed += 1;
            push_failed(&mut failed_files, &mut failed_truncated, rel.clone());
            continue;
        }
        ctx.files.remove(rel);
        ctx.parsed.remove(rel);
        removed_n += 1;
    }

    // ---- 新增 + 变更候选：rayon 并行加载（读 → blake3 → 解析一把梭）----
    /// 单文件加载结果：成功携带解析产物，失败携带跳过原因。
    type LoadOutcome<'a> =
        Result<(&'a SourceFile, walker::LoadedFile), (&'a SourceFile, walker::LoadSkip)>;
    let to_load: Vec<&SourceFile> = added.iter().chain(candidates.iter()).copied().collect();
    let loaded: Vec<LoadOutcome<'_>> = to_load
        .par_iter()
        .map(|sf| {
            walker::load_file(sf)
                .map(|lf| (*sf, lf))
                .map_err(|e| (*sf, e))
        })
        .collect();

    let mut added_n = 0u64;
    let mut modified_n = 0u64;
    for item in loaded {
        let (sf, lf) = match item {
            Ok(pair) => pair,
            Err((sf, _skip)) => {
                // 读取失败已在 load_file 内警告；保留旧记录，失败真实列出。
                failed += 1;
                push_failed(
                    &mut failed_files,
                    &mut failed_truncated,
                    sf.rel_path.clone(),
                );
                continue;
            }
        };
        let is_new = !ctx.files.contains_key(&sf.rel_path);
        // blake3 复核：mtime/size 变但内容未变（touch）→ 只刷新记录，不重入库。
        if let Some(rec) = ctx.files.get(&sf.rel_path) {
            if rec.hash == lf.record.hash {
                ctx.files.insert(sf.rel_path.clone(), lf.record);
                continue;
            }
        }
        if let Err(err) = ctx.store.replace_file_with_calls(&lf.parsed, &lf.calls) {
            tracing::warn!(
                rel = %sf.rel_path,
                error = %err.message,
                "replace_file failed; keeping old index"
            );
            failed += 1;
            push_failed(
                &mut failed_files,
                &mut failed_truncated,
                sf.rel_path.clone(),
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

    ctx.callflow_skipped += failed as usize;
    SyncReport {
        added: added_n,
        modified: modified_n,
        removed: removed_n,
        files: ctx.files.len() as u64,
        elapsed_ms: started.elapsed().as_millis() as u64,
        failed,
        failed_files,
        failed_files_truncated: failed_truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::FileRecord;
    use codeidx_ast::Lang;
    use codeidx_store::Store;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let dst = std::env::temp_dir().join(format!(
            "codeidx-sync-test-{tag}-{}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&dst).expect("create tempdir");
        dst
    }

    /// 手工构造上下文（不依赖 mount）。
    fn make_ctx() -> ProjectContext {
        ProjectContext {
            root: PathBuf::from("/tmp/ctx"),
            store: Store::new(false).expect("store"),
            parsed: HashMap::new(),
            files: HashMap::new(),
            callflow_skipped: 0,
            vector_degraded: false,
            vector_degraded_reason: String::new(),
        }
    }

    fn source_of(root: &Path, name: &str, mtime: u64, size: u64) -> SourceFile {
        SourceFile {
            rel_path: name.to_string(),
            abs: root.join(name),
            mtime,
            size,
            lang: Lang::Rust,
        }
    }

    /// 变更候选读取失败（含 NUL）→ failed=1、failed_files 列出、
    /// 旧记录保留。
    #[test]
    fn reconcile_lists_binary_failure_in_failed_files() {
        let root = temp_dir("failed");
        let mut nul = b"pub fn gone_binary() -> u32 { 0 }".to_vec();
        nul.push(0u8);
        fs::write(root.join("a.rs"), &nul).expect("write nul a.rs");

        let mut ctx = make_ctx();
        ctx.files.insert(
            "a.rs".to_string(),
            FileRecord {
                mtime: 1,
                size: 4,
                hash: "old".to_string(),
            },
        );
        // 磁盘版本 mtime/size 已变 → 变更候选 → 读取报 Binary。
        let present = vec![source_of(&root, "a.rs", 2, 40)];
        let report = reconcile(&mut ctx, present, Instant::now());

        assert_eq!(report.added, 0);
        assert_eq!(report.modified, 0);
        assert_eq!(report.removed, 0);
        assert_eq!(report.failed, 1, "binary candidate counted");
        assert_eq!(report.failed_files, vec!["a.rs".to_string()]);
        assert!(!report.failed_files_truncated);
        // 旧记录保留（失败不清索引）。
        assert!(ctx.files.contains_key("a.rs"));
        fs::remove_dir_all(&root).ok();
    }

    /// 失败文件超过 20 条：failed 计全量、failed_files 截断到 20、
    /// failed_files_truncated=true。
    #[test]
    fn reconcile_truncates_failed_files_beyond_twenty() {
        let root = temp_dir("truncate");
        let mut nul = b"pub fn x() -> u32 { 0 }".to_vec();
        nul.push(0u8);
        fs::write(root.join("nul.rs"), &nul).expect("write shared nul source");

        let mut ctx = make_ctx();
        let mut present = Vec::new();
        for i in 0..25 {
            let name = format!("f{i:02}.rs");
            ctx.files.insert(
                name.clone(),
                FileRecord {
                    mtime: 1,
                    size: 4,
                    hash: "old".to_string(),
                },
            );
            let mut sf = source_of(&root, "nul.rs", 2, 40);
            sf.rel_path = name.clone();
            present.push(sf);
        }
        let report = reconcile(&mut ctx, present, Instant::now());
        assert_eq!(report.failed, 25, "failed counts all 25");
        assert_eq!(report.failed_files.len(), 20, "list capped at 20");
        assert!(report.failed_files_truncated);
        fs::remove_dir_all(&root).ok();
    }

    /// 无失败时 failed 字段零值但恒出现。
    #[test]
    fn reconcile_zero_failures_reported_honestly() {
        let root = temp_dir("clean");
        fs::write(root.join("ok.rs"), "pub fn fine() -> u32 { 1 }").expect("write");

        let mut ctx = make_ctx();
        let present = vec![source_of(&root, "ok.rs", 1, 30)];
        let report = reconcile(&mut ctx, present, Instant::now());
        assert_eq!(report.failed, 0);
        assert!(report.failed_files.is_empty());
        assert!(!report.failed_files_truncated);
        assert_eq!(report.added, 1);
        fs::remove_dir_all(&root).ok();
    }
}
