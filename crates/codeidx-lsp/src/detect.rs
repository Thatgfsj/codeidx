//! 语言服务器二进制探测（PATH 扫描）与 languageId 映射。

use std::env;
use std::ffi::OsStr;
use std::path::{self, Path, PathBuf};

use codeidx_ast::Lang;

use crate::proto::LspError;

/// 一个候选语言服务器：二进制名 + 固定启动参数。
pub(crate) struct ServerSpec {
    /// 二进制名（不带路径，从 PATH 解析）。
    pub(crate) name: &'static str,
    /// 固定启动参数（如 vtsls / typescript-language-server 的 `--stdio`）。
    pub(crate) args: &'static [&'static str],
}

impl ServerSpec {
    const fn new(name: &'static str, args: &'static [&'static str]) -> Self {
        Self { name, args }
    }
}

/// didOpen 用的 languageId（LSP 惯例值），同时用作错误文案里的语言短名。
pub(crate) fn language_id(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => "rust",
        Lang::Python => "python",
        Lang::TypeScript => "typescript",
        Lang::JavaScript => "javascript",
        Lang::Go => "go",
        Lang::Java => "java",
        Lang::C => "c",
        Lang::Cpp => "cpp",
        // 扩展语言包交付（teammates c/a/d）时补真实 languageId；
        // 交付前 lsp 路径不可达（detect_lang 未映射扩展名）。
        Lang::Dart => "dart",
        Lang::Kotlin => "kotlin",
        Lang::CSharp => "csharp",
        Lang::Php => "php",
        Lang::Html => "html",
    }
}

static SERVER_RUST: [ServerSpec; 1] = [ServerSpec::new("rust-analyzer", &[])];
static SERVER_TS: [ServerSpec; 2] = [
    ServerSpec::new("vtsls", &["--stdio"]),
    ServerSpec::new("typescript-language-server", &["--stdio"]),
];
/// Python：basedpyright / pyright 的 LSP 形态是各自的 `-langserver`
/// 入口（裸 CLI 是诊断工具、不 speak LSP）；两者都需要显式 `--stdio`
/// 参数（basedpyright 1.40 实测：缺参会以 "Connection input stream is
/// not set" 退出）。
static SERVER_PY: [ServerSpec; 2] = [
    ServerSpec::new("basedpyright-langserver", &["--stdio"]),
    ServerSpec::new("pyright-langserver", &["--stdio"]),
];
static SERVER_GO: [ServerSpec; 1] = [ServerSpec::new("gopls", &[])];
static SERVER_JAVA: [ServerSpec; 1] = [ServerSpec::new("jdtls", &[])];
static SERVER_C: [ServerSpec; 1] = [ServerSpec::new("clangd", &[])];

/// 占位空表：扩展语言包交付前的探测表（交付时替换为真实候选）。
static SERVER_NONE: [ServerSpec; 0] = [];
static SERVER_DART: [ServerSpec; 1] = [ServerSpec::new("dart", &["language-server"])];

/// 探测表：按候选顺序取第一个 PATH 命中的二进制。
pub(crate) fn server_candidates(lang: Lang) -> &'static [ServerSpec] {
    match lang {
        Lang::Rust => &SERVER_RUST,
        Lang::TypeScript | Lang::JavaScript => &SERVER_TS,
        Lang::Python => &SERVER_PY,
        Lang::Go => &SERVER_GO,
        Lang::Java => &SERVER_JAVA,
        Lang::C | Lang::Cpp => &SERVER_C,
        // 扩展语言的服务器探测表由语言包任务交付时补（c: dart /
        // a: csharp,php / d: kotlin；html 无 LSP 恒空表）；交付前 lsp
        // 路径不可达。dart 的真实探测名 dart language-server。
        Lang::Dart => &SERVER_DART,
        Lang::Kotlin => &SERVER_NONE,
        Lang::CSharp => &SERVER_NONE,
        Lang::Php => &SERVER_NONE,
        Lang::Html => &SERVER_NONE,
    }
}

/// 按 PATH 探测语言服务器；找不到时返回契约错误文案
/// `language server for <lang> not found on PATH`。
pub(crate) fn detect_server(lang: Lang) -> Result<(&'static ServerSpec, PathBuf), LspError> {
    if let Some(path) = env::var_os("PATH") {
        #[cfg(windows)]
        let extensions = env::var("PATHEXT").ok();
        #[cfg(not(windows))]
        let extensions: Option<String> = None;
        for spec in server_candidates(lang) {
            if let Some(program) = resolve_on_path(spec.name, &path, extensions.as_deref()) {
                return Ok((spec, program));
            }
        }
    }
    Err(LspError(format!(
        "language server for {} not found on PATH",
        language_id(lang)
    )))
}

/// 保留 PATH 顺序。使用绝对路径固定命中项，但不解析符号链接（如 rustup 代理）。
pub(crate) fn resolve_on_path(
    binary: &str,
    search_path: &OsStr,
    pathext: Option<&str>,
) -> Option<PathBuf> {
    #[cfg(windows)]
    let names: Vec<_> = windows_extensions(pathext)
        .map(|ext| format!("{binary}{ext}"))
        .collect();
    #[cfg(not(windows))]
    let names = {
        let _ = pathext;
        [binary]
    };

    env::split_paths(search_path).find_map(|dir| {
        // path::absolute 不接受空路径，但 PATH 空组件代表当前目录。
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            &dir
        };
        let dir = path::absolute(dir).ok()?;
        names
            .iter()
            .map(|name| dir.join(name))
            .find(|p| executable(p))
    })
}

#[cfg(any(windows, test))]
pub(crate) fn windows_extensions(pathext: Option<&str>) -> impl Iterator<Item = &'static str> + '_ {
    const ALLOWED: [&str; 4] = [".com", ".exe", ".bat", ".cmd"];
    pathext
        .filter(|exts| !exts.trim().is_empty())
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        // Rust Command 启动原生 .exe/.com，并处理 .bat/.cmd。其余文件关联不适用。
        .filter_map(|ext| {
            ALLOWED
                .into_iter()
                .find(|allowed| ext.trim().eq_ignore_ascii_case(allowed))
        })
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
