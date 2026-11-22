//! codeidx-protocol —— JSON-RPC 2.0 + MCP 类型 + 12 工具 DTO / Schema。
//!
//! 叶子 crate（无内部依赖，见 ARCHITECTURE §2）：为 codeidx-mcp 提供协议
//! 编解码与工具数据契约；这里的序列化产物是 docs/ACCEPTANCE.md 黑盒断言
//! 的直接对象。
//!
//! 全 crate 约定（ARCHITECTURE §4）：
//! - 行号：对外 1-based line、0-based character；
//! - 工具成功默认返回 compact 文本，按显式 format 选择 JSON 或 Markdown；
//! - 工具执行失败走 `isError = true` + `"error: <原因>"` 文本，
//!   仅协议级错误（如 method not found）使用 JSON-RPC error。

pub mod error;
pub mod jsonrpc;
pub mod mcp;
pub mod schema;
pub mod tools;

pub use error::ToolError;
pub use jsonrpc::{
    ErrorResponse, Id, Incoming, Notification, Outgoing, Request, RpcError, SuccessResponse,
    INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR, VERSION,
};
pub use mcp::{
    CallToolResult, Content, Implementation, InitializeParams, InitializeResult,
    ServerCapabilities, ToolDescriptor, ToolPayload, ToolsCallParams, ToolsCapability,
    ToolsListResult, PROTOCOL_VERSION,
};
pub use tools::*;
