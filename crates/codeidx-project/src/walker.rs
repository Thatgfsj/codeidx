//! walker：gitignore 感知的项目源文件收集与单文件加载。
//!
//! mount / sync 共用本模块：
//! - `collect` 遍历项目根，产出待索引源文件清单（携带 mtime/size 快筛元数据）；
//! - `load_file` 读取单个文件，完成 NUL / 大小防线检查、blake3 哈希与
//!   tree-sitter 解析，产出 `ParsedFile` + `FileRecord` 供入库。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use codeidx_ast::{detect_lang, parse_file, Lang, ParsedFile};
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
    pub record: FileRecord,
}

/// 遍历项目根，收集全部可索引源文件（按 rel_path 排序保证确定性）。
///
/// 防线逐层收紧：
/// 1. ignore 遍历遵循 .gitignore / .ignore（WalkBuilder 默认开启）；
/// 2. 内置黑名单目录整棵剪枝（filter_entry，根节点豁免）；
/// 3. 仅接受 detect_lang 命中的源文件；
/// 4. > 512KB 文件跳过；含 NUL 的二进制在 `load_file` 读取阶段跳过。
pub(crate) fn collect(root: &Path) -> Vec<SourceFile> {
    let mut builder = WalkBuilder::new(root);
    // 显式开启 ignore 语义（不依赖 crate 默认值，行为对齐 ripgrep）：
    // 遵循项目内 .gitignore / .ignore 及父目录链中的同名文件。
    // require_git(false)：非 git 仓库目录同样遵循 .gitignore（ignore 0.4.22+
    // 默认 require_git=true，无 .git 时会跳过 gitignore 处理）。
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

    let mut out = Vec::new();
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
            continue;
        };
        let size = meta.len();
        if size > MAX_FILE_BYTES {
            tracing::debug!(path = %entry.path().display(), "exceeds 512KB guard; skipping");
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
        out.push(SourceFile {
            rel_path,
            abs: entry.into_path(),
            mtime,
            size,
            lang,
        });
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
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
/// 读失败 / 含 NUL / 超 512KB → None（内部记 stderr 警告），
/// 由调用方跳过该文件；绝不 panic。
pub(crate) fn load_file(sf: &SourceFile) -> Option<LoadedFile> {
    let bytes = match fs::read(&sf.abs) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(rel = %sf.rel_path, error = %err, "read failed; skipping file");
            return None;
        }
    };
    if bytes.contains(&0u8) {
        tracing::debug!(rel = %sf.rel_path, "NUL byte detected; skipping binary");
        return None;
    }
    if bytes.len() as u64 > MAX_FILE_BYTES {
        tracing::debug!(rel = %sf.rel_path, "file exceeds 512KB guard; skipping");
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let parsed = parse_file(sf.lang, &sf.rel_path, &text);
    let record = FileRecord {
        mtime: sf.mtime,
        size: sf.size,
        hash,
    };
    Some(LoadedFile { parsed, record })
}
