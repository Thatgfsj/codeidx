//! 11 个 MCP 工具的输入 / 输出 DTO。
//!
//! - 输出 DTO 序列化为紧凑 JSON 文本，作为 tools/call 的 content
//!   （ARCHITECTURE §4 的省 token 约定），字段名与表格逐字一致；
//! - 参数命名同样蛇形：JSON 字段名 = Rust 字段名（serde 默认），
//!   以 ARCHITECTURE §4 契约表格为唯一事实来源；
//! - 行号字段一律 1-based line / 0-based character。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 工具名常量（路由表与日志共用；完整事实来源是 schema::TOOL_SPECS）。
pub mod tool_name {
    /// mount_project
    pub const MOUNT_PROJECT: &str = "mount_project";
    /// unmount_project
    pub const UNMOUNT_PROJECT: &str = "unmount_project";
    /// list_mounted_projects
    pub const LIST_MOUNTED_PROJECTS: &str = "list_mounted_projects";
    /// sync_project
    pub const SYNC_PROJECT: &str = "sync_project";
    /// get_repo_map
    pub const GET_REPO_MAP: &str = "get_repo_map";
    /// search_code
    pub const SEARCH_CODE: &str = "search_code";
    /// get_file_outline
    pub const GET_FILE_OUTLINE: &str = "get_file_outline";
    /// read_symbol
    pub const READ_SYMBOL: &str = "read_symbol";
    /// lsp_definition
    pub const LSP_DEFINITION: &str = "lsp_definition";
    /// lsp_references
    pub const LSP_REFERENCES: &str = "lsp_references";
    /// lsp_hover
    pub const LSP_HOVER: &str = "lsp_hover";
}

/// search_code 的检索模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchMode {
    /// FTS5 keyword + 向量 RRF 融合（向量不可用时降级 keyword）。
    #[serde(rename = "hybrid")]
    Hybrid,
    /// 纯 FTS5 keyword。
    #[serde(rename = "keyword")]
    Keyword,
    /// 纯向量。
    #[serde(rename = "vector")]
    Vector,
}

// ---------------------------------------------------------------- 输入 DTO --

/// mount_project 参数（JSON 字段名 = Rust 蛇形字段名，ARCHITECTURE §4）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MountParams {
    /// 项目根目录绝对路径。
    pub path: String,
    /// 是否启用向量检索（懒加载 embedding 模型）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_vector: Option<bool>,
}

/// 仅需 path 的工具参数（unmount_project / sync_project）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathParams {
    /// 已挂载项目的根目录绝对路径。
    pub path: String,
}

/// get_repo_map 参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoMapParams {
    /// 已挂载项目的根目录绝对路径。
    pub path: String,
    /// 聚焦文件（相对路径，其符号权重显著提升）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_files: Option<Vec<String>>,
    /// 输出 token 预算（默认 3000）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u32>,
}

/// search_code 参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchParams {
    /// 已挂载项目的根目录绝对路径。
    pub path: String,
    /// 检索词。
    pub query: String,
    /// 检索模式（默认 hybrid）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SearchMode>,
    /// 返回命中数上限（默认 8）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
}

/// get_file_outline 参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutlineParams {
    /// 已挂载项目的根目录绝对路径。
    pub path: String,
    /// 目标文件相对路径。
    pub file_path: String,
}

/// read_symbol 参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadSymbolParams {
    /// 已挂载项目的根目录绝对路径。
    pub path: String,
    /// 符号所在文件相对路径。
    pub file_path: String,
    /// 符号名。
    pub symbol_name: String,
}

/// lsp_definition / lsp_references / lsp_hover 的位置参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LspPositionParams {
    /// 已挂载项目的根目录绝对路径。
    pub path: String,
    /// 目标文件相对路径。
    pub file_path: String,
    /// 行号（1-based）。
    pub line: u32,
    /// 列号（0-based，UTF-16 code unit）。
    pub character: u32,
}

// ---------------------------------------------------------------- 输出 DTO --

/// mount_project 结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MountReport {
    /// 是否成功挂载。
    pub mounted: bool,
    /// 项目根路径。
    pub path: String,
    /// 已索引文件数。
    pub files: u64,
    /// 提取到的符号数。
    pub symbols: u64,
    /// 索引构建耗时（毫秒）。
    pub elapsed_ms: u64,
    /// 索引内存占用估算（MB）。
    pub mem_est_mb: f64,
    /// 向量检索是否启用。
    pub vector: bool,
}

/// unmount_project 结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnmountReport {
    /// 是否成功卸载。
    pub unmounted: bool,
    /// 本次回收的 LSP 子进程数。
    pub killed_lsps: u32,
}

