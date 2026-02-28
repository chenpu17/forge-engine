//! Additional types for Forge SDK
//!
//! Contains types for SDK operations like session status, MCP info,
//! and tool metadata.

use crate::event::TokenUsage;
use crate::session::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Request identifier (unique per in-flight processing request)
pub type RequestId = String;

/// Memory storage scope
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// User-level memory (global across projects)
    User,
    /// Project-level memory (stored under `.forge/` in the working directory)
    Project,
}

/// Event dispatch strategy for streaming output
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventDispatchMode {
    /// Emit events immediately (default)
    #[default]
    Immediate,
    /// Batch `TextDelta` events and flush on size/latency bounds
    Batched {
        /// Flush when buffered bytes exceed this size
        max_bytes: usize,
        /// Flush at least every N milliseconds (timer-driven)
        max_latency_ms: u64,
    },
}

/// Options for processing a request
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessOptions {
    /// Streaming dispatch mode
    #[serde(default)]
    pub dispatch_mode: EventDispatchMode,
}

/// Session status information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    /// Session ID
    pub id: SessionId,
    /// Number of messages in the session
    pub message_count: usize,
    /// Current model name
    pub model: String,
    /// Working directory
    pub working_dir: PathBuf,
    /// Token usage statistics
    pub token_usage: TokenUsage,
    /// Context limit (max tokens)
    pub context_limit: usize,
    /// Current persona
    pub persona: String,
    /// Session title (if any)
    pub title: Option<String>,
    /// Whether the session has unsaved changes
    pub is_dirty: bool,
}

/// Model switch result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSwitchResult {
    /// Previous model name
    pub previous_model: String,
    /// New model name
    pub new_model: String,
}

// Re-export CompressionResult from forge-session
pub use forge_session::CompressionResult;

// ========================
// MCP Types
// ========================

/// MCP server connection status
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpServerStatus {
    /// Server is configured but not yet connected
    #[default]
    Configured,
    /// Server is connected and ready
    Connected,
    /// Server is disconnected
    Disconnected,
    /// Server connection failed with error
    Error,
}

/// Information about an MCP tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: Option<String>,
    /// Server that provides this tool
    pub server_name: String,
}

/// MCP transport type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum McpTransportType {
    /// Stdio transport (subprocess communication)
    #[default]
    Stdio,
    /// SSE transport (HTTP Server-Sent Events)
    Sse,
    /// Streamable HTTP transport (MCP 2025-11-25 spec)
    #[serde(alias = "streamable_http")]
    StreamableHttp,
}

/// Information about an MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerInfo {
    /// Server name
    pub name: String,
    /// Transport type
    #[serde(default)]
    pub transport: McpTransportType,
    /// Command used to start the server (for stdio transport)
    #[serde(default)]
    pub command: String,
    /// Command arguments (for stdio transport)
    #[serde(default)]
    pub args: Vec<String>,
    /// SSE endpoint URL (for sse transport)
    #[serde(default)]
    pub url: Option<String>,
    /// Connection status
    pub status: McpServerStatus,
    /// Error message (if status is Error)
    pub error: Option<String>,
    /// Tools provided by this server
    pub tools: Vec<McpToolInfo>,
}

/// Summary of all MCP servers
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpStatus {
    /// List of configured MCP servers
    pub servers: Vec<McpServerInfo>,
    /// Total number of tools from all servers
    pub total_tools: usize,
    /// Number of connected servers
    pub connected_count: usize,
}

// ========================
// Tool Management Types
// ========================

/// Tool category for organization in UI
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// File operations: read, write, edit, glob
    FileSystem,
    /// Shell commands: bash, `kill_shell`
    Shell,
    /// Search tools: grep, `web_search`, `web_fetch`
    Search,
    /// Task management: task, `task_output`, `todo_write`
    Task,
    /// User interaction: `ask_user`
    Interactive,
    /// Planning mode: `enter_plan_mode`, `exit_plan_mode`
    Planning,
    /// MCP dynamic tools
    Mcp,
    /// Uncategorized/custom tools
    #[default]
    Other,
}

/// Information about a tool for UI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Tool name (unique identifier)
    pub name: String,
    /// Tool description
    pub description: String,
    /// Whether this is a built-in tool
    pub builtin: bool,
    /// Whether this tool is currently disabled
    pub disabled: bool,
    /// Tool category for UI organization
    pub category: ToolCategory,
    /// Whether this tool requires network access
    #[serde(default)]
    pub requires_network: bool,
}

