//! Session management bindings for NAPI

use napi_derive::napi;

/// Session summary for listing.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSessionSummary {
    /// Session identifier.
    pub id: String,
    /// Session title.
    pub title: Option<String>,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
    /// Last update timestamp (RFC 3339).
    pub updated_at: String,
    /// Number of messages in the session.
    pub message_count: u32,
    /// Total tokens consumed.
    pub total_tokens: u32,
    /// Session tags.
    pub tags: Vec<String>,
    /// Working directory path.
    pub working_dir: String,
}

impl From<forge_sdk::SessionSummary> for JsSessionSummary {
    fn from(s: forge_sdk::SessionSummary) -> Self {
        Self {
            id: s.id,
            title: s.title,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
            message_count: u32::try_from(s.message_count).unwrap_or(u32::MAX),
            total_tokens: u32::try_from(s.total_tokens).unwrap_or(u32::MAX),
            tags: s.tags,
            working_dir: s.working_dir.to_string_lossy().to_string(),
        }
    }
}

/// Session status information.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSessionStatus {
    /// Session identifier.
    pub id: String,
    /// Number of messages in the session.
    pub message_count: u32,
    /// Current model name.
    pub model: String,
    /// Working directory path.
    pub working_dir: String,
    /// Input tokens consumed.
    pub input_tokens: u32,
    /// Output tokens generated.
    pub output_tokens: u32,
    /// Cache read tokens.
    pub cache_read_tokens: Option<u32>,
    /// Cache creation tokens.
    pub cache_creation_tokens: Option<u32>,
    /// Context token limit.
    pub context_limit: u32,
    /// Current persona name.
    pub persona: String,
    /// Session title.
    pub title: Option<String>,
    /// Whether the session has unsaved changes.
    pub is_dirty: bool,
}

impl From<forge_sdk::SessionStatus> for JsSessionStatus {
    fn from(s: forge_sdk::SessionStatus) -> Self {
        Self {
            id: s.id,
            message_count: u32::try_from(s.message_count).unwrap_or(u32::MAX),
            model: s.model,
            working_dir: s.working_dir.to_string_lossy().to_string(),
            input_tokens: u32::try_from(s.token_usage.input_tokens).unwrap_or(u32::MAX),
            output_tokens: u32::try_from(s.token_usage.output_tokens).unwrap_or(u32::MAX),
            cache_read_tokens: s
                .token_usage
                .cache_read_tokens
                .map(|t| u32::try_from(t).unwrap_or(u32::MAX)),
            cache_creation_tokens: s
                .token_usage
                .cache_creation_tokens
                .map(|t| u32::try_from(t).unwrap_or(u32::MAX)),
            context_limit: u32::try_from(s.context_limit).unwrap_or(u32::MAX),
            persona: s.persona,
            title: s.title,
            is_dirty: s.is_dirty,
        }
    }
}

/// Model switch result.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsModelSwitchResult {
    /// Previous model name.
    pub previous_model: String,
    /// New model name.
    pub new_model: String,
}

impl From<forge_sdk::ModelSwitchResult> for JsModelSwitchResult {
    fn from(r: forge_sdk::ModelSwitchResult) -> Self {
        Self { previous_model: r.previous_model, new_model: r.new_model }
    }
}

/// Context compression result.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsCompressionResult {
    /// Number of messages before compression.
    pub messages_before: u32,
    /// Number of messages after compression.
    pub messages_after: u32,
    /// Estimated tokens before compression.
    pub tokens_before: u32,
    /// Estimated tokens after compression.
    pub tokens_after: u32,
    /// Summary of the compression.
    pub summary: Option<String>,
}

impl From<forge_session::CompressionResult> for JsCompressionResult {
    fn from(r: forge_session::CompressionResult) -> Self {
        Self {
            messages_before: u32::try_from(r.messages_before).unwrap_or(u32::MAX),
            messages_after: u32::try_from(r.messages_after).unwrap_or(u32::MAX),
            tokens_before: u32::try_from(r.tokens_before).unwrap_or(u32::MAX),
            tokens_after: u32::try_from(r.tokens_after).unwrap_or(u32::MAX),
            summary: r.summary,
        }
    }
}

