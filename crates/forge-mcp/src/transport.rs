//! MCP Transport implementations.
//!
//! Provides transport layers for MCP communication:
//! - [`StdioTransport`]: Communication via subprocess stdin/stdout
//! - [`SseTransport`]: HTTP Server-Sent Events for remote MCP servers
//! - [`StreamableHttpTransport`]: Streamable HTTP (MCP 2025-11-25 spec)

use crate::types::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, RequestId, JSONRPC_VERSION};
use futures::StreamExt;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// Transport error types.
#[derive(Debug, Error)]
pub enum TransportError {
    /// Failed to spawn subprocess.
    #[error("Failed to spawn process: {0}")]
    SpawnError(String),

    /// IO error during communication.
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    /// Process has exited.
    #[error("Process has exited")]
    ProcessExited,

    /// Response timeout.
    #[error("Response timeout")]
    Timeout,

    /// Channel closed.
    #[error("Channel closed")]
    ChannelClosed,

    /// Invalid response.
    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    /// HTTP error.
    #[error("HTTP error: {0}")]
    HttpError(String),

    /// SSE connection error.
    #[error("SSE connection error: {0}")]
    SseError(String),

    /// Not connected.
    #[error("Not connected")]
    NotConnected,
}

/// Result type for transport operations.
pub type TransportResult<T> = std::result::Result<T, TransportError>;

/// MCP Transport trait.
#[async_trait::async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a request and wait for response.
    async fn request(&self, request: JsonRpcRequest) -> TransportResult<JsonRpcResponse>;

    /// Send a notification (no response expected).
    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> TransportResult<()>;

    /// Check if transport is connected.
    fn is_connected(&self) -> bool;

    /// Close the transport.
    async fn close(&mut self) -> TransportResult<()>;
}

// ============================================================================
// Proxy Configuration (local, replaces forge_infra::ProxyConfig)
// ============================================================================

/// Proxy mode.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// No proxy.
    #[default]
    None,
    /// Use system proxy settings.
    System,
    /// Use environment variables (`HTTP_PROXY`, `HTTPS_PROXY`).
    Environment,
    /// Manual proxy configuration.
    Manual,
}

/// Proxy configuration for HTTP transports.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ProxyConfig {
    /// Proxy mode.
    #[serde(default)]
    pub mode: ProxyMode,
    /// HTTP proxy URL (for manual mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_url: Option<String>,
    /// HTTPS proxy URL (for manual mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https_url: Option<String>,
    /// No-proxy list (comma-separated hostnames).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_proxy: Option<String>,
}

/// Create a streaming HTTP client with optional proxy configuration.
///
/// This client has no global timeout, suitable for SSE long-lived connections.
fn create_streaming_client(proxy: Option<&ProxyConfig>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder();

    if let Some(proxy_config) = proxy {
        match proxy_config.mode {
            ProxyMode::Manual => {
                if let Some(ref http_url) = proxy_config.http_url {
                    let proxy = reqwest::Proxy::http(http_url)
                        .map_err(|e| format!("Invalid HTTP proxy URL: {e}"))?;
                    builder = builder.proxy(proxy);
                }
                if let Some(ref https_url) = proxy_config.https_url {
                    let proxy = reqwest::Proxy::https(https_url)
                        .map_err(|e| format!("Invalid HTTPS proxy URL: {e}"))?;
                    builder = builder.proxy(proxy);
                }
            }
            ProxyMode::System | ProxyMode::Environment => {
                // reqwest uses system/env proxy by default
            }
            ProxyMode::None => {
                builder = builder.no_proxy();
            }
        }
    }

    builder.build().map_err(|e| format!("Failed to build HTTP client: {e}"))
}

// ============================================================================
// Authentication Header
// ============================================================================

/// Authentication header for SSE/message endpoint requests.
#[derive(Debug, Clone)]
pub struct AuthHeader {
    /// Header name (e.g., "Authorization").
    pub name: String,
    /// Header value (e.g., "Bearer <token>").
    pub value: String,
}

impl AuthHeader {
    /// Create a Bearer authentication header.
    pub fn bearer(token: impl Into<String>) -> Self {
        Self { name: "Authorization".to_string(), value: format!("Bearer {}", token.into()) }
    }
}

// ============================================================================
// Stdio Transport
// ============================================================================

/// Stdio transport for subprocess communication.
pub struct StdioTransport {
    /// Child process.
    child: Option<Child>,
    /// Stdin writer (wrapped in `Arc<Mutex>` for interior mutability).
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    /// Request ID counter.
    request_id: AtomicU64,
    /// Pending requests waiting for response.
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Reader task handle.
    reader_handle: Option<std::thread::JoinHandle<()>>,
    /// Stderr reader handle (consumes stderr to prevent blocking).
    stderr_handle: Option<std::thread::JoinHandle<()>>,
    /// Shutdown signal.
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl StdioTransport {
    /// Environment variables that are safe to pass to MCP subprocesses.
    ///
    /// This allowlist prevents leaking sensitive secrets (API keys, tokens, etc.)
    const SAFE_ENV_VARS: &'static [&'static str] = &[
        // Essential for process execution
        "PATH", "HOME", "USER", "SHELL", "TERM", "LANG", "LC_ALL", "LC_CTYPE",
        // Windows system variables (required for Node.js crypto on Windows)
        "SYSTEMROOT", "WINDIR", "USERPROFILE", "APPDATA", "LOCALAPPDATA",
        "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMDATA", "COMSPEC",
        // Node.js / npm
        "NODE_ENV", "NODE_PATH", "NPM_CONFIG_PREFIX",
        // Python
        "PYTHONPATH", "VIRTUAL_ENV",
        // Rust
        "CARGO_HOME", "RUSTUP_HOME",
        // Common development
        "EDITOR", "VISUAL", "TZ", "TMPDIR", "TEMP", "TMP",
        // XDG directories
        "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR",
        // TLS/SSL certificates
        "SSL_CERT_FILE", "SSL_CERT_DIR", "NODE_EXTRA_CA_CERTS",
        "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE",
    ];

