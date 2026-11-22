//! walker：gitignore 感知的项目源文件收集与单文件加载。
//!
//! mount / sync 共用本模块：
//! - `collect` 遍历项目根，产出待索引源文件清单（携带 mtime/size 快筛
//!   元数据）与两类跳过计数（unsupported / oversize）；
//! - `load_file` 读取单个文件，完成 NUL / 大小防线检查、blake3 哈希与
//!   tree-sitter 解析，产出 `ParsedFile` + `FileRecord` 供入库；跳过
//!   原因以 `LoadSkip` 变体区分（产物语义硬化：binary 计数与
//!   SyncReport.failed_files 都依赖该区分）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use codeidx_ast::{detect_lang, parse_file_with_calls, CallSite, Lang, ParsedFile};
use ignore::WalkBuilder;

use crate::manager::FileRecord;

/// 永久黑名单目录名：无论 .gitignore 与否一律剪枝（ARCHITECTURE §5.4）。
const BLACKLIST_DIRS: [&str; 9] = [
    ".git",
    "node_modules",
    "target",
    "build",
    "dist",
    ".next",
    "venv",
    "__pycache__",
    ".agent-index",
];

/// 大文件防线：超过该字节数的源文件不进入索引（ARCHITECTURE §5.4）。
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// walker 收集到的待索引源文件（快筛元数据随行携带）。
#[derive(Debug, Clone)]
pub(crate) struct SourceFile {
    /// 相对项目根路径（`/` 分隔）。
    pub rel_path: String,
    /// 绝对路径。
    pub abs: PathBuf,
    /// mtime（UNIX epoch 秒）。
    pub mtime: u64,
    /// 字节长度。
    pub size: u64,
    /// 扩展名推断出的语言。
    pub lang: Lang,
}

/// 单文件加载产物：解析结果 + 索引记录。
#[derive(Debug)]
pub(crate) struct LoadedFile {
    pub parsed: ParsedFile,
    pub calls: Vec<CallSite>,
    pub record: FileRecord,
}

/// 单文件加载的跳过原因（三类防线 + 读失败，调用方据此计数 / 列名）。
#[derive(Debug)]
pub(crate) enum LoadSkip {
    /// 含 NUL 字节的二进制文件。
    Binary,
    /// 超过 512KB 防线（collect 后文件增长的竞态复核）。
    Oversize,
    /// 读取失败（权限 / 消失等）；详情已在 load_file 内 stderr 告警，
    /// 不属 walker 三类跳过计数。
    Read,
}

/// walker 遍历结果 + 跳过计数（产物语义硬化：MountReport 固定字段数据源）。
#[derive(Debug, Default)]
pub(crate) struct WalkOutcome {
    /// 待索引源文件（按 rel_path 排序保证确定性）。
    pub files: Vec<SourceFile>,
    /// detect_lang 未命中的文件数（.md / .toml 等非受支持语言文件）。
    pub skipped_unsupported: u64,
    /// 超 512KB 跳过的文件数。
    pub skipped_oversize: u64,
}

/// 遍历项目根，收集全部可索引源文件（按 rel_path 排序保证确定性）。
///
/// 薄壳：仅取文件清单、丢弃计数（sync / 既有单测的兼容入口）。
#[cfg(test)]
pub(crate) fn collect(root: &Path) -> Vec<SourceFile> {
    collect_with_counts(root).files
}