/// History message for multi-turn conversations.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsHistoryMessage {
    /// Message role ("user", "assistant", "system").
    pub role: String,
    /// Message content.
    pub content: String,
}

impl From<JsHistoryMessage> for forge_sdk::HistoryMessage {
    fn from(m: JsHistoryMessage) -> Self {
        match m.role.as_str() {
            "user" => forge_sdk::HistoryMessage::user(&m.content),
            "assistant" => forge_sdk::HistoryMessage::assistant(&m.content),
            "system" => forge_sdk::HistoryMessage::system(&m.content),
            _ => forge_sdk::HistoryMessage::user(&m.content),
        }
    }
}

impl From<forge_sdk::HistoryMessage> for JsHistoryMessage {
    fn from(m: forge_sdk::HistoryMessage) -> Self {
        Self {
            role: match m.role {
                forge_sdk::HistoryRole::User => "user".to_string(),
                forge_sdk::HistoryRole::Assistant => "assistant".to_string(),
                forge_sdk::HistoryRole::System => "system".to_string(),
            },
            content: m.content,
        }
    }
}

// ========================
// MCP Types
// ========================

/// MCP tool information.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsMcpToolInfo {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: Option<String>,
    /// Server that provides this tool.
    pub server_name: String,
}

impl From<forge_sdk::McpToolInfo> for JsMcpToolInfo {
    fn from(t: forge_sdk::McpToolInfo) -> Self {
        Self { name: t.name, description: t.description, server_name: t.server_name }
    }
}

/// MCP server information.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsMcpServerInfo {
    /// Server name.
    pub name: String,
    /// Transport type: "stdio", "sse", or "streamable_http".
    pub transport: String,
    /// Command used to start the server (stdio transport).
    pub command: String,
    /// Command arguments (stdio transport).
    pub args: Vec<String>,
    /// SSE endpoint URL (sse/streamable_http transport).
    pub url: Option<String>,
    /// Connection status: "connected", "disconnected", "configured", or "error".
    pub status: String,
    /// Error message if status is "error".
    pub error: Option<String>,
    /// Tools provided by this server.
    pub tools: Vec<JsMcpToolInfo>,
}

impl From<forge_sdk::McpServerInfo> for JsMcpServerInfo {
    fn from(s: forge_sdk::McpServerInfo) -> Self {
        Self {
            name: s.name,
            transport: match s.transport {
                forge_sdk::McpTransportType::Stdio => "stdio".to_string(),
                forge_sdk::McpTransportType::Sse => "sse".to_string(),
                forge_sdk::McpTransportType::StreamableHttp => "streamable_http".to_string(),
            },
            command: s.command,
            args: s.args,
            url: s.url,
            status: match s.status {
                forge_sdk::McpServerStatus::Configured => "configured".to_string(),
                forge_sdk::McpServerStatus::Connected => "connected".to_string(),
                forge_sdk::McpServerStatus::Disconnected => "disconnected".to_string(),
                forge_sdk::McpServerStatus::Error => "error".to_string(),
            },
            error: s.error,
            tools: s.tools.into_iter().map(Into::into).collect(),
        }
    }
}

/// MCP status summary.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsMcpStatus {
    /// List of configured MCP servers.
    pub servers: Vec<JsMcpServerInfo>,
    /// Total number of tools from all servers.
    pub total_tools: u32,
    /// Number of connected servers.
    pub connected_count: u32,
}

impl From<forge_sdk::McpStatus> for JsMcpStatus {
    fn from(s: forge_sdk::McpStatus) -> Self {
        Self {
            servers: s.servers.into_iter().map(Into::into).collect(),
            total_tools: u32::try_from(s.total_tools).unwrap_or(u32::MAX),
            connected_count: u32::try_from(s.connected_count).unwrap_or(u32::MAX),
        }
    }
}

// ========================
// MCP Management Types
// ========================