    /// Create a new stdio transport by spawning a subprocess.
    ///
    /// # Errors
    /// Returns `TransportError::SpawnError` if the subprocess cannot be started.
    pub fn new(command: &str, args: &[&str]) -> TransportResult<Self> {
        Self::new_with_env(command, args, &std::collections::HashMap::new())
    }

    /// Create a new stdio transport by spawning a subprocess with custom environment variables.
    ///
    /// For security, this method:
    /// 1. Clears all inherited environment variables
    /// 2. Only passes through a safe allowlist of standard variables
    /// 3. Adds any custom environment variables provided
    ///
    /// # Errors
    /// Returns `TransportError::SpawnError` if the subprocess cannot be started.
    pub fn new_with_env(
        command: &str,
        args: &[&str],
        env: &std::collections::HashMap<String, String>,
    ) -> TransportResult<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

        // Windows: Hide console window for subprocess
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }

        // Clear inherited environment and only pass safe variables
        cmd.env_clear();

        // Add safe environment variables from host
        for var_name in Self::SAFE_ENV_VARS {
            if let Ok(value) = std::env::var(var_name) {
                cmd.env(var_name, value);
            }
        }

        // Add custom environment variables (these override safe defaults)
        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| TransportError::SpawnError(e.to_string()))?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        // Spawn reader thread for stdout
        let reader_handle = stdout.map(|stdout| std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    // Check for shutdown
                    if shutdown_rx.try_recv().is_ok() {
                        break;
                    }

                    match line {
                        Ok(line) if !line.is_empty() => {
                            match serde_json::from_str::<JsonRpcResponse>(&line) {
                                Ok(response) => {
                                    let id = match &response.id {
                                        RequestId::Number(n) => *n,
                                        RequestId::String(s) => match s.parse() {
                                            Ok(n) => n,
                                            Err(_) => {
                                                tracing::warn!("Non-numeric request ID in response: {s}");
                                                continue;
                                            }
                                        },
                                    };

                                    let sender = pending_clone.lock().remove(&id);
                                    if let Some(sender) = sender {
                                        let _ = sender.send(response);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        "Failed to parse MCP response: {} - {}",
                                        e,
                                        line
                                    );
                                }
                            }
                        }
                        Ok(_) => {} // Empty line
                        Err(e) => {
                            tracing::error!("Error reading from MCP process stdout: {}", e);
                            break;
                        }
                    }
                }
            }));

        // Spawn reader thread for stderr (prevents blocking if stderr buffer fills up)
        let stderr_handle = stderr.map(|stderr| std::thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines() {
                    match line {
                        Ok(line) if !line.is_empty() => {
                            tracing::debug!(target: "mcp::stderr", "{}", line);
                        }
                        Ok(_) => {} // Empty line
                        Err(e) => {
                            tracing::trace!("MCP process stderr closed: {}", e);
                            break;
                        }
                    }
                }
            }));

        Ok(Self {
            child: Some(child),
            stdin: Arc::new(Mutex::new(stdin)),
            request_id: AtomicU64::new(1),
            pending,
            reader_handle,
            stderr_handle,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    /// Get the next request ID.
    fn next_id(&self) -> i64 {
        i64::try_from(self.request_id.fetch_add(1, Ordering::SeqCst)).unwrap_or(i64::MAX)
    }

    /// Write a message to stdin.
    fn write_to_stdin(&self, json: &str) -> TransportResult<()> {
        let mut stdin_guard = self.stdin.lock();

        if let Some(ref mut stdin) = *stdin_guard {
            writeln!(stdin, "{json}")?;
            stdin.flush()?;
            Ok(())
        } else {
            Err(TransportError::ProcessExited)
        }
    }
}

#[async_trait::async_trait]
impl McpTransport for StdioTransport {
    async fn request(&self, mut request: JsonRpcRequest) -> TransportResult<JsonRpcResponse> {
        let id = self.next_id();
        request.id = id.into();
        request.jsonrpc = JSONRPC_VERSION.to_string();

        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock();
            pending.insert(id, tx);
        }

        let json = serde_json::to_string(&JsonRpcMessage::Request(request))?;
        self.write_to_stdin(&json)?;

        match tokio::time::timeout(std::time::Duration::from_secs(30), async {
            rx.await.map_err(|_| TransportError::ChannelClosed)
        })
        .await
        {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                self.pending.lock().remove(&id);
                Err(TransportError::Timeout)
            }
        }
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> TransportResult<()> {
        let notification = JsonRpcNotification::new(method, params);
        let json = serde_json::to_string(&JsonRpcMessage::Notification(notification))?;
        self.write_to_stdin(&json)
    }

    fn is_connected(&self) -> bool {
        self.child.is_some() && self.stdin.lock().is_some()
    }

    async fn close(&mut self) -> TransportResult<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }

        *self.stdin.lock() = None;

        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }

        if let Some(handle) = self.stderr_handle.take() {
            let _ = handle.join();
        }

        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
    }
}

// ============================================================================
// SSE Transport
// ============================================================================

/// Maximum buffer size for SSE data (1MB).
const SSE_MAX_BUFFER_SIZE: usize = 1024 * 1024;

/// Maximum event data size (512KB).
const SSE_MAX_EVENT_DATA_SIZE: usize = 512 * 1024;

