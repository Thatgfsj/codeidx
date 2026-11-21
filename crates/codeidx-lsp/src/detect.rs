//! 语言服务器二进制探测（PATH 扫描）与 languageId 映射。

use std::env;
use std::path::Path;

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
pub(crate) fn detect_server(lang: Lang) -> Result<&'static ServerSpec, LspError> {
    for spec in server_candidates(lang) {
        if is_on_path(spec.name) {
            return Ok(spec);
        }
    }
    Err(LspError(format!(
        "language server for {} not found on PATH",
        language_id(lang)
    )))
}

/// PATH 扫描（与 `Command::new` 的 execvp 语义一致；空 PATH 组件 = cwd）。
fn is_on_path(binary: &str) -> bool {
    let Some(path_var) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path_var).any(|dir| executable(&dir.join(binary)))
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
