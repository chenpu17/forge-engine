//! MCP protocol type definitions.
//!
//! Based on the Model Context Protocol specification.
//! <https://spec.modelcontextprotocol.io>

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// JSON-RPC protocol version.
pub const JSONRPC_VERSION: &str = "2.0";

/// MCP protocol version (2025-11-25 spec).
pub const MCP_VERSION: &str = "2025-11-25";

// =============================================================================
// JSON-RPC 2.0 Types
// =============================================================================

/// JSON-RPC request ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// String ID.
    String(String),
    /// Numeric ID.
    Number(i64),
}

impl From<String> for RequestId {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<i64> for RequestId {
    fn from(n: i64) -> Self {
        Self::Number(n)
    }
}

/// JSON-RPC request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version.
    pub jsonrpc: String,
    /// Request ID.
    pub id: RequestId,
    /// Method name.
    pub method: String,
    /// Method parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new request.
    pub fn new(id: impl Into<RequestId>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self { jsonrpc: JSONRPC_VERSION.to_string(), id: id.into(), method: method.into(), params }
    }
}

/// JSON-RPC response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Protocol version.
    pub jsonrpc: String,
    /// Request ID.
    pub id: RequestId,
    /// Result (mutually exclusive with error).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error (mutually exclusive with result).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Create a success response.
    pub fn success(id: impl Into<RequestId>, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response.
    pub fn error(id: impl Into<RequestId>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: id.into(),
            result: None,
            error: Some(error),
        }
    }
}

/// JSON-RPC error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code.
    pub code: i32,
    /// Error message.
    pub message: String,
    /// Optional additional data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Standard JSON-RPC error codes.
pub mod error_codes {
    /// Parse error.
    pub const PARSE_ERROR: i32 = -32700;
    /// Invalid request.
    pub const INVALID_REQUEST: i32 = -32600;
    /// Method not found.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Invalid params.
    pub const INVALID_PARAMS: i32 = -32602;
    /// Internal error.
    pub const INTERNAL_ERROR: i32 = -32603;
}

/// JSON-RPC notification (no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    /// Protocol version.
    pub jsonrpc: String,
    /// Method name.
    pub method: String,
    /// Method parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    /// Create a new notification.
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self { jsonrpc: JSONRPC_VERSION.to_string(), method: method.into(), params }
    }
}

/// Any JSON-RPC message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    /// Request.
    Request(JsonRpcRequest),
    /// Response.
    Response(JsonRpcResponse),
    /// Notification.
    Notification(JsonRpcNotification),
}

// =============================================================================
// MCP Initialization
// =============================================================================

/// Client capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// Experimental capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<HashMap<String, Value>>,
    /// Roots capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootsCapability>,
    /// Sampling capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling: Option<SamplingCapability>,
}

/// Roots capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RootsCapability {
    /// Whether `list_changed` notifications are supported.
    #[serde(default)]
    pub list_changed: bool,
}

/// Sampling capability (for server-initiated LLM calls).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SamplingCapability {}

/// Server capabilities.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// Experimental capabilities.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub experimental: Option<HashMap<String, Value>>,
    /// Logging capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logging: Option<LoggingCapability>,
    /// Prompts capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompts: Option<PromptsCapability>,
    /// Resources capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesCapability>,
    /// Tools capability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolsCapability>,
}

/// Logging capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoggingCapability {}

/// Prompts capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptsCapability {
    /// Whether `list_changed` notifications are supported.
    #[serde(default)]
    pub list_changed: bool,
}

/// Resources capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourcesCapability {
    /// Whether `list_changed` notifications are supported.
    #[serde(default)]
    pub list_changed: bool,
    /// Whether subscriptions are supported.
    #[serde(default)]
    pub subscribe: bool,
}

/// Tools capability.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsCapability {
    /// Whether `list_changed` notifications are supported.
    #[serde(default)]
    pub list_changed: bool,
}

/// Client information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Client name.
    pub name: String,
    /// Client version.
    pub version: String,
}

/// Server information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Server name.
    pub name: String,
    /// Server version.
    pub version: String,
}