/// SSE transport for remote MCP server communication.
///
/// This transport connects to MCP servers over HTTP using Server-Sent Events (SSE).
/// The protocol works as follows:
/// 1. Client connects to the SSE endpoint (GET with Accept: text/event-stream)
/// 2. Server sends an "endpoint" event with the URL to POST messages to
/// 3. Client POSTs JSON-RPC requests to that endpoint
/// 4. Server sends responses via SSE events
///
/// # Security
/// - The endpoint URL received from the server is validated to be same-origin
/// - Buffer sizes are limited to prevent memory exhaustion attacks
pub struct SseTransport {
    /// HTTP client (no global timeout for SSE long connections).
    client: reqwest::Client,
    /// SSE endpoint URL (for initial connection).
    sse_url: String,
    /// Base URL for same-origin validation (scheme + host + port).
    base_url: String,
    /// Message endpoint URL (received from server via "endpoint" event).
    message_endpoint: Arc<tokio::sync::RwLock<Option<String>>>,
    /// Request ID counter.
    request_id: AtomicU64,
    /// Pending requests waiting for response.
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Connection state.
    connected: Arc<AtomicBool>,
    /// SSE reader task handle.
    sse_handle: Option<tokio::task::JoinHandle<()>>,
    /// Shutdown signal.
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
    /// Optional auth header (for SSE and message endpoint requests).
    auth_header: Option<AuthHeader>,
}

