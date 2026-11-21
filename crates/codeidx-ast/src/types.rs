//! 冻结的核心数据类型契约（全队唯一事实来源）。
//!
//! 本文件由 Phase 1-A 落盘并冻结：B/C/D 队友的 walker、索引器、检索器
//! 与 RepoMap 全部以这里的类型为边界。字段的增删改名必须经 Lead 审批。
//!
//! 行号约定（对齐 ARCHITECTURE §4）：对外一律 1-based line、
//! 0-based character；本文件中的 `start_line`/`end_line` 均为
//! 1-based 且含端点（闭区间）。

use serde::{Deserialize, Serialize};

/// 支持的编程语言。
///
/// 由语言驱动依据文件扩展名推断；JavaScript 复用 TypeScript 的
/// TSX 语法变体解析。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Lang {
    /// Rust（tree-sitter-rust）。
    Rust,
    /// Python（tree-sitter-python）。
    Python,
    /// TypeScript（tree-sitter-typescript，TypeScript 变体）。
    TypeScript,
    /// JavaScript（tree-sitter-typescript，TSX 变体解析 `.js`）。
    JavaScript,
    /// Go（tree-sitter-go）。
    Go,
    /// Java（tree-sitter-java）。
    Java,
    /// C（tree-sitter-c）。
    C,
    /// C++（tree-sitter-cpp）。
    Cpp,
}

/// 符号类别。
///
/// 序列化为变体名本身（如 `"Fn"`、`"Struct"`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    /// 自由函数。
    Fn,
    /// impl 块 / 类体内的方法。
    Method,
    /// Rust struct。
    Struct,
    /// 类（Python/TS/Java 等面向对象语言）。
    Class,
    /// Rust trait。
    Trait,
    /// 接口（TS/Java/Go）。
    Interface,
    /// 枚举。
    Enum,
    /// 常量。
    Const,
    /// 类型别名。
    Type,
    /// 模块。
    Module,
    /// 其余无法归类的定义。
    Other,
}

/// 源码符号定义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolDef {
    /// 符号类别。
    pub kind: SymbolKind,
    /// 符号名。
    pub name: String,
    /// 相对项目根的路径（`/` 分隔）。
    pub rel_path: String,
    /// 定义起始行（1-based，含）。
    pub start_line: u32,
    /// 定义结束行（1-based，含）。
    pub end_line: u32,
    /// 归一化签名（省 token 的单行表示）。
    pub signature: String,
    /// 所属容器符号名（如方法所属类 / impl 宿主 struct），无容器时为 `None`。
    pub parent: Option<String>,
}

/// 对符号的一次引用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolRef {
    /// 被引用的符号名。
    pub name: String,
    /// 引用所在文件的相对路径。
    pub rel_path: String,
    /// 引用所在行（1-based）。
    pub line: u32,
}

/// 检索 / 向量化的最小单元：代码块。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    /// 所属文件相对路径。
    pub rel_path: String,
    /// 所属符号名（文件级 / 无符号块为 `None`）。
    pub symbol: Option<String>,
    /// 块起始行（1-based，含）。
    pub start_line: u32,
    /// 块结束行（1-based，含）。
    pub end_line: u32,
    /// 内容哈希（blake3，十六进制）。
    pub content_hash: String,
    /// 块文本（已注入路径 / 符号上下文）。
    pub text: String,
}

/// 单个文件的完整解析产物。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedFile {
    /// 文件相对路径。
    pub rel_path: String,
    /// 推断出的语言。
    pub lang: Lang,
    /// 从该文件提取的符号定义。
    pub symbols: Vec<SymbolDef>,
    /// 从该文件提取的符号引用。
    pub refs: Vec<SymbolRef>,
    /// 该文件切出的代码块。
    pub chunks: Vec<Chunk>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 契约类型必须 serde round-trip 不变形（字段名即对外 JSON 形态）。
    #[test]
    fn frozen_types_round_trip() {
        let file = ParsedFile {
            rel_path: "src/lib.rs".into(),
            lang: Lang::Rust,
            symbols: vec![SymbolDef {
                kind: SymbolKind::Fn,
                name: "unicorn_marker_fn".into(),
                rel_path: "src/lib.rs".into(),
                start_line: 3,
                end_line: 9,
                signature: "fn unicorn_marker_fn(cfg: &GoldConfig) -> String".into(),
                parent: None,
            }],
            refs: vec![SymbolRef {
                name: "helper_spinner".into(),
                rel_path: "src/lib.rs".into(),
                line: 5,
            }],
            chunks: vec![Chunk {
                rel_path: "src/lib.rs".into(),
                symbol: Some("unicorn_marker_fn".into()),
                start_line: 3,
                end_line: 9,
                content_hash: "0f42a3".into(),
                text: "pub fn unicorn_marker_fn(...)".into(),
            }],
        };

        let json = serde_json::to_string(&file).unwrap();
        let back: ParsedFile = serde_json::from_str(&json).unwrap();
        assert_eq!(file, back);
    }

    /// 对外 JSON 形态断言：字段名与契约逐字一致。
    #[test]
    fn frozen_types_json_shape() {
        let sym = SymbolDef {
            kind: SymbolKind::Method,
            name: "forge".into(),
            rel_path: "src/lib.rs".into(),
            start_line: 10,
            end_line: 13,
            signature: "pub fn forge(label: &str) -> GoldConfig".into(),
            parent: Some("GoldConfig".into()),
        };
        let value: serde_json::Value = serde_json::to_value(&sym).unwrap();
        let expected = serde_json::json!({
            "kind": "Method",
            "name": "forge",
            "rel_path": "src/lib.rs",
            "start_line": 10,
            "end_line": 13,
            "signature": "pub fn forge(label: &str) -> GoldConfig",
            "parent": "GoldConfig",
        });
        assert_eq!(value, expected);
    }
}
