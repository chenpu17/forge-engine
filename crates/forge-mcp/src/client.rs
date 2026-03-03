//! MCP Client implementation.
//!
//! Provides a high-level client for interacting with MCP servers.

use crate::transport::{
    AuthHeader, McpTransport, ProxyConfig, SseTransport, StdioTransport, StreamableHttpTransport,
    TransportError,
};
use crate::types::{
    methods, ClientCapabilities, ClientInfo, InitializeParams, InitializeResult, JsonRpcError,
    JsonRpcRequest, McpTool, PromptGetParams, PromptGetResult, PromptListResult,
    ResourceListResult, ResourceReadParams, ResourceReadResult, ToolCallParams, ToolCallResult,
    ToolListResult, MCP_VERSION,
};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

/// MCP client error types.
#[derive(Debug, Error)]
pub enum McpClientError {
    /// Transport error.
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),

    /// Server returned an error.
    #[error("Server error: {code} - {message}")]
    ServerError {
        /// Error code.
        code: i32,
        /// Error message.
        message: String,
    },

    /// Protocol error.
    #[error("Protocol error: {0}")]
    ProtocolError(String),

    /// Not connected.
    #[error("Not connected to server")]
    NotConnected,

    /// Not initialized.
    #[error("Client not initialized")]
    NotInitialized,

    /// Serialization error.
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
}

impl From<JsonRpcError> for McpClientError {
    fn from(err: JsonRpcError) -> Self {
        Self::ServerError { code: err.code, message: err.message }
    }
}

/// Result type for MCP client operations.
pub type McpClientResult<T> = std::result::Result<T, McpClientError>;

/// Transport type for MCP client.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum McpTransportType {
    /// Stdio transport (subprocess communication).
    #[default]
    Stdio,
    /// SSE transport (HTTP Server-Sent Events).
    Sse,
    /// Streamable HTTP transport (MCP 2025-11-25 spec).
    StreamableHttp,
}

/// MCP client configuration.
#[derive(Debug, Clone)]
pub struct McpClientConfig {
    /// Transport type.
    pub transport_type: McpTransportType,
    /// Server command to execute (for stdio transport).
    pub command: String,
    /// Command arguments (for stdio transport).
    pub args: Vec<String>,
    /// Environment variables for the subprocess (for stdio transport).
    pub env: HashMap<String, String>,
    /// SSE endpoint URL (for sse transport).
    pub url: Option<String>,
    /// Authentication header for SSE/message endpoint requests.
    pub auth_header: Option<AuthHeader>,
    /// Proxy configuration (for sse transport).
    pub proxy: Option<ProxyConfig>,
    /// Client name.
    pub client_name: String,
    /// Client version.
    pub client_version: String,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for McpClientConfig {
    fn default() -> Self {
        Self {
            transport_type: McpTransportType::Stdio,
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            auth_header: None,
            proxy: None,
            client_name: "forge".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            timeout_secs: 30,
        }
    }
}

impl McpClientConfig {
    /// Create a new config for a stdio MCP server.
    pub fn stdio(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            transport_type: McpTransportType::Stdio,
            command: command.into(),
            args,
            ..Default::default()
        }
    }

    /// Create a new config for a stdio MCP server with environment variables.
    pub fn stdio_with_env(
        command: impl Into<String>,
        args: Vec<String>,
        env: HashMap<String, String>,
    ) -> Self {
        Self {
            transport_type: McpTransportType::Stdio,
            command: command.into(),
            args,
            env,
            ..Default::default()
        }
    }

    /// Create a new config for an SSE MCP server.
    pub fn sse(url: impl Into<String>) -> Self {
        Self { transport_type: McpTransportType::Sse, url: Some(url.into()), ..Default::default() }
    }

