//! MCP Tool Wrapper and Manager.
//!
//! Wraps MCP tools to implement the `forge_domain::Tool` trait, allowing them
//! to be used seamlessly with the tool registry.

use crate::auth::OAuthConfig;
use crate::client::{McpClient, McpClientConfig, McpClientError};
use crate::health::CircuitBreaker;
use crate::security::McpSecurity;
use crate::transport::{AuthHeader, ProxyConfig, TransportError};
use crate::types::{ContentBlock, McpTool};
use async_trait::async_trait;
use forge_domain::{ConfirmationLevel, ToolError, ToolOutput};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Wrapper around an MCP tool that implements the `forge_domain::Tool` trait.
pub struct McpToolWrapper {
    /// Server name prefix for the tool.
    server_name: String,
    /// The MCP tool definition.
    tool: McpTool,
    /// Shared MCP client.
    client: Arc<RwLock<McpClient>>,
    /// Cached prefixed name (`server_name:tool_name`).
    prefixed_name: String,
    /// Circuit breaker shared across all tools from the same server.
    circuit_breaker: Arc<Mutex<CircuitBreaker>>,
}

impl McpToolWrapper {
    /// Create a new MCP tool wrapper with an isolated circuit breaker.
    ///
    /// NOTE: This creates a breaker scoped to this single tool. For production
    /// use, prefer [`with_circuit_breaker`](Self::with_circuit_breaker) to share
    /// a breaker across all tools from the same server so that failures on any
    /// tool trip the circuit for the whole server.
    pub fn new(server_name: String, tool: McpTool, client: Arc<RwLock<McpClient>>) -> Self {
        let prefixed_name = format!("mcp__{server_name}__{}", tool.name);
        let circuit_breaker = Arc::new(Mutex::new(CircuitBreaker::new(&server_name)));
        Self { server_name, tool, client, prefixed_name, circuit_breaker }
    }

    /// Create a new MCP tool wrapper with a shared circuit breaker.
    ///
    /// All tools from the same server should share a single breaker so that
    /// failures on any tool trip the circuit for the whole server.
    pub fn with_circuit_breaker(
        server_name: String,
        tool: McpTool,
        client: Arc<RwLock<McpClient>>,
        circuit_breaker: Arc<Mutex<CircuitBreaker>>,
    ) -> Self {
        let prefixed_name = format!("mcp__{server_name}__{}", tool.name);
        Self { server_name, tool, client, prefixed_name, circuit_breaker }
    }

    /// Get the prefixed tool name (`mcp__server_name__tool_name`).
    #[must_use]
    pub fn prefixed_name(&self) -> &str {
        &self.prefixed_name
    }

