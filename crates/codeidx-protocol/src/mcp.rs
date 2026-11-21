//! MCP 层类型：initialize 握手、tools 列表与 tools/call 结果。
//!
//! 字段命名对齐 MCP 规范的 camelCase（serde 统一 `rename_all`）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 本端实现并对外宣告的 MCP 协议版本。
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// MCP 方法名常量。
pub mod method {
    /// `initialize` 请求。
    pub const INITIALIZE: &str = "initialize";
    /// `notifications/initialized` 通知。
    pub const NOTIFICATIONS_INITIALIZED: &str = "notifications/initialized";
    /// `ping` 请求。
    pub const PING: &str = "ping";
    /// `tools/list` 请求。
    pub const TOOLS_LIST: &str = "tools/list";
    /// `tools/call` 请求。
    pub const TOOLS_CALL: &str = "tools/call";
}

/// 实现信息（initialize 的 clientInfo / serverInfo）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Implementation {
    /// 实现名。
    pub name: String,
    /// 实现版本。
    pub version: String,
}

/// `initialize` 请求参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// 客户端请求的协议版本。
    pub protocol_version: String,
    /// 客户端能力声明（原样透传，本端不解读）。
    #[serde(default)]
    pub capabilities: Value,
    /// 客户端实现信息。
    #[serde(default)]
    pub client_info: Option<Implementation>,
}

/// `initialize` 响应结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// 服务端宣告的协议版本。
    pub protocol_version: String,
    /// 服务端能力。
    pub capabilities: ServerCapabilities,
    /// 服务端实现信息。
    pub server_info: Implementation,
}

/// 服务端能力声明（本服务只声明 tools）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    /// tools 能力。
    pub tools: ToolsCapability,
}

/// tools 能力。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsCapability {
    /// 是否推送 tools 列表变更通知（当前恒 false）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

/// `tools/list` 响应结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolsListResult {
    /// 全部可用工具。
    pub tools: Vec<ToolDescriptor>,
}

/// 单个工具的描述符。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    /// 工具名（tools/call 的 name）。
    pub name: String,
    /// 工具描述。
    pub description: String,
    /// 输入 JSON Schema。
    pub input_schema: Value,
}

/// `tools/call` 请求参数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsCallParams {
    /// 工具名。
    pub name: String,
    /// 工具参数（工具各自的 DTO；缺省为 null）。
    #[serde(default)]
    pub arguments: Value,
}

/// `tools/call` 响应结果：content 块 + isError。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
    /// 内容块（Phase 1 仅 text）。
    pub content: Vec<Content>,
    /// 执行失败标记；false 时省略（MCP 允许缺省）。
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
}

fn is_false(v: &bool) -> bool {
    !v
}

impl CallToolResult {
    /// 成功结果：DTO 序列化为紧凑 JSON 文本。
    pub fn text(payload: impl Serialize) -> Self {
        let json = serde_json::to_string(&payload).expect("serialize tool payload");
        Self {
            content: vec![Content::text(json)],
            is_error: false,
        }
    }

    /// 执行错误结果（text 必须已带 `"error: "` 前缀）。
    pub fn error_text<S: Into<String>>(text: S) -> Self {
        Self {
            content: vec![Content::text(text)],
            is_error: true,
        }
    }
}

/// 工具产物：DTO 的紧凑 JSON，或纯文本（get_repo_map 骨架）。
///
/// 由路由层统一转换为 [`CallToolResult`]。
#[derive(Debug, Clone, PartialEq)]
pub enum ToolPayload {
    /// serde DTO → 紧凑 JSON 字符串。
    Json(Value),
    /// 纯文本内容。
    Text(String),
}

impl From<Value> for ToolPayload {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}

impl From<ToolPayload> for CallToolResult {
    fn from(payload: ToolPayload) -> Self {
        match payload {
            ToolPayload::Json(value) => CallToolResult::text(value),
            ToolPayload::Text(s) => CallToolResult {
                content: vec![Content::text(s)],
                is_error: false,
            },
        }
    }
}

/// content 块。Phase 1 只需要 text 类型。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Content {
    /// 纯文本内容。
    #[serde(rename = "text")]
    Text {
        /// 文本本体。
        text: String,
    },
}

impl Content {
    /// 构造 text 块。
    pub fn text<S: Into<String>>(text: S) -> Self {
        Self::Text { text: text.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_result_camel_case_shape() {
        let result = InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            capabilities: ServerCapabilities {
                tools: ToolsCapability {
                    list_changed: Some(false),
                },
            },
            server_info: Implementation {
                name: "codeidx-mcp".into(),
                version: "0.1.0".into(),
            },
        };
        let value: Value = serde_json::to_value(&result).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "codeidx-mcp", "version": "0.1.0"}
            })
        );
        let back: InitializeResult = serde_json::from_value(value).unwrap();
        assert_eq!(result, back);
    }

    #[test]
    fn success_call_result_omits_is_error() {
        let result = CallToolResult::text(serde_json::json!({"mounted": true}));
        let value: Value = serde_json::to_value(&result).unwrap();
        assert!(value.get("isError").is_none());
        assert_eq!(value["content"][0]["type"], "text");
        assert_eq!(value["content"][0]["text"], "{\"mounted\":true}");
        let back: CallToolResult = serde_json::from_value(value).unwrap();
        assert_eq!(result, back);
    }

    #[test]
    fn error_call_result_keeps_is_error_true() {
        let result = CallToolResult::error_text("error: project not mounted");
        assert!(result.is_error);
        let value: Value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["isError"], true);
        assert_eq!(value["content"][0]["text"], "error: project not mounted");
    }

    #[test]
    fn text_payload_becomes_plain_text_content() {
        let result: CallToolResult = ToolPayload::Text("skeleton".into()).into();
        assert!(!result.is_error);
        match &result.content[0] {
            Content::Text { text } => assert_eq!(text, "skeleton"),
        }
    }

    #[test]
    fn tools_call_params_default_arguments() {
        let params: ToolsCallParams =
            serde_json::from_str(r#"{"name":"list_mounted_projects"}"#).unwrap();
        assert_eq!(params.name, "list_mounted_projects");
        assert_eq!(params.arguments, Value::Null);
        let value: Value = serde_json::to_value(&params).unwrap();
        let back: ToolsCallParams = serde_json::from_value(value).unwrap();
        assert_eq!(params, back);
    }

    #[test]
    fn descriptor_camel_case_schema_key() {
        let descriptor = ToolDescriptor {
            name: "search_code".into(),
            description: "d".into(),
            input_schema: serde_json::json!({"type":"object"}),
        };
        let value: Value = serde_json::to_value(&descriptor).unwrap();
        assert!(value.get("inputSchema").is_some());
        assert!(value.get("input_schema").is_none());
    }
}
