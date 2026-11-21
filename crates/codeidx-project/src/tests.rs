//! codeidx-project 单元测试（Phase 1-B）。
//!
//! 全部用 tests/fixtures/sample-project 真实挂载；需要改文件的用例
//! 先把 fixture 复制到 tempdir，不污染仓库。测试各自持有独立
//! ProjectManager 实例，天然并行安全。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::walker;
use crate::ProjectManager;

/// fixture 根目录（仓库内只读迷你仓：3 rust + 2 py + 1 ts + __pycache__）。
fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/sample-project")
        // canonicalize：与 MountReport.path 的 canonical 形态对齐
        .canonicalize()
        .expect("fixture dir exists")
}

/// 递归复制目录（fixture → tempdir 工作副本）。
fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create tempdir");
    for entry in fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("fixture entry");
        let ty = entry.file_type().expect("fixture entry type");
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).expect("copy fixture file");
        }
    }
}

/// 唯一 tempdir 名计数器：测试并行时副本互不冲突。
static COPY_SEQ: AtomicU64 = AtomicU64::new(0);

/// fixture 的独立 tempdir 副本（可自由修改）。
fn temp_project(tag: &str) -> PathBuf {
    let seq = COPY_SEQ.fetch_add(1, Ordering::SeqCst);
    let dst = std::env::temp_dir().join(format!(
        "codeidx-project-test-{tag}-{}-{seq}",
        std::process::id()
    ));
    copy_dir(&fixture_root(), &dst);
    dst
}

/// mount 后 files==6（3 rust + 2 py + 1 ts）、symbols>=10、parsed 含 6 键。
#[test]
fn mount_indexes_six_source_files() {
    let mgr = ProjectManager::new();
    let root = fixture_root();
    let report = mgr.mount(&root, false).expect("mount fixture");
    assert!(report.mounted);
    assert_eq!(report.files, 6, "3 rust + 2 py + 1 ts");
    assert!(report.symbols >= 10, "symbols = {}", report.symbols);
    assert!(!report.vector);
    assert_eq!(report.path, root.to_string_lossy().into_owned());

    mgr.with_project(&root, |ctx| {
        assert_eq!(ctx.parsed.len(), 6, "parsed cache holds all files");
        assert_eq!(ctx.files.len(), 6);
        for rel in [
            "src/lib.rs",
            "src/main.rs",
            "src/utils.rs",
            "app.py",
            "utils.py",
            "index.ts",
        ] {
            assert!(ctx.files.contains_key(rel), "missing {rel}");
        }
        // __pycache__ 黑名单 + Cargo.toml/README.md 非源文件，一并不可见。
        assert!(!ctx.files.keys().any(|k| k.contains("__pycache__")));
        assert!(!ctx.files.contains_key("Cargo.toml"));
        assert_eq!(ctx.store.stats().files, 6);
        assert!(ctx.store.stats().symbols >= 10);
        Ok(())
    })
    .expect("with_project after mount");

    mgr.unmount(&root).expect("cleanup unmount");
}

/// 重复 mount 同一路径幂等：两次 stats 一致、list 无重复条目。
#[test]
fn remount_same_path_is_idempotent() {
    let mgr = ProjectManager::new();
    let root = fixture_root();
    let first = mgr.mount(&root, false).expect("first mount");
    let second = mgr.mount(&root, false).expect("second mount");
    assert_eq!(first.files, second.files);
    assert_eq!(first.symbols, second.symbols);

    let list = mgr.list();
    assert_eq!(list.len(), 1, "no duplicate entries: {list:?}");
    assert_eq!(list[0].path, root.to_string_lossy().into_owned());
    assert_eq!(list[0].files, first.files);
    assert_eq!(list[0].symbols, first.symbols);
    assert!(list[0].lsps.is_empty(), "Phase 1 has no LSP processes");
    assert!(!list[0].vector);

    mgr.unmount(&root).expect("cleanup unmount");
}

/// 未挂载路径 with_project 报 "not mounted"（F1.11 文案）。
#[test]
fn with_project_requires_mount() {
    let mgr = ProjectManager::new();
    let missing = Path::new("/tmp/codeidx-definitely-not-mounted-xyz");
    let err = mgr
        .with_project(missing, |_| Ok(()))
        .expect_err("unmounted path must fail");
    assert!(err.message.contains("not mounted"), "{:?}", err.message);
    assert!(err.message.contains("codeidx-definitely-not-mounted-xyz"));
}

