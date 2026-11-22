//! 同步工具路由表。异步工具在 server.rs 中分发。

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
        // server.rs 单独分发 lsp_* 和 trace_calls。
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

    /// 工具目录的每个名称必须对应同步路由或异步分发。
    #[test]
    fn sync_table_excludes_async_tools() {
        assert_eq!(route_table().len(), 8);
        for spec in codeidx_protocol::schema::TOOL_SPECS.iter() {
            let is_async =
                crate::server::is_lsp_tool(spec.name) || spec.name == tool_name::TRACE_CALLS;
            assert_eq!(
                route_table().contains_key(spec.name),
                !is_async,
                "unexpected routing for {}",
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

    /// Phase 1-B 真接线：路由层走全局 ProjectManager，挂载只读 fixture 仓。
    #[test]
    fn mount_returns_json_report() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/sample-project");
        let args = serde_json::json!({"path": root, "format": "json"});
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