/// collect 的完整形态：文件清单 + 两类跳过计数（mount 的 MountReport
/// 固定字段数据源）。
///
/// 防线逐层收紧：
/// 1. ignore 遍历遵循 .gitignore / .ignore（显式开启，行为对齐 ripgrep；
///    require_git(false) 使非 git 仓库同样生效）；
/// 2. 内置黑名单目录整棵剪枝（filter_entry，根节点豁免；黑名单目录不属
///    "文件级跳过"，不计数）；
/// 3. 仅接受 detect_lang 命中的源文件，未命中计入 skipped_unsupported；
/// 4. > 512KB 跳过计入 skipped_oversize；NUL 二进制由 `load_file` 读取时报 `LoadSkip::Binary`。
pub(crate) fn collect_with_counts(root: &Path) -> WalkOutcome {
    let mut builder = WalkBuilder::new(root);
    // 显式开启 ignore 语义（不依赖 crate 默认值，行为对齐 ripgrep）：
    // 遵循项目内 .gitignore / .ignore 及父目录链中的同名文件。
    // require_git(false)：非 git 仓库目录同样遵循 .gitignore（ignore
    // 0.4.22+ 默认 require_git=true，无 .git 时会跳过 gitignore 处理）。
    builder
        .git_ignore(true)
        .ignore(true)
        .git_exclude(true)
        .parents(true)
        .require_git(false);
    builder.filter_entry(|entry| {
        if entry.depth() == 0 {
            // 根节点即使名字命中黑名单也必须保留（例如挂载名为 build 的目录）。
            return true;
        }
        let name = entry.file_name().to_string_lossy();
        !BLACKLIST_DIRS.contains(&name.as_ref())
    });

    let mut out = WalkOutcome::default();
    for entry in builder.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(error = %err, "walk entry failed; skipping");
                continue;
            }
        };
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!(path = %entry.path().display(), error = %err, "metadata failed");
                continue;
            }
        };
        if !meta.is_file() {
            continue;
        }
        let Some(lang) = detect_lang(&entry.path().to_string_lossy()) else {
            out.skipped_unsupported += 1;
            continue;
        };
        let size = meta.len();
        if size > MAX_FILE_BYTES {
            tracing::debug!(path = %entry.path().display(), "exceeds 512KB guard; skipping");
            out.skipped_oversize += 1;
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let Some(rel_path) = rel_path_of(root, entry.path()) else {
            continue;
        };
        out.files.push(SourceFile {
            rel_path,
            abs: entry.into_path(),
            mtime,
            size,
            lang,
        });
    }
    out.files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out
}