impl SseTransport {
    const fn auth_header_name(&self) -> &'static str {
        if self.auth_header.is_some() { "set" } else { "none" }
    }

    /// Extract base URL (scheme + host + port) for same-origin validation.
    fn extract_base_url(url: &str) -> Result<String, TransportError> {
        let parsed = url::Url::parse(url)
            .map_err(|e| TransportError::SseError(format!("Invalid URL: {e}")))?;

        let scheme = parsed.scheme();
        let host = parsed
            .host_str()
            .ok_or_else(|| TransportError::SseError("URL missing host".to_string()))?;

        let base = parsed.port().map_or_else(
            || format!("{scheme}://{host}"),
            |port| format!("{scheme}://{host}:{port}"),
        );

        Ok(base)
    }

    /// Validate that an endpoint URL is same-origin as the SSE URL.
    ///
    /// This prevents SSRF attacks where a malicious server could redirect
    /// requests to arbitrary internal endpoints.
    ///
    /// If the endpoint is a relative path (starts with '/'), it's considered
    /// same-origin by definition.
    fn validate_same_origin(base_url: &str, endpoint_url: &str) -> Result<(), TransportError> {
        if endpoint_url.starts_with('/') {
            return Ok(());
        }

        let endpoint_base = Self::extract_base_url(endpoint_url)?;

        if endpoint_base != base_url {
            return Err(TransportError::SseError(format!(
                "Security: endpoint URL '{endpoint_url}' is not same-origin as SSE URL '{base_url}'. \
                 Cross-origin endpoints are not allowed to prevent SSRF attacks."
            )));
        }

        Ok(())
    }

    /// Create a new SSE transport and connect to the server.
    ///
    /// # Arguments
    /// * `url` - The SSE endpoint URL (e.g., "<https://mcp.example.com/sse>")
    /// * `api_key` - Optional API key for authentication
    ///
    /// # Errors
    /// Returns a `TransportError` if the connection fails.
    #[allow(dead_code)] // Convenience method for backward compatibility
    pub async fn connect(url: &str, api_key: Option<String>) -> TransportResult<Self> {
        let auth_header = api_key.map(AuthHeader::bearer);
        Self::connect_with_proxy(url, auth_header, None).await
    }

    /// Create a new SSE transport with optional proxy configuration.
    ///
    /// # Errors
    /// Returns a `TransportError` if the connection fails.
    pub async fn connect_with_proxy(
        url: &str,
        auth_header: Option<AuthHeader>,
        proxy: Option<&ProxyConfig>,
    ) -> TransportResult<Self> {
        let base_url = Self::extract_base_url(url)?;

        let client = create_streaming_client(proxy).map_err(|e| {
            TransportError::HttpError(format!("Failed to create client: {e}"))
        })?;

        let pending: Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let message_endpoint = Arc::new(tokio::sync::RwLock::new(None));
        let connected = Arc::new(AtomicBool::new(false));
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let mut transport = Self {
            client,
            sse_url: url.to_string(),
            base_url,
            message_endpoint,
            request_id: AtomicU64::new(1),
            pending,
            connected,
            sse_handle: None,
            shutdown_tx: Some(shutdown_tx),
            auth_header,
        };

        transport.start_sse_listener().await?;

        Ok(transport)
    }

    /// Start the SSE event listener.
    #[allow(clippy::too_many_lines)]
    async fn start_sse_listener(&mut self) -> TransportResult<()> {
        let mut request = self
            .client
            .get(&self.sse_url)
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache");

        if let Some(ref auth) = self.auth_header {
            request = request.header(&auth.name, &auth.value);
        }

        tracing::debug!(
            "SSE connecting to {} with auth_header={}",
            self.sse_url,
            self.auth_header_name()
        );

        let response = request
            .send()
            .await
            .map_err(|e| TransportError::SseError(format!("Failed to connect: {e}")))?;

        tracing::debug!("SSE response status: {} for {}", response.status(), self.sse_url);

        if !response.status().is_success() {
            return Err(TransportError::SseError(format!(
                "SSE connection failed: HTTP {} (auth_header={})",
                response.status(),
                self.auth_header_name()
            )));
        }

        let pending = self.pending.clone();
        let message_endpoint = self.message_endpoint.clone();
        let connected = self.connected.clone();
        let base_url = self.base_url.clone();
        let mut shutdown_rx = self
            .shutdown_tx
            .as_ref()
            .ok_or_else(|| {
                TransportError::SseError("Internal: shutdown channel not initialized".into())
            })?
            .subscribe();

        let handle = tokio::spawn(async move {
            let mut stream = response.bytes_stream();
            let mut buffer: Vec<u8> = Vec::new();
            let mut event_type = String::new();
            let mut event_data = String::new();

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        tracing::debug!("SSE reader received shutdown signal");
                        break;
                    }
                    chunk = stream.next() => {
                        match chunk {
                            Some(Ok(bytes)) => {
                                if buffer.len() + bytes.len() > SSE_MAX_BUFFER_SIZE {
                                    tracing::error!(
                                        "SSE buffer exceeded maximum size ({}), disconnecting",
                                        SSE_MAX_BUFFER_SIZE
                                    );
                                    connected.store(false, Ordering::Release);
                                    break;
                                }

                                buffer.extend_from_slice(&bytes);

                                while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                                    let mut line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
                                    if line_bytes.last() == Some(&b'\n') {
                                        line_bytes.pop();
                                    }
                                    if line_bytes.last() == Some(&b'\r') {
                                        line_bytes.pop();
                                    }
                                    let line = String::from_utf8_lossy(&line_bytes);
                                    let line = line.as_ref();

                                    if line.is_empty() {
                                        if !event_data.is_empty() {
                                            let preview = if event_data.len() > 100 {
                                                let mut end = 100;
                                                while end > 0 && !event_data.is_char_boundary(end) {
                                                    end -= 1;
                                                }
                                                &event_data[..end]
                                            } else {
                                                &event_data
                                            };
                                            tracing::debug!(
                                                "SSE event received: type='{}' data='{}'",
                                                event_type,
                                                preview
                                            );
                                            Self::handle_sse_event(
                                                &event_type,
                                                &event_data,
                                                &pending,
                                                &message_endpoint,
                                                &connected,
                                                &base_url,
                                            ).await;
                                        }
                                        event_type.clear();
                                        event_data.clear();
                                    } else if let Some(value) = line.strip_prefix("event:") {
                                        event_type = value.trim().to_string();
                                    } else if let Some(value) = line.strip_prefix("data:") {
                                        let new_data = value.trim();
                                        if event_data.len() + new_data.len() + 1 > SSE_MAX_EVENT_DATA_SIZE {
                                            tracing::warn!(
                                                "SSE event data exceeded maximum size ({}), skipping",
                                                SSE_MAX_EVENT_DATA_SIZE
                                            );
                                            event_data.clear();
                                            event_type.clear();
                                            continue;
                                        }
                                        if !event_data.is_empty() {
                                            event_data.push('\n');
                                        }
                                        event_data.push_str(new_data);
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                tracing::error!("SSE stream error: {}", e);
                                connected.store(false, Ordering::Release);
                                break;
                            }
                            None => {
                                tracing::info!("SSE stream ended");
                                connected.store(false, Ordering::Release);
                                break;
                            }
                        }
                    }
                }
            }
        });

        self.sse_handle = Some(handle);

        // Wait for endpoint to be received (with timeout)
        let endpoint_received = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                if self.message_endpoint.read().await.is_some() {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await;

        match endpoint_received {
            Ok(true) => {
                self.connected.store(true, Ordering::Release);
                tracing::info!("SSE transport connected to {}", self.sse_url);
                Ok(())
            }
            _ => Err(TransportError::SseError(
                "Timeout waiting for endpoint event from server".to_string(),
            )),
        }
    }

    /// Handle an SSE event.
    async fn handle_sse_event(
        event_type: &str,
        event_data: &str,
        pending: &Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcResponse>>>>,
        message_endpoint: &Arc<tokio::sync::RwLock<Option<String>>>,
        connected: &Arc<AtomicBool>,
        base_url: &str,
    ) {
        let handle_message = |payload: &str| match serde_json::from_str::<JsonRpcMessage>(payload) {
            Ok(JsonRpcMessage::Response(response)) => {
                let id = match &response.id {
                    RequestId::Number(n) => *n,
                    RequestId::String(s) => match s.parse() {
                        Ok(n) => n,
                        Err(_) => {
                            tracing::warn!("Non-numeric request ID in SSE response: {s}");
                            return;
                        }
                    },
                };

                let sender = pending.lock().remove(&id);
                if let Some(sender) = sender {
                    let _ = sender.send(response);
                } else {
                    tracing::debug!("Received response for unknown request id {}", id);
                }
            }
            Ok(JsonRpcMessage::Notification(notification)) => {
                if notification.method == "heartbeat" {
                    tracing::debug!("Received SSE heartbeat");
                } else {
                    tracing::debug!("Received SSE notification method='{}'", notification.method);
                }
            }
            Ok(JsonRpcMessage::Request(_)) => {
                tracing::debug!("Ignoring unexpected SSE JSON-RPC request");
            }
            Err(e) => {
                if payload.contains("\"method\":\"heartbeat\"") {
                    tracing::debug!("Received SSE heartbeat (unparsed)");
                    return;
                }
                tracing::warn!("Failed to parse SSE message as JSON-RPC: {}", e);
            }
        };

        match event_type {
            "endpoint" => {
                match Self::validate_same_origin(base_url, event_data) {
                    Ok(()) => {
                        let full_url = if event_data.starts_with('/') {
                            format!("{base_url}{event_data}")
                        } else {
                            event_data.to_string()
                        };
                        tracing::debug!("Received valid endpoint: {} -> {}", event_data, full_url);
                        *message_endpoint.write().await = Some(full_url);
                    }
                    Err(e) => {
                        tracing::error!("Rejected endpoint due to security check: {}", e);
                        connected.store(false, Ordering::Release);
                    }
                }
            }
            "message" | "" => handle_message(event_data),
            "error" => {
                tracing::error!("SSE server error: {}", event_data);
                connected.store(false, Ordering::Release);
            }
            other => {
                tracing::debug!("Ignoring SSE event type: {}", other);
            }
        }
    }

    /// Get the next request ID.
    fn next_id(&self) -> i64 {
        i64::try_from(self.request_id.fetch_add(1, Ordering::SeqCst)).unwrap_or(i64::MAX)
    }
}