/// list_mounted_projects 的单个项目信息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// 项目根路径。
    pub path: String,
    /// 已索引文件数。
    pub files: u64,
    /// 符号数。
    pub symbols: u64,
    /// 该项目存活的语言服务器（进程名）。
    pub lsps: Vec<String>,
    /// 向量检索是否启用。
    pub vector: bool,
}

/// list_mounted_projects 结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectsReport {
    /// 当前已挂载项目。
    pub projects: Vec<ProjectInfo>,
}

/// sync_project 结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncReport {
    /// 新增文件数。
    pub added: u64,
    /// 修改文件数。
    pub modified: u64,
    /// 删除文件数。
    pub removed: u64,
    /// 同步后项目总文件数。
    pub files: u64,
    /// 同步耗时（毫秒）。
    pub elapsed_ms: u64,
}

/// search_code 单条命中。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    /// 命中文件相对路径。
    pub file: String,
    /// 命中所属符号；文件级命中为 None（省略键）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// 命中起始行（1-based）。
    pub start_line: u32,
    /// 命中结束行（1-based，含）。
    pub end_line: u32,
    /// 排序得分（越高越相关）。
    pub score: f64,
    /// 命中片段（≤ 3 行）。
    pub snippet: String,
}

/// search_code 结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    /// 命中列表（得分降序）。
    pub results: Vec<SearchHit>,
    /// 实际生效的检索模式。
    pub mode: SearchMode,
    /// 检索耗时（毫秒）。
    pub elapsed_ms: u64,
}

/// get_file_outline 单条符号。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutlineEntry {
    /// 符号类别（小写字符串形态，如 "fn"、"class"；Phase 1-C 定映射）。
    pub kind: String,
    /// 符号名。
    pub name: String,
    /// 起始行（1-based）。
    pub start_line: u32,
    /// 结束行（1-based，含）。
    pub end_line: u32,
    /// 归一化签名。
    pub sig: String,
}

/// get_file_outline 结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileOutline {
    /// 文件相对路径。
    pub file: String,
    /// 符号列表。
    pub symbols: Vec<OutlineEntry>,
}

/// read_symbol 结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolCode {
    /// 符号所在文件相对路径。
    pub file: String,
    /// 符号名。
    pub name: String,
    /// 起始行（1-based）。
    pub start_line: u32,
    /// 结束行（1-based，含）。
    pub end_line: u32,
    /// 完整实现体文本。
    pub code: String,
}

/// lsp 位置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    /// 文件相对路径。
    pub file: String,
    /// 行号（1-based）。
    pub line: u32,
    /// 列号（0-based）。
    pub character: u32,
}

/// lsp_definition / lsp_references 结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocationsReport {
    /// 位置列表。
    pub locations: Vec<Location>,
}