    /// Get the server name this tool belongs to.
    #[allow(dead_code)]
    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Convert MCP content blocks to string output.
    fn format_content(content: &[ContentBlock]) -> String {
        content
            .iter()
            .map(|block| match block {
                ContentBlock::Text { text } => text.clone(),
                ContentBlock::Image { data, mime_type } => {
                    format!("[Image: {mime_type} ({} bytes)]", data.len())
                }
                ContentBlock::Resource { resource } => resource
                    .text
                    .as_ref()
                    .map_or_else(|| format!("[Resource: {}]", resource.uri), Clone::clone),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[async_trait]
impl forge_domain::Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.prefixed_name
    }

    fn description(&self) -> &str {
        self.tool.description.as_deref().unwrap_or("MCP tool (no description)")
    }

    fn parameters_schema(&self) -> Value {
        self.tool.input_schema.clone()
    }

    fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
        ConfirmationLevel::Once
    }

    async fn execute(
        &self,
        params: Value,
        _ctx: &dyn forge_domain::ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        // Validate parameters using MCP security
        let mcp_security = McpSecurity::default();
        if let Some(obj) = params.as_object() {
            for (key, value) in obj {
                if let Err(e) = mcp_security.validate_json_param(key, value) {
                    tracing::warn!(
                        tool = %self.prefixed_name,
                        param = %key,
                        error = %e,
                        "MCP parameter validation failed"
                    );
                    return Err(ToolError::ExecutionFailed(format!(
                        "Parameter validation failed for '{key}': {e}"
                    )));
                }
            }
        }

        // Convert params to HashMap
        let arguments: HashMap<String, Value> =
            params.as_object().map_or_else(HashMap::new, |obj| {
                obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            });

        // Check circuit breaker before attempting the remote call
        {
            let mut cb = self.circuit_breaker.lock();
            if let Err(msg) = cb.allow_request() {
                return Err(ToolError::ExecutionFailed(msg));
            }
        }

        let mut client = self.client.write().await;
        let result = client.call_tool(&self.tool.name, arguments.clone()).await;
        let result = match result {
            Ok(ok) => Ok(ok),
            Err(err) => {
                let should_retry = matches!(
                    err,
                    McpClientError::NotConnected
                        | McpClientError::NotInitialized
                        | McpClientError::Transport(
                            TransportError::NotConnected | TransportError::Timeout
                        )
                );
                if should_retry {
                    tracing::warn!(
                        tool = %self.prefixed_name,
                        error = %err,
                        "MCP tool call failed; reconnecting and retrying once"
                    );
                    let _ = client.disconnect().await;
                    if let Err(e) = client.connect().await {
                        Err(e)
                    } else if let Err(e) = client.initialize().await {
                        Err(e)
                    } else {
                        client.call_tool(&self.tool.name, arguments).await
                    }
                } else {
                    Err(err)
                }
            }
        };

        match result {
            Ok(result) => {
                self.circuit_breaker.lock().record_success();

                let content = Self::format_content(&result.content);
                if result.is_error {
                    Ok(ToolOutput::error(content))
                } else {
                    Ok(ToolOutput::success(content))
                }
            }
            Err(e) => {
                self.circuit_breaker.lock().record_failure();

                let error_msg = match e {
                    McpClientError::Transport(te) => format!("Transport error: {te}"),
                    McpClientError::ServerError { code, message } => {
                        format!("Server error {code}: {message}")
                    }
                    McpClientError::ProtocolError(msg) => format!("Protocol error: {msg}"),
                    McpClientError::NotConnected => "Not connected to MCP server".to_string(),
                    McpClientError::NotInitialized => "MCP client not initialized".to_string(),
                    McpClientError::SerializationError(se) => {
                        format!("Serialization error: {se}")
                    }
                };
                Err(ToolError::ExecutionFailed(error_msg))
            }
        }
    }
}

// ============================================================================
// MCP Server Configuration
// ============================================================================

/// MCP transport type.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum McpTransportType {
    /// Stdio transport (subprocess communication).
    #[default]
    Stdio,
    /// SSE transport (HTTP Server-Sent Events, legacy).
    Sse,
    /// Streamable HTTP transport (MCP 2025-11-25 spec).
    #[serde(alias = "streamable_http")]
    StreamableHttp,
}

/// SSE API key authentication mode.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyAuth {
    /// `Authorization: Bearer <token>`.
    #[default]
    Bearer,
    /// Custom header (e.g. `x-api-key: <token>`).
    Header,
}

/// MCP server configuration for loading from config file.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct McpServerConfig {
    /// Server name (used as prefix for tool names).
    pub name: String,
    /// Transport type (stdio or sse).
    #[serde(default)]
    pub transport: McpTransportType,
    /// Command to execute (for stdio transport).
    #[serde(default)]
    pub command: String,
    /// Command arguments (for stdio transport).
    #[serde(default)]
    pub args: Vec<String>,
    /// SSE endpoint URL (for sse transport).
    #[serde(default)]
    pub url: Option<String>,
    /// API key for authentication (for sse transport) - plain text (not recommended).
    #[serde(default)]
    pub api_key: Option<String>,
    /// Read API key from system keychain (recommended for Desktop).
    #[serde(default)]
    pub api_key_from_keychain: bool,
    /// SSE API key authentication mode.
    #[serde(default)]
    pub api_key_auth: ApiKeyAuth,
    /// API key header name (only used when `api_key_auth = "header"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_header: Option<String>,
    /// API key header value prefix (only used when `api_key_auth = "header"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_prefix: Option<String>,
    /// Environment variables (for stdio transport).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Whether this server is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Name of the proxy to use (references a named proxy object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_name: Option<String>,
    /// OAuth 2.1 configuration (for servers requiring OAuth authentication).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OAuthConfig>,
}

