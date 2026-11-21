//! 语言驱动：扩展名推断、tree-sitter grammar 加载与行注释前缀。
//!
//! `detect_lang` 是对外的唯一语言入口；grammar 加载使用 tree-sitter 0.27
//! 的 `LANGUAGE` 常量（经冒烟验证的版本组，见根 Cargo.toml）。

use tree_sitter::Language;

use crate::types::Lang;

/// 按文件扩展名推断语言。
///
/// 覆盖 `.rs/.py/.ts/.tsx/.js/.jsx/.mjs/.go/.java/.c/.h/.cpp/.cc/.hpp/.dart`；
/// 其余扩展名返回 `None`。JavaScript 复用 TypeScript 的 TSX 变体解析。
pub fn detect_lang(path: &str) -> Option<Lang> {
    let ext = path.rsplit('.').next()?;
    if path == ext {
        // 没有点号或点号在末尾之外：rsplit 取到的是整段路径而非扩展名
        return None;
    }
    match ext {
        "rs" => Some(Lang::Rust),
        "py" => Some(Lang::Python),
        "ts" | "tsx" => Some(Lang::TypeScript),
        "js" | "jsx" | "mjs" => Some(Lang::JavaScript),
        "go" => Some(Lang::Go),
        "java" => Some(Lang::Java),
        "c" | "h" => Some(Lang::C),
        "cpp" | "cc" | "hpp" => Some(Lang::Cpp),
        "dart" => Some(Lang::Dart),
        _ => None,
    }
}

/// 加载语言对应的 tree-sitter grammar。
pub fn language_of(lang: Lang) -> Language {
    match lang {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        // JS 用 TSX 变体：可同时容忍 JSX 与纯脚本语法。
        Lang::JavaScript => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
        Lang::Java => tree_sitter_java::LANGUAGE.into(),
        Lang::C => tree_sitter_c::LANGUAGE.into(),
        Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        Lang::Dart => tree_sitter_dart::LANGUAGE.into(),
        // 四门扩展语言（Kotlin/CSharp/Php/Html）的 grammar 由并行语言包
        // 任务交付（teammates a/d）；交付前 detect_lang 尚未映射对应扩展
        // 名，本分支运行时不可达，交付时逐个替换为真实 grammar。
        Lang::Kotlin | Lang::CSharp | Lang::Php | Lang::Html => {
            unreachable!("extractor for {lang:?} not yet delivered")
        }
    }
}

/// 该语言的行注释前缀（用于 chunk 上下文注入）。
pub fn comment_prefix(lang: Lang) -> &'static str {
    match lang {
        Lang::Python => "#",
        _ => "//",
    }
}

/// 解析器使用的保护上限：超过该字节数的文本跳过 AST 解析
/// （ARCHITECTURE §5.4 大文件保护；调用方还有一道过滤）。
pub(crate) const MAX_PARSE_BYTES: usize = 512 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Lang;

    #[test]
    fn detect_lang_covers_all_extensions() {
        assert_eq!(detect_lang("a.rs"), Some(Lang::Rust));
        assert_eq!(detect_lang("a.py"), Some(Lang::Python));
        assert_eq!(detect_lang("a.ts"), Some(Lang::TypeScript));
        assert_eq!(detect_lang("a.tsx"), Some(Lang::TypeScript));
        assert_eq!(detect_lang("a.js"), Some(Lang::JavaScript));
        assert_eq!(detect_lang("a.jsx"), Some(Lang::JavaScript));
        assert_eq!(detect_lang("a.mjs"), Some(Lang::JavaScript));
        assert_eq!(detect_lang("a.go"), Some(Lang::Go));
        assert_eq!(detect_lang("a.java"), Some(Lang::Java));
        assert_eq!(detect_lang("a.c"), Some(Lang::C));
        assert_eq!(detect_lang("a.h"), Some(Lang::C));
        assert_eq!(detect_lang("a.cpp"), Some(Lang::Cpp));
        assert_eq!(detect_lang("a.cc"), Some(Lang::Cpp));
        assert_eq!(detect_lang("a.hpp"), Some(Lang::Cpp));
        assert_eq!(detect_lang("a.dart"), Some(Lang::Dart));
        assert_eq!(detect_lang("/x/y/main.dart"), Some(Lang::Dart));
    }

    #[test]
    fn detect_lang_rejects_unknown_and_edge_cases() {
        assert_eq!(detect_lang("README.md"), None);
        assert_eq!(detect_lang("noext"), None);
        assert_eq!(detect_lang("a."), None); // rsplit('.') 得空扩展名
        assert_eq!(detect_lang("weird.RS"), None);
        assert_eq!(detect_lang("/x/y/script.mjs"), Some(Lang::JavaScript));
    }

    #[test]
    fn comment_prefix_matches_language_convention() {
        assert_eq!(comment_prefix(Lang::Python), "#");
        assert_eq!(comment_prefix(Lang::Rust), "//");
        assert_eq!(comment_prefix(Lang::TypeScript), "//");
        assert_eq!(comment_prefix(Lang::JavaScript), "//");
        assert_eq!(comment_prefix(Lang::Go), "//");
        assert_eq!(comment_prefix(Lang::Java), "//");
        assert_eq!(comment_prefix(Lang::C), "//");
        assert_eq!(comment_prefix(Lang::Cpp), "//");
    }

    #[test]
    fn grammars_all_load() {
        for lang in [
            Lang::Rust,
            Lang::Python,
            Lang::TypeScript,
            Lang::JavaScript,
            Lang::Go,
            Lang::Java,
            Lang::C,
            Lang::Cpp,
            Lang::Dart,
        ] {
            let mut p = tree_sitter::Parser::new();
            p.set_language(&language_of(lang)).expect("grammar load");
        }
    }
}