/// MCP server configuration for CRUD operations.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsMcpServerManageConfig {
    /// Server name (unique identifier).
    pub name: String,
    /// Transport type: "stdio", "sse", or "streamable_http".
    pub transport: String,
    /// Whether this server is enabled.
    pub enabled: bool,

    // Stdio transport fields
    /// Command to execute (stdio transport).
    pub command: String,
    /// Command arguments (stdio transport).
    pub args: Vec<String>,
    /// Environment variables as JSON string (stdio transport).
    pub env_json: Option<String>,

    // SSE/HTTP transport fields
    /// Endpoint URL (sse/streamable_http transport).
    pub url: Option<String>,
    /// API key for authentication.
    pub api_key: Option<String>,
    /// Whether to read API key from keychain.
    pub api_key_from_keychain: bool,

    // SSE auth configuration
    /// API key auth mode: "bearer" or "header".
    pub api_key_auth: Option<String>,
    /// Custom API key header name.
    pub api_key_header: Option<String>,
    /// Custom API key prefix.
    pub api_key_prefix: Option<String>,

    /// Name of the proxy to use (None = direct connection).
    pub proxy_name: Option<String>,
}

impl From<forge_sdk::McpServerManageConfig> for JsMcpServerManageConfig {
    fn from(c: forge_sdk::McpServerManageConfig) -> Self {
        Self {
            name: c.name,
            transport: match c.transport {
                forge_sdk::McpTransportType::Stdio => "stdio".to_string(),
                forge_sdk::McpTransportType::Sse => "sse".to_string(),
                forge_sdk::McpTransportType::StreamableHttp => "streamable_http".to_string(),
            },
            enabled: c.enabled,
            command: c.command,
            args: c.args,
            env_json: if c.env.is_empty() {
                None
            } else {
                serde_json::to_string(&c.env).ok()
            },
            url: c.url,
            api_key: c.api_key,
            api_key_from_keychain: c.api_key_from_keychain,
            api_key_auth: c.api_key_auth,
            api_key_header: c.api_key_header,
            api_key_prefix: c.api_key_prefix,
            proxy_name: c.proxy_name,
        }
    }
}

impl From<JsMcpServerManageConfig> for forge_sdk::McpServerManageConfig {
    fn from(c: JsMcpServerManageConfig) -> Self {
        let env: std::collections::HashMap<String, String> = c
            .env_json
            .and_then(|json| {
                serde_json::from_str(&json)
                    .map_err(|e| {
                        eprintln!(
                            "[forge-napi] McpServerManageConfig: invalid env_json, \
                             environment variables will be empty: {e}"
                        );
                    })
                    .ok()
            })
            .unwrap_or_default();

        Self {
            name: c.name,
            transport: match c.transport.as_str() {
                "sse" => forge_sdk::McpTransportType::Sse,
                "streamable_http" => forge_sdk::McpTransportType::StreamableHttp,
                _ => forge_sdk::McpTransportType::Stdio,
            },
            enabled: c.enabled,
            command: c.command,
            args: c.args,
            env,
            url: c.url,
            api_key: c.api_key,
            api_key_from_keychain: c.api_key_from_keychain,
            api_key_auth: c.api_key_auth,
            api_key_header: c.api_key_header,
            api_key_prefix: c.api_key_prefix,
            proxy_name: c.proxy_name,
        }
    }
}

/// Result of MCP connection test.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsMcpConnectionTestResult {
    /// Whether the connection succeeded.
    pub success: bool,
    /// Error message if connection failed.
    pub error: Option<String>,
    /// List of tools available from the server.
    pub tools: Vec<JsMcpToolInfo>,
    /// Server protocol version.
    pub protocol_version: Option<String>,
}

impl From<forge_sdk::McpConnectionTestResult> for JsMcpConnectionTestResult {
    fn from(r: forge_sdk::McpConnectionTestResult) -> Self {
        Self {
            success: r.success,
            error: r.error,
            tools: r.tools.into_iter().map(Into::into).collect(),
            protocol_version: r.protocol_version,
        }
    }
}