impl McpServerConfig {
    /// Validate the server configuration.
    ///
    /// # Errors
    /// Returns an error string if the configuration is invalid.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.name.contains("__") {
            return Err(format!(
                "Server name '{}' cannot contain '__' (double underscore) \
                 as it conflicts with MCP tool naming convention",
                self.name
            ));
        }

        if self.name.is_empty() {
            return Err("Server name cannot be empty".to_string());
        }

        if self.name.starts_with("mcp__") {
            return Err(format!("Server name '{}' cannot start with 'mcp__' prefix", self.name));
        }

        match self.transport {
            McpTransportType::Stdio => {
                if self.command.is_empty() {
                    return Err("Server command cannot be empty for stdio transport".to_string());
                }
            }
            McpTransportType::Sse => {
                if self.url.is_none()
                    || self.url.as_ref().map_or(true, std::string::String::is_empty)
                {
                    return Err("Server URL is required for SSE transport".to_string());
                }
                if let Some(ref url) = self.url {
                    if !url.starts_with("http://") && !url.starts_with("https://") {
                        return Err(format!(
                            "Invalid SSE URL '{url}': must start with http:// or https://"
                        ));
                    }
                }
            }
            McpTransportType::StreamableHttp => {
                if self.url.is_none()
                    || self.url.as_ref().map_or(true, std::string::String::is_empty)
                {
                    return Err("Server URL is required for Streamable HTTP transport".to_string());
                }
                if let Some(ref url) = self.url {
                    if !url.starts_with("http://") && !url.starts_with("https://") {
                        return Err(format!(
                            "Invalid Streamable HTTP URL '{url}': must start with http:// or https://"
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Check if this is an SSE transport config.
    #[must_use]
    pub fn is_sse(&self) -> bool {
        self.transport == McpTransportType::Sse
    }

    /// Check if this is a stdio transport config.
    #[must_use]
    pub fn is_stdio(&self) -> bool {
        self.transport == McpTransportType::Stdio
    }
}

const fn default_true() -> bool {
    true
}

fn build_auth_header(config: &McpServerConfig, api_key: &str) -> AuthHeader {
    let token = normalize_token(api_key);
    match config.api_key_auth {
        ApiKeyAuth::Bearer => AuthHeader::bearer(token),
        ApiKeyAuth::Header => {
            let header = config.api_key_header.clone().unwrap_or_else(|| "x-api-key".to_string());
            let prefix = config.api_key_prefix.clone().unwrap_or_default();
            AuthHeader { name: header, value: format!("{prefix}{token}") }
        }
    }
}

fn normalize_token(input: &str) -> String {
    let trimmed = input.trim();
    let mut parts = trimmed.split_whitespace();
    if let Some(first) = parts.next() {
        if first.eq_ignore_ascii_case("bearer") {
            if let Some(token) = parts.next() {
                return token.to_string();
            }
        }
    }
    trimmed.to_string()
}

// ============================================================================
// MCP Configuration File
// ============================================================================

/// MCP configuration file format.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, Default)]
pub struct McpConfig {
    /// Global proxy configuration for all MCP servers.
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
    /// MCP servers.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

impl McpConfig {
    /// Load configuration from a file.
    ///
    /// # Errors
    /// Returns an error string if the file cannot be read or parsed.
    pub fn load_from_file(path: &std::path::Path) -> std::result::Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read MCP config file: {e}"))?;

        if path.extension().is_some_and(|e| e == "json") {
            serde_json::from_str(&content)
                .map_err(|e| format!("Failed to parse MCP config as JSON: {e}"))
        } else {
            toml::from_str(&content).map_err(|e| format!("Failed to parse MCP config as TOML: {e}"))
        }
    }

    /// Get enabled servers.
    pub fn enabled_servers(&self) -> impl Iterator<Item = &McpServerConfig> {
        self.servers.iter().filter(|s| s.enabled)
    }
}

// ============================================================================
// MCP Manager
// ============================================================================

/// MCP Manager for handling multiple MCP servers.
pub struct McpManager {
    /// Connected clients.
    clients: HashMap<String, Arc<RwLock<McpClient>>>,
    /// Global proxy configuration.
    global_proxy: Option<ProxyConfig>,
}

impl McpManager {
    /// Create a new MCP manager.
    #[must_use]
    pub fn new() -> Self {
        Self { clients: HashMap::new(), global_proxy: None }
    }

    /// Create a new MCP manager with global proxy configuration.
    #[must_use]
    pub fn with_proxy(proxy: Option<ProxyConfig>) -> Self {
        Self { clients: HashMap::new(), global_proxy: proxy }
    }

    /// Set global proxy configuration.
    pub fn set_global_proxy(&mut self, proxy: Option<ProxyConfig>) {
        self.global_proxy = proxy;
    }

    /// Resolve effective proxy configuration for a server based on `proxy_name`.
    fn resolve_proxy_by_name(&self, proxy_name: Option<&str>) -> Option<ProxyConfig> {
        match proxy_name {
            Some("global") => self.global_proxy.clone(),
            Some(_other) => None,
            None => None,
        }
    }

    /// Connect to an MCP server.
    ///
    /// # Errors
    /// Returns an error string if the connection fails.
    pub async fn connect(&mut self, config: &McpServerConfig) -> std::result::Result<(), String> {
        config.validate()?;

        let client_config = match config.transport {
            McpTransportType::Stdio => McpClientConfig::stdio_with_env(
                &config.command,
                config.args.clone(),
                config.env.clone(),
            ),
            McpTransportType::Sse => {
                let url = config
                    .url
                    .as_ref()
                    .ok_or_else(|| "SSE URL is required for SSE transport".to_string())?;

                let proxy = self.resolve_proxy_by_name(config.proxy_name.as_deref());

                // Use api_key directly (keychain integration removed in this crate)
                let api_key = config.api_key.clone();
                let auth_header = api_key.as_ref().map(|k| build_auth_header(config, k));

                McpClientConfig::sse_with_proxy(url, auth_header, proxy)
            }
            McpTransportType::StreamableHttp => {
                let url = config
                    .url
                    .as_ref()
                    .ok_or_else(|| "URL is required for Streamable HTTP transport".to_string())?;

                let proxy = self.resolve_proxy_by_name(config.proxy_name.as_deref());

                let api_key = config.api_key.clone();
                let auth_header = api_key.as_ref().map(|k| build_auth_header(config, k));

                McpClientConfig::streamable_http(url, auth_header, proxy)
            }
        };

        let mut client = McpClient::new(client_config);

        let primary_result = async {
            client.connect().await.map_err(|e| e.to_string())?;
            client.initialize().await.map_err(|e| e.to_string())?;
            Ok::<(), String>(())
        }
        .await;

        if primary_result.is_ok() {
            self.clients.insert(config.name.clone(), Arc::new(RwLock::new(client)));
            return Ok(());
        }

        let primary_error = primary_result.err().unwrap_or_else(|| "unknown error".to_string());
        let can_stdio_fallback =
            matches!(config.transport, McpTransportType::Sse | McpTransportType::StreamableHttp)
                && !config.command.is_empty();
        if !can_stdio_fallback {
            return Err(format!(
                "Failed to connect to MCP server '{}': {}",
                config.name, primary_error
            ));
        }

        tracing::warn!(
            server = %config.name,
            from = %format!("{:?}", config.transport),
            to = "stdio",
            reason = %primary_error,
            "MCP transport fallback activated"
        );

        let fallback_cfg = McpClientConfig::stdio_with_env(
            &config.command,
            config.args.clone(),
            config.env.clone(),
        );
        let mut fallback_client = McpClient::new(fallback_cfg);
        let transport_name = format!("{:?}", config.transport);
        fallback_client.connect().await.map_err(|e| {
            format!(
                "Failed to connect to MCP server '{}' (primary {} failed: {}; fallback stdio connect failed: {})",
                config.name, transport_name, primary_error, e
            )
        })?;
        fallback_client.initialize().await.map_err(|e| {
            format!(
                "Failed to initialize MCP server '{}' (primary {} failed: {}; fallback stdio init failed: {})",
                config.name, transport_name, primary_error, e
            )
        })?;

        self.clients.insert(config.name.clone(), Arc::new(RwLock::new(fallback_client)));
        Ok(())
    }

    /// Get all tools from all connected servers.
    pub async fn list_all_tools(&mut self) -> Vec<(String, Vec<McpTool>)> {
        let mut all_tools = Vec::new();

        for (name, client) in &self.clients {
            let mut client = client.write().await;
            match client.list_tools().await {
                Ok(tools) => {
                    all_tools.push((name.clone(), tools));
                }
                Err(e) => {
                    tracing::warn!("Failed to list tools from MCP server '{}': {}", name, e);
                }
            }
        }

        all_tools
    }

    /// Create tool wrappers for all MCP tools.
    pub async fn create_tool_wrappers(&mut self) -> Vec<Arc<dyn forge_domain::Tool>> {
        let mut wrappers: Vec<Arc<dyn forge_domain::Tool>> = Vec::new();

        for (server_name, tools) in self.list_all_tools().await {
            if let Some(client) = self.clients.get(&server_name) {
                let cb = Arc::new(Mutex::new(CircuitBreaker::new(&server_name)));
                for tool in tools {
                    let wrapper = McpToolWrapper::with_circuit_breaker(
                        server_name.clone(),
                        tool,
                        client.clone(),
                        cb.clone(),
                    );
                    wrappers.push(Arc::new(wrapper));
                }
            }
        }

        wrappers
    }

    /// Get a client by name.
    #[must_use]
    pub fn get_client(&self, name: &str) -> Option<&Arc<RwLock<McpClient>>> {
        self.clients.get(name)
    }

    /// Disconnect all clients.
    #[allow(clippy::unused_async)]
    pub async fn disconnect_all(&mut self) {
        self.clients.clear();
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_config_parse() {
        let toml_content = r#"
[[servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
enabled = true

[[servers]]
name = "git"
command = "uvx"
args = ["mcp-server-git"]
enabled = false
"#;

        let config: McpConfig = toml::from_str(toml_content).expect("parse TOML");
        assert_eq!(config.servers.len(), 2);
        assert_eq!(config.servers[0].name, "filesystem");
        assert!(config.servers[0].enabled);
        assert_eq!(config.servers[1].name, "git");
        assert!(!config.servers[1].enabled);

        let enabled: Vec<_> = config.enabled_servers().collect();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name, "filesystem");
    }

    #[test]
    fn test_prefixed_name() {
        let tool = McpTool {
            name: "read_file".to_string(),
            description: Some("Read a file".to_string()),
            input_schema: serde_json::json!({"type": "object"}),
        };

        let client = Arc::new(RwLock::new(McpClient::new(McpClientConfig::default())));
        let wrapper = McpToolWrapper::new("filesystem".to_string(), tool, client);

        assert_eq!(wrapper.prefixed_name(), "mcp__filesystem__read_file");
        assert_eq!(forge_domain::Tool::name(&wrapper), "mcp__filesystem__read_file");
    }

    #[test]
    fn test_server_config_validation_valid() {
        let config = McpServerConfig {
            name: "filesystem".to_string(),
            transport: McpTransportType::Stdio,
            command: "npx".to_string(),
            args: vec![],
            url: None,
            api_key: None,
            api_key_from_keychain: false,
            api_key_auth: ApiKeyAuth::Bearer,
            api_key_header: None,
            api_key_prefix: None,
            env: HashMap::new(),
            enabled: true,
            proxy_name: None,
            oauth: None,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_server_config_validation_double_underscore() {
        let config = McpServerConfig {
            name: "bad__name".to_string(),
            transport: McpTransportType::Stdio,
            command: "npx".to_string(),
            args: vec![],
            url: None,
            api_key: None,
            api_key_from_keychain: false,
            api_key_auth: ApiKeyAuth::Bearer,
            api_key_header: None,
            api_key_prefix: None,
            env: HashMap::new(),
            enabled: true,
            proxy_name: None,
            oauth: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.expect_err("should fail").contains("__"));
    }

    #[test]
    fn test_server_config_validation_empty_name() {
        let config = McpServerConfig {
            name: String::new(),
            transport: McpTransportType::Stdio,
            command: "npx".to_string(),
            args: vec![],
            url: None,
            api_key: None,
            api_key_from_keychain: false,
            api_key_auth: ApiKeyAuth::Bearer,
            api_key_header: None,
            api_key_prefix: None,
            env: HashMap::new(),
            enabled: true,
            proxy_name: None,
            oauth: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.expect_err("should fail").contains("empty"));
    }

    #[test]
    fn test_server_config_validation_sse_valid() {
        let config = McpServerConfig {
            name: "bocha".to_string(),
            transport: McpTransportType::Sse,
            command: String::new(),
            args: vec![],
            url: Some("https://mcp.bochaai.com/sse".to_string()),
            api_key: Some("test-api-key".to_string()),
            api_key_from_keychain: false,
            api_key_auth: ApiKeyAuth::Bearer,
            api_key_header: None,
            api_key_prefix: None,
            env: HashMap::new(),
            enabled: true,
            proxy_name: None,
            oauth: None,
        };
        assert!(config.validate().is_ok());
        assert!(config.is_sse());
        assert!(!config.is_stdio());
    }

    #[test]
    fn test_server_config_validation_sse_no_url() {
        let config = McpServerConfig {
            name: "bocha".to_string(),
            transport: McpTransportType::Sse,
            command: String::new(),
            args: vec![],
            url: None,
            api_key: None,
            api_key_from_keychain: false,
            api_key_auth: ApiKeyAuth::Bearer,
            api_key_header: None,
            api_key_prefix: None,
            env: HashMap::new(),
            enabled: true,
            proxy_name: None,
            oauth: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.expect_err("should fail").contains("URL"));
    }

    #[test]
    fn test_server_config_validation_streamable_http_valid() {
        let config = McpServerConfig {
            name: "remote".to_string(),
            transport: McpTransportType::StreamableHttp,
            command: String::new(),
            args: vec![],
            url: Some("https://mcp.example.com/mcp".to_string()),
            api_key: Some("test-key".to_string()),
            api_key_from_keychain: false,
            api_key_auth: ApiKeyAuth::Bearer,
            api_key_header: None,
            api_key_prefix: None,
            env: HashMap::new(),
            enabled: true,
            proxy_name: None,
            oauth: None,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_mcp_config_parse_streamable_http() {
        let toml_content = r#"
[[servers]]
name = "remote-mcp"
transport = "streamablehttp"
url = "https://mcp.example.com/mcp"
api_key = "sk-test"
"#;
        let config: McpConfig = toml::from_str(toml_content).expect("parse TOML");
        assert_eq!(config.servers.len(), 1);
        assert_eq!(config.servers[0].transport, McpTransportType::StreamableHttp);
        assert_eq!(config.servers[0].url, Some("https://mcp.example.com/mcp".to_string()));
    }

    #[test]
    fn test_normalize_token_plain() {
        assert_eq!(normalize_token("my-token"), "my-token");
    }

    #[test]
    fn test_normalize_token_bearer_prefix() {
        assert_eq!(normalize_token("Bearer my-token"), "my-token");
        assert_eq!(normalize_token("bearer my-token"), "my-token");
    }

    #[test]
    fn test_normalize_token_whitespace() {
        assert_eq!(normalize_token("  my-token  "), "my-token");
    }
}