/// unmount 后再查报 "not mounted"、二次 unmount 报错、list 不含。
#[test]
fn unmount_releases_context() {
    let mgr = ProjectManager::new();
    let root = fixture_root();
    mgr.mount(&root, false).expect("mount");

    let report = mgr.unmount(&root).expect("unmount");
    assert!(report.unmounted);
    assert_eq!(report.killed_lsps, 0, "Phase 1 has no LSP to kill");

    let err = mgr.unmount(&root).expect_err("double unmount must fail");
    assert!(err.message.contains("not mounted"), "{:?}", err.message);
    let err = mgr
        .with_project(&root, |_| Ok(()))
        .expect_err("query after unmount must fail");
    assert!(err.message.contains("not mounted"), "{:?}", err.message);

    let listed = root.to_string_lossy().into_owned();
    assert!(
        !mgr.list().iter().any(|p| p.path == listed),
        "list must not contain unmounted root"
    );
}

/// sync 三分类：改 1 增 1 删 1 → 1/1/1，且 store 检索反映变更。
#[test]
fn sync_reconciles_add_modify_remove() {
    let mgr = ProjectManager::new();
    let root = temp_project("sync");
    mgr.mount(&root, false).expect("mount copy");

    // 改 1：src/lib.rs 追加函数（size 与内容都变）。
    let lib = root.join("src/lib.rs");
    let old = fs::read_to_string(&lib).expect("read lib.rs");
    fs::write(
        &lib,
        format!("{old}pub fn edited_fn_added_later() -> u32 {{ 7 }}"),
    )
    .expect("append to lib.rs");
    // 增 1：全新文件。
    fs::write(
        root.join("src/extra.rs"),
        "pub fn brand_new_fn() -> u32 { 42 }",
    )
    .expect("write extra.rs");
    // 删 1：移除 app.py。
    fs::remove_file(root.join("app.py")).expect("remove app.py");
    // 让秒级 mtime 也翻转（快筛本就命中 size 变化，此处只为稳定）。
    std::thread::sleep(Duration::from_millis(1100));

    let report = mgr.sync(&root).expect("sync");
    assert_eq!(report.added, 1, "added: {report:?}");
    assert_eq!(report.modified, 1, "modified: {report:?}");
    assert_eq!(report.removed, 1, "removed: {report:?}");
    assert_eq!(report.files, 6, "6 + 1 - 1 = 6");

    mgr.with_project(&root, |ctx| {
        // 新文件已入库可检索。
        let hits = ctx
            .store
            .search_keyword("brand_new_fn", 8)
            .expect("search new");
        assert!(
            hits.iter().any(|h| h.file == "src/extra.rs"),
            "extra.rs must be searchable: {:?}",
            hits.iter().map(|h| h.file.clone()).collect::<Vec<_>>()
        );
        // 修改后的内容已生效。
        let hits = ctx
            .store
            .search_keyword("edited_fn_added_later", 8)
            .expect("search edited");
        assert!(!hits.is_empty(), "edited content must be indexed");
        // 删除的文件不再可检索：helper_spinner 仍有命中（utils.py 等），
        // 但任何命中都不应来自 app.py。
        let hits = ctx
            .store
            .search_keyword("helper_spinner", 8)
            .expect("search helper");
        assert!(!hits.is_empty(), "utils.py still indexed");
        assert!(
            hits.iter().all(|h| h.file != "app.py"),
            "app.py content must be gone"
        );
        assert!(!ctx.files.contains_key("app.py"));
        assert!(ctx.files.contains_key("src/extra.rs"));
        assert_eq!(ctx.files.len(), 6);
        Ok(())
    })
    .expect("with_project after sync");

    // 无变更的二次 sync 全零（幂等）。
    let again = mgr.sync(&root).expect("second sync");
    assert_eq!((again.added, again.modified, again.removed), (0, 0, 0));

    mgr.unmount(&root).expect("cleanup unmount");
    fs::remove_dir_all(&root).ok();
}