/// Initialize request parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Protocol version.
    pub protocol_version: String,
    /// Client capabilities.
    pub capabilities: ClientCapabilities,
    /// Client info.
    pub client_info: ClientInfo,
}

/// Initialize response result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Protocol version.
    pub protocol_version: String,
    /// Server capabilities.
    pub capabilities: ServerCapabilities,
    /// Server info.
    pub server_info: ServerInfo,
    /// Optional instructions for the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

// =============================================================================
// Tools
// =============================================================================

/// MCP tool definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    /// Tool name.
    pub name: String,
    /// Tool description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Input schema (JSON Schema).
    pub input_schema: Value,
}

/// Tool list result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolListResult {
    /// Available tools.
    pub tools: Vec<McpTool>,
    /// Pagination cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Tool call request parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallParams {
    /// Tool name.
    pub name: String,
    /// Tool arguments.
    #[serde(default)]
    pub arguments: HashMap<String, Value>,
}

/// Tool call result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallResult {
    /// Result content.
    pub content: Vec<ContentBlock>,
    /// Whether the result is an error.
    #[serde(default)]
    pub is_error: bool,
}

// =============================================================================
// Resources
// =============================================================================

/// MCP resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResource {
    /// Resource URI.
    pub uri: String,
    /// Resource name.
    pub name: String,
    /// Resource description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Resource template.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTemplate {
    /// URI template (RFC 6570).
    pub uri_template: String,
    /// Template name.
    pub name: String,
    /// Template description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MIME type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// Resource list result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceListResult {
    /// Available resources.
    pub resources: Vec<McpResource>,
    /// Pagination cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Resource read request parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceReadParams {
    /// Resource URI.
    pub uri: String,
}

/// Resource read result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceReadResult {
    /// Resource contents.
    pub contents: Vec<ResourceContent>,
}

/// Resource content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceContent {
    /// Resource URI.
    pub uri: String,
    /// MIME type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// Text content (mutually exclusive with blob).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Binary content as base64 (mutually exclusive with text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blob: Option<String>,
}

// =============================================================================
// Prompts
// =============================================================================

/// MCP prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    /// Prompt name.
    pub name: String,
    /// Prompt description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Prompt arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Vec<PromptArgument>>,
}

/// Prompt argument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    /// Argument name.
    pub name: String,
    /// Argument description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the argument is required.
    #[serde(default)]
    pub required: bool,
}

/// Prompt list result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptListResult {
    /// Available prompts.
    pub prompts: Vec<McpPrompt>,
    /// Pagination cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Prompt get request parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptGetParams {
    /// Prompt name.
    pub name: String,
    /// Prompt arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<HashMap<String, String>>,
}

/// Prompt get result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptGetResult {
    /// Prompt description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Prompt messages.
    pub messages: Vec<PromptMessage>,
}

/// Prompt message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMessage {
    /// Role.
    pub role: PromptRole,
    /// Content.
    pub content: ContentBlock,
}

/// Prompt role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptRole {
    /// User message.
    User,
    /// Assistant message.
    Assistant,
}

// =============================================================================
// Content
// =============================================================================

/// Content block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text content.
    Text {
        /// Text content.
        text: String,
    },
    /// Image content.
    Image {
        /// Image data as base64.
        data: String,
        /// MIME type.
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Embedded resource.
    Resource {
        /// Resource content.
        resource: ResourceContent,
    },
}

impl ContentBlock {
    /// Create a text content block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

// =============================================================================
// Logging
// =============================================================================

/// Log level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Debug level.
    Debug,
    /// Info level.
    Info,
    /// Notice level.
    Notice,
    /// Warning level.
    Warning,
    /// Error level.
    Error,
    /// Critical level.
    Critical,
    /// Alert level.
    Alert,
    /// Emergency level.
    Emergency,
}

/// Log message notification parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogMessageParams {
    /// Log level.
    pub level: LogLevel,
    /// Logger name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logger: Option<String>,
    /// Log data.
    pub data: Value,
}

// =============================================================================
// Progress
// =============================================================================