/// lsp_hover 结果（contents 内部结构由 Phase 3 冻结）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HoverReport {
    /// hover 内容。
    pub contents: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    fn round_trip<T>(value: &T)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, value);
    }

    #[test]
    fn mount_report_compact_json_shape() {
        let report = MountReport {
            mounted: true,
            path: "/repo".into(),
            files: 12,
            symbols: 34,
            elapsed_ms: 56,
            mem_est_mb: 1.5,
            vector: false,
        };
        let value: Value = serde_json::to_value(&report).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "mounted": true,
                "path": "/repo",
                "files": 12,
                "symbols": 34,
                "elapsed_ms": 56,
                "mem_est_mb": 1.5,
                "vector": false
            })
        );
        round_trip(&report);
    }

    #[test]
    fn mount_params_parse_snake_case_and_default() {
        let params: MountParams =
            serde_json::from_str(r#"{"path":"/repo","enable_vector":true}"#).unwrap();
        assert_eq!(params.path, "/repo");
        assert_eq!(params.enable_vector, Some(true));

        let minimal: MountParams = serde_json::from_str(r#"{"path":"/r"}"#).unwrap();
        assert_eq!(minimal.enable_vector, None);
        round_trip(&minimal);
    }

    #[test]
    fn search_mode_serializes_lowercase() {
        assert_eq!(serde_json::to_value(SearchMode::Hybrid).unwrap(), "hybrid");
        assert_eq!(
            serde_json::to_value(SearchMode::Keyword).unwrap(),
            "keyword"
        );
        assert_eq!(serde_json::to_value(SearchMode::Vector).unwrap(), "vector");
    }

    #[test]
    fn lsp_position_params_parse_snake_case() {
        let params: LspPositionParams =
            serde_json::from_str(r#"{"path":"/r","file_path":"src/a.rs","line":3,"character":7}"#)
                .unwrap();
        assert_eq!(params.file_path, "src/a.rs");
        assert_eq!(params.line, 3);
        assert_eq!(params.character, 7);
    }

    #[test]
    fn search_params_parse_optional_mode_and_top_k() {
        let params: SearchParams =
            serde_json::from_str(r#"{"path":"/r","query":"unicorn"}"#).unwrap();
        assert_eq!(params.mode, None);
        assert_eq!(params.top_k, None);

        let full: SearchParams =
            serde_json::from_str(r#"{"path":"/r","query":"u","mode":"keyword","top_k":5}"#)
                .unwrap();
        assert_eq!(full.mode, Some(SearchMode::Keyword));
        assert_eq!(full.top_k, Some(5));
    }

    #[test]
    fn search_hit_omits_null_symbol_key() {
        let hit = SearchHit {
            file: "src/a.rs".into(),
            symbol: None,
            start_line: 1,
            end_line: 2,
            score: 0.5,
            snippet: "fn f()".into(),
        };
        let value: Value = serde_json::to_value(&hit).unwrap();
        assert!(value.get("symbol").is_none());
        round_trip(&hit);
    }

    /// 防回归：输出 JSON 的字段名必须是 ARCHITECTURE §4 契约表格的蛇形形态。
    #[test]
    fn output_json_uses_snake_case_keys() {
        let mount = serde_json::to_string(&MountReport {
            mounted: true,
            path: "/r".into(),
            files: 1,
            symbols: 2,
            elapsed_ms: 3,
            mem_est_mb: 4.0,
            vector: false,
        })
        .unwrap();
        assert!(mount.contains("\"elapsed_ms\":3"));
        assert!(mount.contains("\"mem_est_mb\":4.0"));

        let unmount = serde_json::to_string(&UnmountReport {
            unmounted: true,
            killed_lsps: 1,
        })
        .unwrap();
        assert!(unmount.contains("\"killed_lsps\":1"));

        let hit = serde_json::to_string(&SearchHit {
            file: "src/a.rs".into(),
            symbol: None,
            start_line: 1,
            end_line: 2,
            score: 0.5,
            snippet: "s".into(),
        })
        .unwrap();
        assert!(hit.contains("\"start_line\":1"));
        assert!(hit.contains("\"end_line\":2"));

        let outline = serde_json::to_string(&OutlineEntry {
            kind: "fn".into(),
            name: "f".into(),
            start_line: 1,
            end_line: 2,
            sig: "fn f()".into(),
        })
        .unwrap();
        assert!(outline.contains("\"start_line\":1"));

        let sync = serde_json::to_string(&SyncReport {
            added: 1,
            modified: 2,
            removed: 3,
            files: 4,
            elapsed_ms: 5,
        })
        .unwrap();
        assert!(sync.contains("\"elapsed_ms\":5"));
    }

    #[test]
    fn all_output_dtos_round_trip() {
        round_trip(&UnmountReport {
            unmounted: true,
            killed_lsps: 2,
        });
        round_trip(&SyncReport {
            added: 1,
            modified: 2,
            removed: 3,
            files: 4,
            elapsed_ms: 5,
        });
        round_trip(&ProjectsReport {
            projects: vec![ProjectInfo {
                path: "/r".into(),
                files: 1,
                symbols: 2,
                lsps: vec!["rust-analyzer".into()],
                vector: true,
            }],
        });
        round_trip(&SearchResult {
            results: vec![SearchHit {
                file: "src/a.rs".into(),
                symbol: Some("f".into()),
                start_line: 1,
                end_line: 2,
                score: 0.5,
                snippet: "fn f()".into(),
            }],
            mode: SearchMode::Hybrid,
            elapsed_ms: 9,
        });
        round_trip(&FileOutline {
            file: "src/a.rs".into(),
            symbols: vec![OutlineEntry {
                kind: "fn".into(),
                name: "f".into(),
                start_line: 1,
                end_line: 2,
                sig: "fn f()".into(),
            }],
        });
        round_trip(&SymbolCode {
            file: "src/a.rs".into(),
            name: "f".into(),
            start_line: 1,
            end_line: 2,
            code: "fn f() {}".into(),
        });
        round_trip(&LocationsReport {
            locations: vec![Location {
                file: "src/a.rs".into(),
                line: 1,
                character: 0,
            }],
        });
        round_trip(&HoverReport {
            contents: serde_json::json!({"type": "markdown", "value": "x"}),
        });
    }
}