/// walker 过滤：node_modules / .git 黑名单、600KB 大文件、含 NUL 二进制。
#[test]
fn walker_skips_blacklist_large_and_binary_files() {
    let mgr = ProjectManager::new();
    let root = temp_project("walker");

    // 黑名单目录里的源文件。
    fs::create_dir_all(root.join("node_modules")).expect("mkdir node_modules");
    fs::write(
        root.join("node_modules/x.rs"),
        "pub fn blacklisted_dep_fn() -> u32 { 1 }",
    )
    .expect("write node_modules/x.rs");
    fs::create_dir_all(root.join(".git")).expect("mkdir .git");
    fs::write(root.join(".git/config"), "[core]").expect("write .git/config");
    // 600KB 大文件（> 512KB 防线）。
    let big_body = "// padding".repeat(60_000); // 600KB
    let big = format!("pub fn big_file_fn() -> u32 {{ 1 }} {big_body}");
    fs::write(root.join("big.rs"), &big).expect("write big.rs");
    // 含 NUL 的二进制（扩展名命中 lang，靠 NUL 防线跳过）。
    let mut nul = b"pub fn nul_file_fn() -> u32 { 0 }".to_vec();
    nul.push(0u8);
    nul.extend_from_slice(b"binary-junk");
    fs::write(root.join("nul.rs"), &nul).expect("write nul.rs");

    let report = mgr.mount(&root, false).expect("mount polluted copy");
    assert_eq!(
        report.files, 6,
        "only the 6 fixture sources; got {}",
        report.files
    );

    mgr.with_project(&root, |ctx| {
        for banned in ["node_modules/x.rs", ".git/config", "big.rs", "nul.rs"] {
            assert!(!ctx.files.contains_key(banned), "{banned} must be skipped");
        }
        // 检索词取各污染文件的独有词：拆词 OR 匹配下 "fn" 这类公共词
        // 会让正常块命中、断言失真。
        for needle in ["blacklisted", "big", "junk"] {
            let hits = ctx.store.search_keyword(needle, 8).expect("search");
            assert!(
                hits.is_empty(),
                "{needle} must not be indexed: {:?}",
                hits.iter().map(|h| h.file.clone()).collect::<Vec<_>>()
            );
        }
        Ok(())
    })
    .expect("with_project after polluted mount");

    mgr.unmount(&root).expect("cleanup unmount");
    fs::remove_dir_all(&root).ok();
}

/// mount 不存在路径报 "path not found"。
#[test]
fn mount_missing_path_reports_not_found() {
    let mgr = ProjectManager::new();
    let missing = Path::new("/tmp/codeidx-definitely-missing-root-xyz");
    let err = mgr.mount(missing, false).expect_err("mount missing path");
    assert!(err.message.contains("path not found"), "{:?}", err.message);
    assert!(err.message.contains("codeidx-definitely-missing-root-xyz"));
    assert!(mgr.list().is_empty());
}

/// 未挂载路径 sync 报 "not mounted"。
#[test]
fn sync_requires_mount() {
    let mgr = ProjectManager::new();
    let missing = Path::new("/tmp/codeidx-never-mounted-xyz");
    let err = mgr.sync(missing).expect_err("sync unmounted");
    assert!(err.message.contains("not mounted"), "{:?}", err.message);
}

/// walker 遵循项目根 .gitignore。
#[test]
fn collect_honors_gitignore() {
    let root = temp_project("gitignore");
    fs::write(root.join(".gitignore"), "ignored.rs").expect("write .gitignore");
    fs::write(root.join("ignored.rs"), "pub fn ignored_fn() -> u32 { 0 }")
        .expect("write ignored.rs");

    let collected = walker::collect(&root);
    assert!(
        !collected.iter().any(|s| s.rel_path == "ignored.rs"),
        "gitignored file must not be collected"
    );
    assert!(collected.iter().any(|s| s.rel_path == "src/lib.rs"));
    // 元数据完整：mtime 与 size 均有值。
    let lib = collected
        .iter()
        .find(|s| s.rel_path == "src/lib.rs")
        .unwrap();
    assert!(lib.size > 0);
    assert!(lib.mtime > 0);

    fs::remove_dir_all(&root).ok();
}
