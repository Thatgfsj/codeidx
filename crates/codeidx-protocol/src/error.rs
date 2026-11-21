//! 工具执行错误（tools/call `result.isError`）类型。
//!
//! 约定（ARCHITECTURE §4）：工具执行失败不使用 JSON-RPC error，
//! 而是 `isError = true` + `content[0].text = "error: <原因>"`；
//! 协议级错误才回落到 JSON-RPC error（由 codeidx-mcp 的 server 层处理）。

use crate::mcp::CallToolResult;

/// 工具执行错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    /// 去掉前缀后的原因文本。
    pub message: String,
}

impl ToolError {
    /// 文本前缀（黑盒断言依据）。
    pub const PREFIX: &'static str = "error: ";

    /// 新建执行错误。
    pub fn new<S: Into<String>>(message: S) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// 渲染为 MCP content 文本：`error: <原因>`。
    pub fn to_text(&self) -> String {
        format!("{}{}", Self::PREFIX, self.message)
    }
}

impl From<&str> for ToolError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<ToolError> for CallToolResult {
    fn from(err: ToolError) -> Self {
        CallToolResult::error_text(err.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::Content;

    #[test]
    fn text_prefix_is_contractual() {
        let err = ToolError::new("project not mounted: /x");
        assert_eq!(err.to_text(), "error: project not mounted: /x");
    }

    #[test]
    fn converts_to_is_error_result() {
        let result: CallToolResult = ToolError::new("LSP bridge not available yet").into();
        assert!(result.is_error);
        match &result.content[0] {
            Content::Text { text } => assert_eq!(text, "error: LSP bridge not available yet"),
        }
    }

    #[test]
    fn accepts_string_and_str() {
        let from_str: ToolError = "boom".into();
        let from_string: ToolError = "bang".to_string().into();
        assert_eq!(from_str.message, "boom");
        assert_eq!(from_string.message, "bang");
    }
}