/// Progress notification parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressParams {
    /// Progress token.
    pub progress_token: RequestId,
    /// Progress value (0.0 to 1.0).
    pub progress: f64,
    /// Total units (if known).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
}

// =============================================================================
// MCP Method Names
// =============================================================================

/// MCP method names.
pub mod methods {
    // Lifecycle
    /// Initialize.
    pub const INITIALIZE: &str = "initialize";
    /// Initialized notification.
    pub const INITIALIZED: &str = "notifications/initialized";
    /// Ping.
    pub const PING: &str = "ping";

    // Tools
    /// List tools.
    pub const TOOLS_LIST: &str = "tools/list";
    /// Call tool.
    pub const TOOLS_CALL: &str = "tools/call";
    /// Tools list changed notification.
    pub const TOOLS_LIST_CHANGED: &str = "notifications/tools/list_changed";

    // Resources
    /// List resources.
    pub const RESOURCES_LIST: &str = "resources/list";
    /// List resource templates.
    pub const RESOURCES_TEMPLATES_LIST: &str = "resources/templates/list";
    /// Read resource.
    pub const RESOURCES_READ: &str = "resources/read";
    /// Subscribe to resource.
    pub const RESOURCES_SUBSCRIBE: &str = "resources/subscribe";
    /// Unsubscribe from resource.
    pub const RESOURCES_UNSUBSCRIBE: &str = "resources/unsubscribe";
    /// Resources list changed notification.
    pub const RESOURCES_LIST_CHANGED: &str = "notifications/resources/list_changed";
    /// Resource updated notification.
    pub const RESOURCES_UPDATED: &str = "notifications/resources/updated";

    // Prompts
    /// List prompts.
    pub const PROMPTS_LIST: &str = "prompts/list";
    /// Get prompt.
    pub const PROMPTS_GET: &str = "prompts/get";
    /// Prompts list changed notification.
    pub const PROMPTS_LIST_CHANGED: &str = "notifications/prompts/list_changed";

    // Logging
    /// Set log level.
    pub const LOGGING_SET_LEVEL: &str = "logging/setLevel";
    /// Log message notification.
    pub const LOG_MESSAGE: &str = "notifications/message";

    // Progress
    /// Progress notification.
    pub const PROGRESS: &str = "notifications/progress";

    // Cancellation
    /// Cancel request notification.
    pub const CANCELLED: &str = "notifications/cancelled";
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_request_serialization() {
        let request =
            JsonRpcRequest::new(1i64, "test_method", Some(serde_json::json!({"key": "value"})));

        let json = serde_json::to_string(&request).expect("serialize request");
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"test_method\""));
    }

    #[test]
    fn test_json_rpc_response_success() {
        let response = JsonRpcResponse::success(1i64, serde_json::json!({"result": "ok"}));

        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[test]
    fn test_json_rpc_response_error() {
        let error = JsonRpcError {
            code: error_codes::METHOD_NOT_FOUND,
            message: "Method not found".to_string(),
            data: None,
        };
        let response = JsonRpcResponse::error(1i64, error);

        assert!(response.result.is_none());
        assert!(response.error.is_some());
    }

    #[test]
    fn test_mcp_tool_serialization() {
        let tool = McpTool {
            name: "test_tool".to_string(),
            description: Some("A test tool".to_string()),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string"}
                }
            }),
        };

        let json = serde_json::to_string(&tool).expect("serialize tool");
        assert!(json.contains("\"name\":\"test_tool\""));
        assert!(json.contains("\"inputSchema\""));
    }

    #[test]
    fn test_content_block_text() {
        let block = ContentBlock::text("Hello, world!");

        let json = serde_json::to_string(&block).expect("serialize content block");
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"Hello, world!\""));
    }

    #[test]
    fn test_initialize_params() {
        let params = InitializeParams {
            protocol_version: MCP_VERSION.to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo { name: "forge".to_string(), version: "0.1.0".to_string() },
        };

        let json = serde_json::to_string(&params).expect("serialize init params");
        assert!(json.contains("\"protocolVersion\""));
        assert!(json.contains("\"clientInfo\""));
    }
}