/// 相对项目根的路径（`/` 分隔）；根自身返回 None。
fn rel_path_of(root: &Path, abs: &Path) -> Option<String> {
    let rel = abs.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for comp in rel.components() {
        parts.push(comp.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// 读取并解析单个源文件：内容 blake3 哈希 + tree-sitter 解析。
///
/// 读失败（LoadSkip::Read）/ 含 NUL（LoadSkip::Binary）/ 超 512KB
/// （LoadSkip::Oversize）由调用方按语义计数或列名；绝不 panic。
pub(crate) fn load_file(sf: &SourceFile) -> Result<LoadedFile, LoadSkip> {
    let bytes = match fs::read(&sf.abs) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(rel = %sf.rel_path, error = %err, "read failed; skipping file");
            return Err(LoadSkip::Read);
        }
    };
    if bytes.contains(&0u8) {
        tracing::debug!(rel = %sf.rel_path, "NUL byte detected; skipping binary");
        return Err(LoadSkip::Binary);
    }
    if bytes.len() as u64 > MAX_FILE_BYTES {
        tracing::debug!(rel = %sf.rel_path, "file exceeds 512KB guard; skipping");
        return Err(LoadSkip::Oversize);
    }
    let text = String::from_utf8_lossy(&bytes);
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let (parsed, calls) = parse_file_with_calls(sf.lang, &sf.rel_path, &text);
    let record = FileRecord {
        mtime: sf.mtime,
        size: sf.size,
        hash,
    };
    Ok(LoadedFile {
        parsed,
        calls,
        record,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    /// 独立 tempdir（测试并行不冲突）。
    fn temp_dir(tag: &str) -> PathBuf {
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let dst = std::env::temp_dir().join(format!(
            "codeidx-walker-test-{tag}-{}-{seq}",
            std::process::id()
        ));
        fs::create_dir_all(&dst).expect("create tempdir");
        dst
    }

    /// 手工 SourceFile（绕过 collect，专测 load_file 的防线分支）。
    fn source_of(root: &Path, name: &str, lang: Lang) -> SourceFile {
        let abs = root.join(name);
        let meta = fs::metadata(&abs).expect("metadata");
        SourceFile {
            rel_path: name.to_string(),
            abs,
            mtime: 1,
            size: meta.len(),
            lang,
        }
    }

    /// skipped_unsupported / skipped_oversize 两类计数在 collect 层生效；
    /// NUL 二进制由 load_file 层报 Binary（见 load_file_rejects_binary）。
    #[test]
    fn collect_counts_unsupported_and_oversize() {
        let root = temp_dir("counts");
        fs::write(root.join("keep.rs"), "pub fn a() -> u32 { 1 }").expect("write");
        // 非受支持语言：README.md。
        fs::write(root.join("README.md"), "docs").expect("write");
        // 超 512KB 源文件（540KB > 524288B 防线）。
        let big = format!("pub fn big() -> u32 {{ 1 }} {}", "// pad".repeat(90_000));
        fs::write(root.join("big.rs"), &big).expect("write");

        let outcome = collect_with_counts(&root);
        assert_eq!(outcome.files.len(), 1, "only keep.rs");
        assert_eq!(outcome.files[0].rel_path, "keep.rs");
        assert_eq!(outcome.skipped_unsupported, 1, ".md counted");
        assert_eq!(outcome.skipped_oversize, 1, "600KB counted");
        fs::remove_dir_all(&root).ok();
    }

    /// collect 对黑名单目录整棵剪枝且不计入三类跳过计数。
    #[test]
    fn collect_prunes_blacklist_without_counting() {
        let root = temp_dir("blacklist");
        fs::create_dir_all(root.join("node_modules")).expect("mkdir");
        fs::write(root.join("node_modules/x.rs"), "pub fn dep() -> u32 { 2 }").expect("write");
        fs::write(root.join("app.rs"), "pub fn mine() -> u32 { 3 }").expect("write");

        let outcome = collect_with_counts(&root);
        assert_eq!(outcome.files.len(), 1);
        assert_eq!(outcome.skipped_unsupported, 0);
        assert_eq!(outcome.skipped_oversize, 0);
        fs::remove_dir_all(&root).ok();
    }

    /// load_file 三种跳过变体：Binary（NUL）/ Oversize / Read（不存在路径）。
    #[test]
    fn load_file_rejects_binary_oversize_and_read_failure() {
        let root = temp_dir("loadskip");
        // Binary：含 NUL 的 .rs。
        let mut nul = b"pub fn n() -> u32 { 0 }".to_vec();
        nul.push(0u8);
        fs::write(root.join("nul.rs"), &nul).expect("write nul.rs");
        // Oversize：collect 后手工构造 SourceFile 绕过 metadata 预检，
        // 验证 load_file 的读取级竞态复核。
        let big = format!("pub fn b() -> u32 {{ 9 }} {}", "// pad".repeat(90_000));
        fs::write(root.join("big.rs"), &big).expect("write big.rs");
        // Read：指向不存在的路径。
        fs::write(root.join("ok.rs"), "pub fn ok() -> u32 { 1 }").expect("write ok.rs");

        let nul_sf = source_of(&root, "nul.rs", Lang::Rust);
        assert!(matches!(load_file(&nul_sf), Err(LoadSkip::Binary)));
        let big_sf = source_of(&root, "big.rs", Lang::Rust);
        assert!(matches!(load_file(&big_sf), Err(LoadSkip::Oversize)));
        let mut ghost = source_of(&root, "ok.rs", Lang::Rust);
        ghost.abs = root.join("ghost.rs");
        assert!(matches!(load_file(&ghost), Err(LoadSkip::Read)));

        let ok_sf = source_of(&root, "ok.rs", Lang::Rust);
        let loaded = load_file(&ok_sf).expect("normal file loads");
        assert_eq!(loaded.parsed.rel_path, "ok.rs");
        assert_eq!(loaded.record.hash.len(), 64, "blake3 hex");
        fs::remove_dir_all(&root).ok();
    }
}