#[async_trait::async_trait]
impl McpTransport for SseTransport {
    async fn request(&self, mut request: JsonRpcRequest) -> TransportResult<JsonRpcResponse> {
        if !self.connected.load(Ordering::Acquire) {
            return Err(TransportError::NotConnected);
        }

        let endpoint =
            self.message_endpoint.read().await.clone().ok_or(TransportError::NotConnected)?;

        let id = self.next_id();
        request.id = id.into();
        request.jsonrpc = JSONRPC_VERSION.to_string();

        let (tx, rx) = oneshot::channel();

        {
            let mut pending = self.pending.lock();
            pending.insert(id, tx);
        }

        let json = serde_json::to_string(&JsonRpcMessage::Request(request))?;

        let mut post_request = self
            .client
            .post(&endpoint)
            .timeout(std::time::Duration::from_secs(30))
            .header("Content-Type", "application/json")
            .body(json);

        if let Some(ref auth) = self.auth_header {
            post_request = post_request.header(&auth.name, &auth.value);
        }

        let response = post_request
            .send()
            .await
            .map_err(|e| TransportError::HttpError(format!("Failed to send request: {e}")))?;

        if !response.status().is_success() {
            self.pending.lock().remove(&id);
            return Err(TransportError::HttpError(format!(
                "HTTP error: {} (auth_header={})",
                response.status(),
                self.auth_header_name()
            )));
        }

        match tokio::time::timeout(std::time::Duration::from_secs(30), async {
            rx.await.map_err(|_| TransportError::ChannelClosed)
        })
        .await
        {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                self.pending.lock().remove(&id);
                Err(TransportError::Timeout)
            }
        }
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> TransportResult<()> {
        if !self.connected.load(Ordering::Acquire) {
            return Err(TransportError::NotConnected);
        }

        let endpoint =
            self.message_endpoint.read().await.clone().ok_or(TransportError::NotConnected)?;

        let notification = JsonRpcNotification::new(method, params);
        let json = serde_json::to_string(&JsonRpcMessage::Notification(notification))?;

        let mut post_request = self
            .client
            .post(&endpoint)
            .timeout(std::time::Duration::from_secs(30))
            .header("Content-Type", "application/json")
            .body(json);

        if let Some(ref auth) = self.auth_header {
            post_request = post_request.header(&auth.name, &auth.value);
        }

        post_request.send().await.map_err(|e| {
            TransportError::HttpError(format!("Failed to send notification: {e}"))
        })?;

        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    async fn close(&mut self) -> TransportResult<()> {
        if let Some(ref tx) = self.shutdown_tx {
            let _ = tx.send(());
        }

        if let Some(handle) = self.sse_handle.take() {
            let _ = handle.await;
        }

        self.connected.store(false, Ordering::Release);
        tracing::info!("SSE transport closed");

        Ok(())
    }
}

impl Drop for SseTransport {
    fn drop(&mut self) {
        if let Some(ref tx) = self.shutdown_tx {
            let _ = tx.send(());
        }
        self.connected.store(false, Ordering::Release);
    }
}

// ============================================================================
// Streamable HTTP Transport (MCP 2025-11-25)
// ============================================================================

/// Streamable HTTP transport for MCP 2025-11-25 spec.
///
/// Uses a single HTTP endpoint for all communication:
/// - POST: Send JSON-RPC requests/notifications
/// - GET: Open SSE stream for server-initiated notifications
/// - DELETE: Terminate session
///
/// The server may respond to POST with either:
/// - `application/json`: Single JSON-RPC response
/// - `text/event-stream`: SSE stream with JSON-RPC messages
///
/// Session management uses the `Mcp-Session-Id` header.
pub struct StreamableHttpTransport {
    /// HTTP client.
    client: reqwest::Client,
    /// MCP endpoint URL.
    endpoint: String,
    /// Request ID counter.
    request_id: AtomicU64,
    /// Session ID (received from server on initialize).
    session_id: Arc<tokio::sync::RwLock<Option<String>>>,
    /// Session ID that currently owns the GET stream (if any).
    stream_session_id: Arc<tokio::sync::RwLock<Option<String>>>,
    /// Pending requests waiting for a response delivered over GET stream.
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Background GET stream task handle.
    stream_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// Shutdown signal for background GET stream task.
    shutdown_tx: Arc<tokio::sync::Mutex<Option<tokio::sync::broadcast::Sender<()>>>>,
    /// Connection state.
    connected: Arc<AtomicBool>,
    /// Optional auth header.
    auth_header: Option<AuthHeader>,
}

impl StreamableHttpTransport {
    /// Create a new Streamable HTTP transport.
    ///
    /// **Note:** This is a lazy connection -- it builds the HTTP client struct
    /// without making any network request. `is_connected()` returns `true`
    /// immediately. The actual server connection is validated on the first
    /// `request()` call.
    ///
    /// # Errors
    /// Returns a `TransportError` if the HTTP client cannot be created.
    #[allow(clippy::unused_async)]
    pub async fn connect(
        endpoint: &str,
        auth_header: Option<AuthHeader>,
        proxy: Option<&ProxyConfig>,
    ) -> TransportResult<Self> {
        let client = create_streaming_client(proxy).map_err(|e| {
            TransportError::HttpError(format!("Failed to create client: {e}"))
        })?;

        let transport = Self {
            client,
            endpoint: endpoint.to_string(),
            request_id: AtomicU64::new(1),
            session_id: Arc::new(tokio::sync::RwLock::new(None)),
            stream_session_id: Arc::new(tokio::sync::RwLock::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            stream_handle: Arc::new(tokio::sync::Mutex::new(None)),
            shutdown_tx: Arc::new(tokio::sync::Mutex::new(None)),
            connected: Arc::new(AtomicBool::new(true)),
            auth_header,
        };

        tracing::info!("Streamable HTTP transport created for {}", endpoint);
        Ok(transport)
    }

    /// Get the next request ID.
    fn next_id(&self) -> i64 {
        i64::try_from(self.request_id.fetch_add(1, Ordering::SeqCst)).unwrap_or(i64::MAX)
    }

    /// Build a POST request with standard headers.
    fn build_post(&self, body: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .post(&self.endpoint)
            .timeout(std::time::Duration::from_secs(60))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(body.to_string());

        if let Some(ref auth) = self.auth_header {
            req = req.header(&auth.name, &auth.value);
        }

        req
    }

    /// Attach session ID header if available.
    async fn attach_session_id(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref sid) = *self.session_id.read().await {
            req.header("Mcp-Session-Id", sid.as_str())
        } else {
            req
        }
    }

