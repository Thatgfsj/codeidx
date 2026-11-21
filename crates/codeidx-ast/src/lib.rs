//! codeidx-ast —— 语言驱动、符号 / 引用提取与 PageRank RepoMap。
//!
//! 对外契约（Phase 1-C，B 队友按此调用）：
//! - detect_lang 按扩展名推断语言；
//! - parse_file 解析单文件，产出符号 / 引用 / 分块（纯 CPU 同步，
//!   调用方可用 rayon 并行；解析失败降级为空结果，绝不 panic）；
//! - repo_map 对符号 / 引用全集计算 PageRank 加权骨架图。
//!
//! 冻结的核心数据类型见 types 模块（Phase 1-A 契约，字段不可增删改名）。

pub mod types;

mod chunk;
mod extract;
mod lang;
mod pagerank;
mod repomap;

pub use lang::detect_lang;
pub use repomap::repo_map;
pub use types::{Chunk, Lang, ParsedFile, SymbolDef, SymbolKind, SymbolRef};

/// 解析单文件：提取符号、引用、分块。
///
/// 纯 CPU 同步函数；超大文本（> 512KB）与解析失败返回尽力而为的
/// 部分结果（空符号 / 引用），禁止 panic。
pub fn parse_file(lang: Lang, rel_path: &str, text: &str) -> ParsedFile {
    let (symbols, refs) = extract::extract(lang, rel_path, text);
    let chunks = chunk::chunk_file(lang, rel_path, text, &symbols);
    ParsedFile {
        rel_path: rel_path.to_string(),
        lang,
        symbols,
        refs,
        chunks,
    }
}
