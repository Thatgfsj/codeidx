//! 工具路由表：11 个工具的名称 → 执行器映射与统一分发。
//!
//! Phase 1-A：全部 handler 为 stub（见 handlers 模块）。
//! 真实实现分别由 Phase 1-B/C（索引、检索）与 Phase 3（LSP 桥）接管。

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use codeidx_protocol::tools::tool_name;
use codeidx_protocol::{CallToolResult, ToolError, ToolPayload};

use crate::handlers;

/// 单个工具执行器：解析参数 → 产出 DTO 或执行错误。
type ToolHandler = fn(&Value) -> Result<ToolPayload, ToolError>;

/// 路由表（进程级单例，首次访问时构建）。
fn route_table() -> &'static HashMap<&'static str, ToolHandler> {
    static TABLE: OnceLock<HashMap<&'static str, ToolHandler>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: HashMap<&'static str, ToolHandler> = HashMap::new();
        table.insert(tool_name::MOUNT_PROJECT, handlers::mount_project);
        table.insert(tool_name::UNMOUNT_PROJECT, handlers::unmount_project);
        table.insert(
            tool_name::LIST_MOUNTED_PROJECTS,
            handlers::list_mounted_projects,
        );
        table.insert(tool_name::SYNC_PROJECT, handlers::sync_project);
        table.insert(tool_name::GET_REPO_MAP, handlers::get_repo_map);
        table.insert(tool_name::SEARCH_CODE, handlers::search_code);
        table.insert(tool_name::GET_FILE_OUTLINE, handlers::get_file_outline);
        table.insert(tool_name::READ_SYMBOL, handlers::read_symbol);
        table.insert(tool_name::LSP_DEFINITION, handlers::lsp_definition);
        table.insert(tool_name::LSP_REFERENCES, handlers::lsp_references);
        table.insert(tool_name::LSP_HOVER, handlers::lsp_hover);
        table
    })
}

/// 按名分发 tools/call。
///
/// 未知工具名 → `isError` + `"error: unknown tool: <name>"`
/// （任务书硬性约定：tools/call 对不存在的工具走 isError，而非 -32601）。
pub fn dispatch(name: &str, arguments: &Value) -> CallToolResult {
    let Some(&handler) = route_table().get(name) else {
        return ToolError::new(format!("unknown tool: {name}")).into();
    };
    match handler(arguments) {
        Ok(payload) => payload.into(),
        Err(err) => err.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codeidx_protocol::mcp::Content;

    #[test]
    fn all_11_tools_registered_and_match_specs() {
        assert_eq!(route_table().len(), 11);
        for spec in codeidx_protocol::schema::TOOL_SPECS.iter() {
            assert!(
                route_table().contains_key(spec.name),
                "missing handler: {}",
                spec.name
            );
        }
    }

    #[test]
    fn unknown_tool_is_execution_error() {
        let result = dispatch("no_such_tool", &serde_json::json!({}));
        assert!(result.is_error);
        match &result.content[0] {
            Content::Text { text } => assert!(text.starts_with("error: unknown tool: ")),
        }
    }

    #[test]
    fn lsp_tools_stub_contract() {
        let args = serde_json::json!({"path": "/tmp", "file_path": "x", "line": 1, "character": 0});
        for name in [
            tool_name::LSP_DEFINITION,
            tool_name::LSP_REFERENCES,
            tool_name::LSP_HOVER,
        ] {
            let result = dispatch(name, &args);
            assert!(result.is_error, "{name} must be isError in Phase 1");
            match &result.content[0] {
                Content::Text { text } => assert_eq!(text, "error: LSP bridge not available yet"),
            }
        }
    }

    /// Phase 1-B 真接线：路由层走全局 ProjectManager，挂载只读 fixture 仓。
    #[test]
    fn mount_returns_json_report() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sample-project");
        let args = serde_json::json!({"path": root});
        let result = dispatch(tool_name::MOUNT_PROJECT, &args);
        assert!(!result.is_error, "mount fixture must succeed");
        match &result.content[0] {
            Content::Text { text } => {
                let value: Value = serde_json::from_str(text).expect("report is compact JSON");
                assert_eq!(value["mounted"], true);
                assert_eq!(value["files"], 6);
            }
        }
        dispatch(tool_name::UNMOUNT_PROJECT, &args);
    }
}