    /// Extract and store session ID from response headers.
    async fn capture_session_id(&self, headers: &reqwest::header::HeaderMap) {
        if let Some(val) = headers.get("mcp-session-id") {
            if let Ok(sid) = val.to_str() {
                let sid = sid.to_string();
                let changed = {
                    let mut guard = self.session_id.write().await;
                    let changed = guard.as_ref() != Some(&sid);
                    if changed {
                        *guard = Some(sid.clone());
                    }
                    changed
                };

                if changed {
                    tracing::debug!("Captured Mcp-Session-Id: {}", sid);
                    if let Err(e) = self.ensure_get_stream().await {
                        tracing::warn!("Failed to start Streamable HTTP GET stream: {}", e);
                    }
                }
            }
        }
    }

    /// Ensure the background GET stream is running for the current session.
    #[allow(clippy::too_many_lines)]
    async fn ensure_get_stream(&self) -> TransportResult<()> {
        if !self.connected.load(Ordering::Acquire) {
            return Ok(());
        }

        let Some(session_id) = self.session_id.read().await.clone() else {
            return Ok(());
        };

        {
            let same_session = self
                .stream_session_id
                .read()
                .await
                .as_ref()
                .is_some_and(|s| s == &session_id);
            if same_session {
                let guard = self.stream_handle.lock().await;
                if let Some(handle) = guard.as_ref() {
                    if !handle.is_finished() {
                        return Ok(());
                    }
                }
            }
        }

        self.shutdown_get_stream().await;

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel(1);
        {
            let mut guard = self.shutdown_tx.lock().await;
            *guard = Some(shutdown_tx);
        }
        {
            let mut guard = self.stream_session_id.write().await;
            *guard = Some(session_id.clone());
        }

        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let auth_header = self.auth_header.clone();
        let pending = self.pending.clone();

        let handle = tokio::spawn(async move {
            let mut req = client
                .get(&endpoint)
                .header("Accept", "text/event-stream")
                .header("Mcp-Session-Id", session_id.as_str());

            if let Some(ref auth) = auth_header {
                req = req.header(&auth.name, &auth.value);
            }

            let response = match req.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!("Streamable HTTP GET stream failed to connect: {}", e);
                    return;
                }
            };

            if !response.status().is_success() {
                tracing::warn!("Streamable HTTP GET stream returned HTTP {}", response.status());
                return;
            }

            let mut stream = response.bytes_stream();
            let mut buffer: Vec<u8> = Vec::new();
            let mut event_type = String::new();
            let mut event_data = String::new();

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        tracing::debug!("Streamable HTTP GET stream received shutdown signal");
                        break;
                    }
                    chunk = stream.next() => {
                        match chunk {
                            Some(Ok(bytes)) => {
                                if buffer.len() + bytes.len() > SSE_MAX_BUFFER_SIZE {
                                    tracing::warn!(
                                        "Streamable HTTP GET buffer exceeded {} bytes",
                                        SSE_MAX_BUFFER_SIZE
                                    );
                                    break;
                                }
                                buffer.extend_from_slice(&bytes);

                                while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                                    let mut line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
                                    if line_bytes.last() == Some(&b'\n') {
                                        line_bytes.pop();
                                    }
                                    if line_bytes.last() == Some(&b'\r') {
                                        line_bytes.pop();
                                    }

                                    let line = String::from_utf8_lossy(&line_bytes);
                                    let line = line.as_ref();

                                    if line.is_empty() {
                                        if !event_data.is_empty() {
                                            Self::handle_streamable_message(
                                                &event_type,
                                                &event_data,
                                                &pending,
                                            );
                                        }
                                        event_type.clear();
                                        event_data.clear();
                                    } else if let Some(value) = line.strip_prefix("event:") {
                                        event_type = value.trim().to_string();
                                    } else if let Some(value) = line.strip_prefix("data:") {
                                        let data = value.trim_start();
                                        if data == "[DONE]" {
                                            continue;
                                        }
                                        if event_data.len() + data.len() + 1 > SSE_MAX_EVENT_DATA_SIZE {
                                            tracing::warn!(
                                                "Streamable HTTP GET event data exceeded {} bytes",
                                                SSE_MAX_EVENT_DATA_SIZE
                                            );
                                            event_type.clear();
                                            event_data.clear();
                                            continue;
                                        }
                                        if !event_data.is_empty() {
                                            event_data.push('\n');
                                        }
                                        event_data.push_str(data);
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                tracing::warn!("Streamable HTTP GET stream error: {}", e);
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }

            if !event_data.is_empty() {
                Self::handle_streamable_message(&event_type, &event_data, &pending);
            }
            tracing::debug!("Streamable HTTP GET stream ended");
        });

        {
            let mut guard = self.stream_handle.lock().await;
            *guard = Some(handle);
        }
        Ok(())
    }

    /// Shutdown the background GET stream task, if running.
    async fn shutdown_get_stream(&self) {
        let shutdown_tx = self.shutdown_tx.lock().await.take();
        if let Some(tx) = shutdown_tx {
            let _ = tx.send(());
        }
        let stream_handle = self.stream_handle.lock().await.take();
        if let Some(handle) = stream_handle {
            handle.abort();
        }
    }

    /// Handle a JSON-RPC message received from Streamable HTTP GET stream.
    fn handle_streamable_message(
        event_type: &str,
        event_data: &str,
        pending: &Arc<Mutex<HashMap<i64, oneshot::Sender<JsonRpcResponse>>>>,
    ) {
        if !event_type.is_empty() && event_type != "message" {
            tracing::debug!("Ignoring Streamable HTTP event type '{}'", event_type);
            return;
        }

        match serde_json::from_str::<JsonRpcMessage>(event_data) {
            Ok(JsonRpcMessage::Response(response)) => {
                let id = match &response.id {
                    RequestId::Number(n) => *n,
                    RequestId::String(s) => match s.parse() {
                        Ok(n) => n,
                        Err(_) => {
                            tracing::warn!("Non-numeric request ID in Streamable HTTP response: {s}");
                            return;
                        }
                    },
                };

                let sender = pending.lock().remove(&id);
                if let Some(sender) = sender {
                    let _ = sender.send(response);
                } else {
                    tracing::debug!(
                        "Received Streamable HTTP response for unknown request id {}",
                        id
                    );
                }
            }
            Ok(JsonRpcMessage::Notification(notification)) => {
                tracing::debug!("Received Streamable HTTP notification '{}'", notification.method);
            }
            Ok(JsonRpcMessage::Request(request)) => {
                tracing::debug!(
                    "Ignoring server-initiated Streamable HTTP request '{}'",
                    request.method
                );
            }
            Err(e) => {
                tracing::warn!("Failed to parse Streamable HTTP message as JSON-RPC: {}", e);
            }
        }
    }