    /// Create a new config for an SSE MCP server with API key authentication.
    pub fn sse_with_auth(url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            transport_type: McpTransportType::Sse,
            url: Some(url.into()),
            auth_header: Some(AuthHeader::bearer(api_key.into())),
            ..Default::default()
        }
    }

    /// Create a new config for an SSE MCP server with proxy configuration.
    pub fn sse_with_proxy(
        url: impl Into<String>,
        auth_header: Option<AuthHeader>,
        proxy: Option<ProxyConfig>,
    ) -> Self {
        Self {
            transport_type: McpTransportType::Sse,
            url: Some(url.into()),
            auth_header,
            proxy,
            ..Default::default()
        }
    }

    /// Create a new config for a Streamable HTTP MCP server.
    pub fn streamable_http(
        url: impl Into<String>,
        auth_header: Option<AuthHeader>,
        proxy: Option<ProxyConfig>,
    ) -> Self {
        Self {
            transport_type: McpTransportType::StreamableHttp,
            url: Some(url.into()),
            auth_header,
            proxy,
            ..Default::default()
        }
    }
}

/// MCP client state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientState {
    /// Not connected.
    Disconnected,
    /// Connected but not initialized.
    Connected,
    /// Fully initialized and ready.
    Ready,
}

/// MCP Client.
pub struct McpClient {
    /// Configuration.
    config: McpClientConfig,
    /// Transport.
    transport: Option<Arc<RwLock<Box<dyn McpTransport>>>>,
    /// Current state.
    state: ClientState,
    /// Server info (after initialization).
    server_info: Option<InitializeResult>,
    /// Cached tools list.
    cached_tools: Option<Vec<McpTool>>,
}

impl McpClient {
    /// Create a new MCP client.
    #[must_use]
    pub fn new(config: McpClientConfig) -> Self {
        Self {
            config,
            transport: None,
            state: ClientState::Disconnected,
            server_info: None,
            cached_tools: None,
        }
    }

    /// Connect to the MCP server.
    ///
    /// # Errors
    /// Returns `McpClientError` if the connection fails.
    pub async fn connect(&mut self) -> McpClientResult<()> {
        if self.state != ClientState::Disconnected {
            return Ok(());
        }

        let transport: Box<dyn McpTransport> = match self.config.transport_type {
            McpTransportType::Stdio => {
                let args: Vec<&str> =
                    self.config.args.iter().map(std::string::String::as_str).collect();
                let transport =
                    StdioTransport::new_with_env(&self.config.command, &args, &self.config.env)?;
                Box::new(transport)
            }
            McpTransportType::Sse => {
                let url = self.config.url.as_ref().ok_or_else(|| {
                    TransportError::SseError("SSE URL not configured".to_string())
                })?;
                let transport = SseTransport::connect_with_proxy(
                    url,
                    self.config.auth_header.clone(),
                    self.config.proxy.as_ref(),
                )
                .await?;
                Box::new(transport)
            }
            McpTransportType::StreamableHttp => {
                let url = self.config.url.as_ref().ok_or_else(|| {
                    TransportError::HttpError("Streamable HTTP URL not configured".to_string())
                })?;
                let transport = StreamableHttpTransport::connect(
                    url,
                    self.config.auth_header.clone(),
                    self.config.proxy.as_ref(),
                )
                .await?;
                Box::new(transport)
            }
        };

        self.transport = Some(Arc::new(RwLock::new(transport)));
        self.state = ClientState::Connected;

        Ok(())
    }

    /// Initialize the MCP session.
    ///
    /// # Errors
    /// Returns `McpClientError` if initialization fails.
    pub async fn initialize(&mut self) -> McpClientResult<InitializeResult> {
        if self.state == ClientState::Disconnected {
            return Err(McpClientError::NotConnected);
        }

        let transport = self.transport.as_ref().ok_or(McpClientError::NotConnected)?;

        let params = InitializeParams {
            protocol_version: MCP_VERSION.to_string(),
            capabilities: ClientCapabilities::default(),
            client_info: ClientInfo {
                name: self.config.client_name.clone(),
                version: self.config.client_version.clone(),
            },
        };

        let request =
            JsonRpcRequest::new(0i64, methods::INITIALIZE, Some(serde_json::to_value(&params)?));

        let response = transport.read().await.request(request).await?;

        if let Some(err) = response.error {
            return Err(err.into());
        }

        let result: InitializeResult = response
            .result
            .ok_or_else(|| McpClientError::ProtocolError("No result in response".to_string()))
            .and_then(|v| serde_json::from_value(v).map_err(McpClientError::from))?;

        self.server_info = Some(result.clone());

        transport.read().await.notify(methods::INITIALIZED, None).await?;

        self.state = ClientState::Ready;

        Ok(result)
    }

