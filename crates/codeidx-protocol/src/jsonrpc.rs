//! JSON-RPC 2.0 消息模型（请求 / 通知 / 响应）与标准错误码。
//!
//! 传输形态：一行一条 JSON 消息（stdio，LSP 式 Content-Length 头不使用）。
//! 入站 [`Incoming`] 解析失败视作非法消息，由调用方回 ParseError。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 协议版本常量（消息内 `"jsonrpc"` 字段的固定值）。
pub const VERSION: &str = "2.0";

/// 解析错误（非法 JSON / 不符合 JSON-RPC 2.0 结构）。
pub const PARSE_ERROR: i64 = -32700;
/// 无效请求。
pub const INVALID_REQUEST: i64 = -32600;
/// 方法不存在（协议级；tools/call 的未知工具名走 isError，见 error 模块）。
pub const METHOD_NOT_FOUND: i64 = -32601;
/// 无效参数（如 tools/call 的 params 结构非法）。
pub const INVALID_PARAMS: i64 = -32602;
/// 服务端内部错误。
pub const INTERNAL_ERROR: i64 = -32603;

/// JSON-RPC 请求 / 响应标识符。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    /// 数字标识（JSON-RPC 首选）。
    Number(i64),
    /// 字符串标识（MCP 客户端可能使用）。
    Text(String),
}

/// 入站消息：请求（带 id，需要响应）或通知（无 id）。
///
/// untagged 解析失败 = 非法 JSON-RPC 结构，由调用方回 ParseError
/// （响应 id 按规范为 null，见 [`Outgoing::parse_error`]）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Incoming {
    /// 客户端请求。
    Request(Request),
    /// 客户端通知。
    Notification(Notification),
}

/// JSON-RPC 请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// 固定为 `"2.0"`。
    pub jsonrpc: String,
    /// 请求标识。
    pub id: Id,
    /// 方法名。
    pub method: String,
    /// 方法参数；缺省为 None。
    #[serde(default)]
    pub params: Option<Value>,
}

/// JSON-RPC 通知（无 id，不需要响应）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    /// 固定为 `"2.0"`。
    pub jsonrpc: String,
    /// 方法名。
    pub method: String,
    /// 方法参数；缺省为 None。
    #[serde(default)]
    pub params: Option<Value>,
}

/// 出站消息（成功响应或错误响应）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Outgoing {
    /// 成功响应。
    Ok(SuccessResponse),
    /// 错误响应。
    Err(ErrorResponse),
}

impl Outgoing {
    /// 构造成功响应（result 由 DTO 序列化而来）。
    pub fn ok(id: Id, result: impl Serialize) -> Self {
        Self::Ok(SuccessResponse {
            jsonrpc: VERSION.to_owned(),
            id,
            result: serde_json::to_value(&result).expect("serialize success result"),
        })
    }

    /// 构造错误响应（data 省略）。
    pub fn error(id: Id, code: i64, message: impl Into<String>) -> Self {
        Self::Err(ErrorResponse {
            jsonrpc: VERSION.to_owned(),
            id: Some(id),
            error: RpcError {
                code,
                message: message.into(),
                data: None,
            },
        })
    }

    /// 解析错误响应：规范要求 id 为 null。
    pub fn parse_error() -> Self {
        Self::Err(ErrorResponse {
            jsonrpc: VERSION.to_owned(),
            id: None,
            error: RpcError {
                code: PARSE_ERROR,
                message: "Parse error".to_owned(),
                data: None,
            },
        })
    }
}

/// JSON-RPC 成功响应。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuccessResponse {
    /// 固定为 `"2.0"`。
    pub jsonrpc: String,
    /// 对应请求的标识。
    pub id: Id,
    /// 结果。
    pub result: Value,
}

/// JSON-RPC 错误响应。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// 固定为 `"2.0"`。
    pub jsonrpc: String,
    /// 对应请求的标识；ParseError 时为 null。
    pub id: Option<Id>,
    /// 错误对象。
    pub error: RpcError,
}

/// JSON-RPC error 对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    /// 错误码（标准码见本模块顶部常量）。
    pub code: i64,
    /// 人读错误信息。
    pub message: String,
    /// 附加数据；缺省省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip() {
        let raw =
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_code"}}"#;
        let msg: Incoming = serde_json::from_str(raw).unwrap();
        let Incoming::Request(req) = &msg else {
            panic!("expected request");
        };
        assert_eq!(req.id, Id::Number(1));
        assert_eq!(req.method, "tools/call");
        let back: Incoming = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn notification_round_trip() {
        let raw = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let msg: Incoming = serde_json::from_str(raw).unwrap();
        assert!(matches!(msg, Incoming::Notification(_)));
        let back: Incoming = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn string_id_round_trip() {
        let raw = r#"{"jsonrpc":"2.0","id":"abc-1","method":"ping"}"#;
        let msg: Incoming = serde_json::from_str(raw).unwrap();
        let Incoming::Request(req) = &msg else {
            panic!("expected request");
        };
        assert_eq!(req.id, Id::Text("abc-1".into()));
        let back: Incoming = serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn params_default_to_none() {
        let raw = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let Incoming::Notification(ntf) = serde_json::from_str::<Incoming>(raw).unwrap() else {
            panic!("expected notification");
        };
        assert_eq!(ntf.params, None);
    }

    #[test]
    fn invalid_message_fails_to_parse() {
        assert!(serde_json::from_str::<Incoming>(r#"{"jsonrpc":"2.0"}"#).is_err());
        assert!(serde_json::from_str::<Incoming>("not json at all").is_err());
    }

    #[test]
    fn outgoing_success_shape() {
        let msg = Outgoing::ok(Id::Number(7), serde_json::json!({"tools":[]}));
        let value: Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"jsonrpc":"2.0","id":7,"result":{"tools":[]}})
        );
    }

    #[test]
    fn outgoing_error_shape_and_round_trip() {
        let msg = Outgoing::error(Id::Number(2), METHOD_NOT_FOUND, "method not found: x");
        let value: Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(value["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(value["error"]["message"], "method not found: x");
        assert!(value["error"].get("data").is_none());
        let back: Outgoing = serde_json::from_value(value).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn parse_error_response_has_null_id() {
        let msg = Outgoing::parse_error();
        let value: Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(value["id"], Value::Null);
        assert_eq!(value["error"]["code"], PARSE_ERROR);
        assert_eq!(value["error"]["message"], "Parse error");
    }
}