    /// Parse one JSON payload and return response when it is a JSON-RPC response.
    fn parse_jsonrpc_response_payload(payload: &str) -> TransportResult<Option<JsonRpcResponse>> {
        let msg: JsonRpcMessage = serde_json::from_str(payload).map_err(|e| {
            TransportError::InvalidResponse(format!("JSON parse error: {e} - {payload}"))
        })?;

        match msg {
            JsonRpcMessage::Response(response) => Ok(Some(response)),
            JsonRpcMessage::Notification(notification) => {
                tracing::debug!(
                    "Ignoring notification in request-response path: '{}'",
                    notification.method
                );
                Ok(None)
            }
            JsonRpcMessage::Request(request) => {
                tracing::debug!(
                    "Ignoring server request in request-response path: '{}'",
                    request.method
                );
                Ok(None)
            }
        }
    }

    async fn await_pending_response(
        &self,
        id: i64,
        rx: oneshot::Receiver<JsonRpcResponse>,
    ) -> TransportResult<JsonRpcResponse> {
        match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => {
                self.pending.lock().remove(&id);
                Err(TransportError::ChannelClosed)
            }
            Err(_) => {
                self.pending.lock().remove(&id);
                Err(TransportError::Timeout)
            }
        }
    }

    /// Parse a single JSON response body into `JsonRpcResponse`.
    async fn parse_json_response(
        &self,
        response: reqwest::Response,
    ) -> TransportResult<Option<JsonRpcResponse>> {
        self.capture_session_id(response.headers()).await;
        let text = response
            .text()
            .await
            .map_err(|e| TransportError::HttpError(format!("Failed to read body: {e}")))?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        Self::parse_jsonrpc_response_payload(&text)
    }

    /// Parse an SSE response stream, returning the first JSON-RPC response found.
    async fn parse_sse_response(
        &self,
        response: reqwest::Response,
    ) -> TransportResult<Option<JsonRpcResponse>> {
        self.capture_session_id(response.headers()).await;

        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        let mut event_data = String::new();
        let timeout_dur = std::time::Duration::from_secs(60);

        loop {
            match tokio::time::timeout(timeout_dur, stream.next()).await {
                Ok(Some(Ok(bytes))) => {
                    if buffer.len() + bytes.len() > SSE_MAX_BUFFER_SIZE {
                        return Err(TransportError::HttpError(format!(
                            "SSE buffer exceeded {SSE_MAX_BUFFER_SIZE} bytes"
                        )));
                    }
                    buffer.extend_from_slice(&bytes);

                    while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                        let mut line_bytes: Vec<u8> = buffer.drain(..=pos).collect();
                        if line_bytes.last() == Some(&b'\n') {
                            line_bytes.pop();
                        }
                        if line_bytes.last() == Some(&b'\r') {
                            line_bytes.pop();
                        }

                        let line = String::from_utf8_lossy(&line_bytes);
                        let line = line.as_ref();

                        if line.is_empty() {
                            if !event_data.is_empty() {
                                match Self::parse_jsonrpc_response_payload(&event_data) {
                                    Ok(Some(resp)) => return Ok(Some(resp)),
                                    Ok(None) => {}
                                    Err(e) => {
                                        tracing::warn!("SSE event parse error: {}", e);
                                    }
                                }
                                event_data.clear();
                            }
                        } else if let Some(data) = line.strip_prefix("data:") {
                            let data = data.trim_start();
                            if data == "[DONE]" {
                                continue;
                            }
                            if event_data.len() + data.len() + 1 > SSE_MAX_EVENT_DATA_SIZE {
                                return Err(TransportError::HttpError(format!(
                                    "SSE event data exceeded {SSE_MAX_EVENT_DATA_SIZE} bytes"
                                )));
                            }
                            if !event_data.is_empty() {
                                event_data.push('\n');
                            }
                            event_data.push_str(data);
                        }
                    }
                }
                Ok(Some(Err(e))) => {
                    return Err(TransportError::HttpError(format!("SSE stream error: {e}")));
                }
                Ok(None) => {
                    if !event_data.is_empty() {
                        return Self::parse_jsonrpc_response_payload(&event_data);
                    }
                    return Ok(None);
                }
                Err(_) => {
                    return Err(TransportError::Timeout);
                }
            }
        }
    }

    fn remove_pending_request(&self, id: i64) {
        self.pending.lock().remove(&id);
    }

    fn register_pending_request(&self, id: i64) -> oneshot::Receiver<JsonRpcResponse> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id, tx);
        rx
    }
}