impl ToolInfo {
    /// Network-enabled tools that support proxy configuration
    const NETWORK_TOOLS: &'static [&'static str] = &["web_search", "web_fetch"];

    /// Get the category for a built-in tool by name
    #[must_use]
    pub fn category_for_builtin(name: &str) -> ToolCategory {
        match name {
            "read" | "write" | "edit" | "glob" | "notebook_edit" => ToolCategory::FileSystem,
            "bash" | "kill_shell" => ToolCategory::Shell,
            "grep" | "web_search" | "web_fetch" => ToolCategory::Search,
            "task" | "task_output" | "todo_write" => ToolCategory::Task,
            "ask_user" => ToolCategory::Interactive,
            "enter_plan_mode" | "exit_plan_mode" => ToolCategory::Planning,
            name if name.starts_with("mcp_") => ToolCategory::Mcp,
            _ => ToolCategory::Other,
        }
    }

    /// Check if a tool is network-enabled
    #[must_use]
    pub fn is_network_tool(name: &str) -> bool {
        Self::NETWORK_TOOLS.contains(&name)
    }
}

/// MCP server management configuration (for add/update/get operations).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerManageConfig {
    /// Server name
    pub name: String,
    /// Transport type
    #[serde(default)]
    pub transport: McpTransportType,
    /// Whether the server is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Command used to start the server (for stdio transport)
    #[serde(default)]
    pub command: String,
    /// Command arguments (for stdio transport)
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables for the server process
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// SSE/HTTP endpoint URL
    #[serde(default)]
    pub url: Option<String>,
    /// API key for authentication
    #[serde(default)]
    pub api_key: Option<String>,
    /// Whether to load API key from keychain
    #[serde(default)]
    pub api_key_from_keychain: bool,
    /// API key authentication method ("bearer" or "header")
    #[serde(default)]
    pub api_key_auth: Option<String>,
    /// Custom header name for API key
    #[serde(default)]
    pub api_key_header: Option<String>,
    /// Custom prefix for API key value
    #[serde(default)]
    pub api_key_prefix: Option<String>,
    /// Named proxy to use for this server
    #[serde(default)]
    pub proxy_name: Option<String>,
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_category_for_builtin() {
        assert_eq!(ToolInfo::category_for_builtin("read"), ToolCategory::FileSystem);
        assert_eq!(ToolInfo::category_for_builtin("write"), ToolCategory::FileSystem);
        assert_eq!(ToolInfo::category_for_builtin("edit"), ToolCategory::FileSystem);
        assert_eq!(ToolInfo::category_for_builtin("glob"), ToolCategory::FileSystem);
        assert_eq!(ToolInfo::category_for_builtin("bash"), ToolCategory::Shell);
        assert_eq!(ToolInfo::category_for_builtin("grep"), ToolCategory::Search);
        assert_eq!(ToolInfo::category_for_builtin("web_search"), ToolCategory::Search);
        assert_eq!(ToolInfo::category_for_builtin("task"), ToolCategory::Task);
        assert_eq!(ToolInfo::category_for_builtin("ask_user"), ToolCategory::Interactive);
        assert_eq!(ToolInfo::category_for_builtin("enter_plan_mode"), ToolCategory::Planning);
        assert_eq!(ToolInfo::category_for_builtin("mcp_something"), ToolCategory::Mcp);
        assert_eq!(ToolInfo::category_for_builtin("unknown_tool"), ToolCategory::Other);
    }

    #[test]
    fn test_is_network_tool() {
        assert!(ToolInfo::is_network_tool("web_search"));
        assert!(ToolInfo::is_network_tool("web_fetch"));
        assert!(!ToolInfo::is_network_tool("read"));
        assert!(!ToolInfo::is_network_tool("bash"));
    }

    #[test]
    fn test_memory_scope_serde() {
        let user = MemoryScope::User;
        let json = serde_json::to_string(&user).unwrap();
        assert_eq!(json, "\"user\"");
        let project: MemoryScope = serde_json::from_str("\"project\"").unwrap();
        assert_eq!(project, MemoryScope::Project);
    }

    #[test]
    fn test_mcp_server_status_default() {
        let status = McpServerStatus::default();
        assert_eq!(status, McpServerStatus::Configured);
    }

    #[test]
    fn test_mcp_transport_type_default() {
        let transport = McpTransportType::default();
        assert_eq!(transport, McpTransportType::Stdio);
    }

    #[test]
    fn test_event_dispatch_mode_default() {
        let mode = EventDispatchMode::default();
        assert!(matches!(mode, EventDispatchMode::Immediate));
    }
}

/// Result of testing an MCP server connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConnectionTestResult {
    /// Whether the connection was successful
    pub success: bool,
    /// Error message (if connection failed)
    pub error: Option<String>,
    /// Tools discovered on the server
    pub tools: Vec<McpToolInfo>,
    /// Protocol version reported by the server
    pub protocol_version: Option<String>,
}

/// Named proxy configuration info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyInfo {
    /// Proxy name (e.g. "global")
    pub name: String,
    /// Proxy configuration
    pub config: forge_config::ProxyConfig,
}