    /// List available tools.
    ///
    /// # Errors
    /// Returns `McpClientError` if the request fails.
    pub async fn list_tools(&mut self) -> McpClientResult<Vec<McpTool>> {
        self.ensure_ready()?;

        let transport = self.transport.as_ref().ok_or(McpClientError::NotConnected)?;

        let request = JsonRpcRequest::new(0i64, methods::TOOLS_LIST, None);

        let response = transport.read().await.request(request).await?;

        if let Some(err) = response.error {
            return Err(err.into());
        }

        let result: ToolListResult = response
            .result
            .ok_or_else(|| McpClientError::ProtocolError("No result in response".to_string()))
            .and_then(|v| serde_json::from_value(v).map_err(McpClientError::from))?;

        self.cached_tools = Some(result.tools.clone());

        Ok(result.tools)
    }

    /// Call a tool.
    ///
    /// # Errors
    /// Returns `McpClientError` if the tool call fails.
    #[tracing::instrument(name = "mcp_call_tool", skip(self, arguments), fields(tool = %name))]
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: HashMap<String, serde_json::Value>,
    ) -> McpClientResult<ToolCallResult> {
        self.ensure_ready()?;

        let transport = self.transport.as_ref().ok_or(McpClientError::NotConnected)?;

        let params = ToolCallParams { name: name.to_string(), arguments };

        let request =
            JsonRpcRequest::new(0i64, methods::TOOLS_CALL, Some(serde_json::to_value(&params)?));

        let response = transport.read().await.request(request).await?;

        if let Some(err) = response.error {
            return Err(err.into());
        }

        let result: ToolCallResult = response
            .result
            .ok_or_else(|| McpClientError::ProtocolError("No result in response".to_string()))
            .and_then(|v| serde_json::from_value(v).map_err(McpClientError::from))?;

        Ok(result)
    }

    /// List available resources.
    ///
    /// # Errors
    /// Returns `McpClientError` if the request fails.
    pub async fn list_resources(&self) -> McpClientResult<ResourceListResult> {
        self.ensure_ready()?;

        let transport = self.transport.as_ref().ok_or(McpClientError::NotConnected)?;

        let request = JsonRpcRequest::new(0i64, methods::RESOURCES_LIST, None);

        let response = transport.read().await.request(request).await?;

        if let Some(err) = response.error {
            return Err(err.into());
        }

        response
            .result
            .ok_or_else(|| McpClientError::ProtocolError("No result in response".to_string()))
            .and_then(|v| serde_json::from_value(v).map_err(McpClientError::from))
    }

    /// Read a resource.
    ///
    /// # Errors
    /// Returns `McpClientError` if the request fails.
    pub async fn read_resource(&self, uri: &str) -> McpClientResult<ResourceReadResult> {
        self.ensure_ready()?;

        let transport = self.transport.as_ref().ok_or(McpClientError::NotConnected)?;

        let params = ResourceReadParams { uri: uri.to_string() };

        let request = JsonRpcRequest::new(
            0i64,
            methods::RESOURCES_READ,
            Some(serde_json::to_value(&params)?),
        );

        let response = transport.read().await.request(request).await?;

        if let Some(err) = response.error {
            return Err(err.into());
        }

        response
            .result
            .ok_or_else(|| McpClientError::ProtocolError("No result in response".to_string()))
            .and_then(|v| serde_json::from_value(v).map_err(McpClientError::from))
    }

    /// List available prompts.
    ///
    /// # Errors
    /// Returns `McpClientError` if the request fails.
    pub async fn list_prompts(&self) -> McpClientResult<PromptListResult> {
        self.ensure_ready()?;

        let transport = self.transport.as_ref().ok_or(McpClientError::NotConnected)?;

        let request = JsonRpcRequest::new(0i64, methods::PROMPTS_LIST, None);

        let response = transport.read().await.request(request).await?;

        if let Some(err) = response.error {
            return Err(err.into());
        }

        response
            .result
            .ok_or_else(|| McpClientError::ProtocolError("No result in response".to_string()))
            .and_then(|v| serde_json::from_value(v).map_err(McpClientError::from))
    }

    /// Get a prompt.
    ///
    /// # Errors
    /// Returns `McpClientError` if the request fails.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<HashMap<String, String>>,
    ) -> McpClientResult<PromptGetResult> {
        self.ensure_ready()?;

        let transport = self.transport.as_ref().ok_or(McpClientError::NotConnected)?;

        let params = PromptGetParams { name: name.to_string(), arguments };

        let request =
            JsonRpcRequest::new(0i64, methods::PROMPTS_GET, Some(serde_json::to_value(&params)?));

        let response = transport.read().await.request(request).await?;

        if let Some(err) = response.error {
            return Err(err.into());
        }

        response
            .result
            .ok_or_else(|| McpClientError::ProtocolError("No result in response".to_string()))
            .and_then(|v| serde_json::from_value(v).map_err(McpClientError::from))
    }

    /// Get server info (after initialization).
    #[must_use]
    pub const fn server_info(&self) -> Option<&InitializeResult> {
        self.server_info.as_ref()
    }

    /// Get cached tools.
    #[must_use]
    pub fn cached_tools(&self) -> Option<&[McpTool]> {
        self.cached_tools.as_deref()
    }

    /// Get current state.
    #[must_use]
    pub fn state(&self) -> ClientState {
        self.state.clone()
    }

    /// Check if client is ready.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.state == ClientState::Ready
    }

    /// Disconnect from the server.
    ///
    /// # Errors
    /// Returns `McpClientError` if disconnection fails.
    pub async fn disconnect(&mut self) -> McpClientResult<()> {
        if let Some(transport) = self.transport.take() {
            let mut guard = transport.write().await;
            guard.close().await?;
            drop(guard);
        }

        self.state = ClientState::Disconnected;
        self.server_info = None;
        self.cached_tools = None;

        Ok(())
    }

    /// Ensure client is in ready state.
    const fn ensure_ready(&self) -> McpClientResult<()> {
        match self.state {
            ClientState::Disconnected => Err(McpClientError::NotConnected),
            ClientState::Connected => Err(McpClientError::NotInitialized),
            ClientState::Ready => Ok(()),
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Transport will be cleaned up by its own Drop implementation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_config_default() {
        let config = McpClientConfig::default();
        assert_eq!(config.client_name, "forge");
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn test_client_config_stdio() {
        let config = McpClientConfig::stdio("node", vec!["server.js".to_string()]);
        assert_eq!(config.command, "node");
        assert_eq!(config.args, vec!["server.js"]);
    }

    #[test]
    fn test_client_state_initial() {
        let config = McpClientConfig::default();
        let client = McpClient::new(config);
        assert_eq!(client.state(), ClientState::Disconnected);
        assert!(!client.is_ready());
    }

    #[test]
    fn test_mcp_client_error_from_json_rpc_error() {
        let json_rpc_err =
            JsonRpcError { code: -32600, message: "Invalid request".to_string(), data: None };
        let err: McpClientError = json_rpc_err.into();
        match err {
            McpClientError::ServerError { code, message } => {
                assert_eq!(code, -32600);
                assert_eq!(message, "Invalid request");
            }
            _ => panic!("Expected ServerError"),
        }
    }
}