#[async_trait::async_trait]
impl McpTransport for StreamableHttpTransport {
    async fn request(&self, mut request: JsonRpcRequest) -> TransportResult<JsonRpcResponse> {
        if !self.connected.load(Ordering::Acquire) {
            return Err(TransportError::NotConnected);
        }

        let id = self.next_id();
        let rx = self.register_pending_request(id);
        request.id = id.into();
        request.jsonrpc = JSONRPC_VERSION.to_string();
        let _ = self.ensure_get_stream().await;

        let json = serde_json::to_string(&JsonRpcMessage::Request(request))?;
        let req = self.build_post(&json);
        let req = self.attach_session_id(req).await;

        let response = match req.send().await {
            Ok(response) => response,
            Err(e) => {
                self.remove_pending_request(id);
                return Err(TransportError::HttpError(format!("Request failed: {e}")));
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            self.remove_pending_request(id);
            return Err(TransportError::HttpError(format!("HTTP {status}: {body}")));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let immediate = if content_type.contains("text/event-stream") {
            self.parse_sse_response(response).await
        } else {
            self.parse_json_response(response).await
        };

        match immediate {
            Ok(Some(resp)) => {
                self.remove_pending_request(id);
                Ok(resp)
            }
            Ok(None) => self.await_pending_response(id, rx).await,
            Err(e) => {
                self.remove_pending_request(id);
                Err(e)
            }
        }
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> TransportResult<()> {
        if !self.connected.load(Ordering::Acquire) {
            return Err(TransportError::NotConnected);
        }

        let notification = JsonRpcNotification::new(method, params);
        let json = serde_json::to_string(&JsonRpcMessage::Notification(notification))?;
        let req = self.build_post(&json);
        let req = self.attach_session_id(req).await;

        let response = req
            .send()
            .await
            .map_err(|e| TransportError::HttpError(format!("Notify failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(TransportError::HttpError(format!("Notify HTTP {status}: {body}")));
        }

        self.capture_session_id(response.headers()).await;
        let _ = self.ensure_get_stream().await;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    async fn close(&mut self) -> TransportResult<()> {
        self.shutdown_get_stream().await;

        let session_id = self.session_id.read().await.clone();
        if let Some(ref sid) = session_id {
            let mut req = self.client.delete(&self.endpoint).header("Mcp-Session-Id", sid.as_str());

            if let Some(ref auth) = self.auth_header {
                req = req.header(&auth.name, &auth.value);
            }

            let _ = req.send().await;
            tracing::debug!("Sent DELETE to terminate MCP session");
        }

        self.connected.store(false, Ordering::Release);
        self.pending.lock().clear();
        tracing::info!("Streamable HTTP transport closed");
        Ok(())
    }
}

impl Drop for StreamableHttpTransport {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.shutdown_tx.try_lock() {
            if let Some(tx) = guard.take() {
                let _ = tx.send(());
            }
        }
        if let Ok(mut guard) = self.stream_handle.try_lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
        self.connected.store(false, Ordering::Release);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_error_display() {
        let err = TransportError::SpawnError("command not found".to_string());
        assert!(err.to_string().contains("command not found"));
    }

    #[test]
    fn test_transport_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: TransportError = io_err.into();
        assert!(matches!(err, TransportError::IoError(_)));
    }

    #[test]
    fn test_extract_base_url() {
        assert_eq!(
            SseTransport::extract_base_url("https://mcp.example.com/sse").expect("parse"),
            "https://mcp.example.com"
        );
        assert_eq!(
            SseTransport::extract_base_url("http://localhost:8080/api/sse").expect("parse"),
            "http://localhost:8080"
        );
        assert_eq!(
            SseTransport::extract_base_url("https://api.example.com:8443/mcp").expect("parse"),
            "https://api.example.com:8443"
        );
    }

    #[test]
    fn test_extract_base_url_invalid() {
        assert!(SseTransport::extract_base_url("not-a-url").is_err());
        assert!(SseTransport::extract_base_url("file:///local/path").is_err());
    }

    #[test]
    fn test_validate_same_origin_success() {
        let base = "https://mcp.example.com";
        assert!(SseTransport::validate_same_origin(base, "https://mcp.example.com/message").is_ok());
        assert!(
            SseTransport::validate_same_origin(base, "https://mcp.example.com/api/v1/post").is_ok()
        );
    }

    #[test]
    fn test_validate_same_origin_relative_path() {
        let base = "https://mcp.example.com";
        assert!(SseTransport::validate_same_origin(base, "/message").is_ok());
    }

    #[test]
    fn test_validate_same_origin_failure_different_host() {
        let base = "https://mcp.example.com";
        let result = SseTransport::validate_same_origin(base, "https://internal.corp.net/api");
        assert!(result.is_err());
        let err = result.expect_err("should fail").to_string();
        assert!(err.contains("SSRF"));
    }

    #[test]
    fn test_validate_same_origin_failure_different_scheme() {
        let base = "https://mcp.example.com";
        let result = SseTransport::validate_same_origin(base, "http://mcp.example.com/message");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_same_origin_failure_different_port() {
        let base = "https://mcp.example.com:8443";
        let result =
            SseTransport::validate_same_origin(base, "https://mcp.example.com:9443/message");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_same_origin_localhost() {
        let base = "http://localhost:8080";
        assert!(SseTransport::validate_same_origin(base, "http://localhost:8080/post").is_ok());
        assert!(SseTransport::validate_same_origin(base, "http://localhost:9090/post").is_err());
        assert!(SseTransport::validate_same_origin(base, "http://127.0.0.1:8080/post").is_err());
    }

    #[test]
    fn test_streamable_http_transport_error_display() {
        let err = TransportError::HttpError("connection refused".to_string());
        assert!(err.to_string().contains("connection refused"));
    }

    #[tokio::test]
    async fn test_streamable_http_connect_creates_transport() {
        let transport =
            StreamableHttpTransport::connect("http://127.0.0.1:1/mcp", None, None).await;
        assert!(transport.is_ok());
        let transport = transport.expect("should succeed");
        assert!(transport.is_connected());
    }

    #[tokio::test]
    async fn test_streamable_http_request_fails_on_unreachable() {
        let transport =
            StreamableHttpTransport::connect("http://127.0.0.1:1/mcp", None, None)
                .await
                .expect("connect should succeed");
        let req = JsonRpcRequest::new(1i64, "ping", None);
        let result = transport.request(req).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_auth_header_bearer() {
        let header = AuthHeader::bearer("my-token");
        assert_eq!(header.name, "Authorization");
        assert_eq!(header.value, "Bearer my-token");
    }

    #[test]
    fn test_proxy_config_default() {
        let config = ProxyConfig::default();
        assert_eq!(config.mode, ProxyMode::None);
        assert!(config.http_url.is_none());
    }
}
