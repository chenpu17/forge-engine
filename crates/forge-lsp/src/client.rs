//! JSON-RPC client for LSP communication over stdin/stdout
//!
//! Implements the base protocol layer for Language Server Protocol,
//! handling message framing (Content-Length headers) and JSON-RPC request/response.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};

use crate::LspError;

/// Type alias for the pending response channel map.
type PendingMap = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, LspError>>>>>;

/// JSON-RPC request
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: i64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// JSON-RPC response
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<i64>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

/// LSP client connected to a single language server process
///
/// Debug is manually implemented since inner types (Mutex, Child) don't derive it.
pub struct LspClient {
    /// Language identifier (e.g., "rust", "typescript", "python")
    pub language: String,
    /// Server command that was used to start this client
    pub server_cmd: String,
    /// Working directory
    pub working_dir: PathBuf,
    /// Next request ID
    next_id: AtomicI64,
    /// Child process stdin (for sending requests)
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    /// Pending response channels
    pending: PendingMap,
    /// Child process handle (kept alive)
    _child: Arc<Mutex<Child>>,
    /// Whether the server has been initialized
    initialized: Arc<std::sync::atomic::AtomicBool>,
    /// Set to true when the reader task exits (server crashed/exited).
    /// Prevents new requests from being inserted into `pending` after drain.
    server_dead: Arc<std::sync::atomic::AtomicBool>,
}

impl std::fmt::Debug for LspClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspClient")
            .field("language", &self.language)
            .field("server_cmd", &self.server_cmd)
            .field("working_dir", &self.working_dir)
            .finish_non_exhaustive()
    }
}

impl LspClient {
    /// Spawn a language server process and create a client
    ///
    /// # Errors
    /// Returns error if the server process cannot be spawned
    #[allow(clippy::unused_async)]
    pub async fn spawn(
        command: &str,
        args: &[&str],
        working_dir: &Path,
        language: &str,
    ) -> Result<Self, LspError> {
        tracing::info!(command = %command, language = %language, "Spawning LSP server");

        let mut child = Command::new(command)
            .args(args)
            .current_dir(working_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| LspError::ServerSpawn(format!("{command}: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| LspError::ServerSpawn("Failed to capture stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::ServerSpawn("Failed to capture stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| LspError::ServerSpawn("Failed to capture stderr".to_string()))?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        let server_dead = Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Spawn reader task to process responses
        let pending_clone = pending.clone();
        let dead_flag = server_dead.clone();
        tokio::spawn(async move {
            let result = read_responses(stdout, pending_clone.clone()).await;
            if let Err(e) = &result {
                tracing::warn!(error = %e, "LSP response reader exited");
            }
            // Mark server as dead BEFORE draining, so no new requests
            // can be inserted into `pending` between drain and flag set.
            dead_flag.store(true, Ordering::Release);
            // Drain all pending channels so waiters don't hang forever
            // when the server crashes or exits unexpectedly.
            let mut pending = pending_clone.lock().await;
            for (id, tx) in pending.drain() {
                let _ = tx.send(Err(LspError::Protocol(format!(
                    "LSP server exited while request {id} was pending"
                ))));
            }
        });

        // Always drain stderr to avoid child process blocking on a full pipe.
        tokio::spawn(async move {
            drain_stderr(stderr).await;
        });

        Ok(Self {
            language: language.to_string(),
            server_cmd: command.to_string(),
            working_dir: working_dir.to_path_buf(),
            next_id: AtomicI64::new(1),
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            _child: Arc::new(Mutex::new(child)),
            initialized: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            server_dead,
        })
    }

    /// Check whether the language server process has exited.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.server_dead.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Write a framed message to the server's stdin.
    async fn write_message(&self, message: &str) -> Result<(), LspError> {
        let write_result = tokio::time::timeout(STDIN_WRITE_TIMEOUT, async {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(message.as_bytes()).await?;
            stdin.flush().await
        })
        .await;

        match write_result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(LspError::Protocol(format!("Write error: {e}"))),
            Err(_) => Err(LspError::Protocol(format!(
                "Timed out writing to LSP stdin after {}s",
                STDIN_WRITE_TIMEOUT.as_secs()
            ))),
        }
    }

    /// Initialize the LSP server (must be called before other requests)
    ///
    /// # Errors
    /// Returns error if initialization fails
    pub async fn initialize(&self) -> Result<Value, LspError> {
        if self.initialized.load(Ordering::Acquire) {
            return Ok(Value::Null);
        }

        let init_params = serde_json::json!({
            "processId": std::process::id(),
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": { "relatedInformation": true },
                    "definition": { "dynamicRegistration": false },
                    "references": { "dynamicRegistration": false },
                    "hover": { "dynamicRegistration": false }
                }
            },
            "rootUri": path_to_file_uri(&self.working_dir),
            "workspaceFolders": [{
                "uri": path_to_file_uri(&self.working_dir),
                "name": self.working_dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
            }]
        });

        let result = self.request("initialize", Some(init_params)).await?;

        // Send initialized notification
        self.notify("initialized", Some(serde_json::json!({}))).await?;

        self.initialized.store(true, Ordering::Release);
        Ok(result)
    }

    /// Send a request and wait for response
    ///
    /// # Errors
    /// Returns error if the request fails or times out
    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value, LspError> {
        // Fail fast if the server has exited
        if self.server_dead.load(Ordering::Acquire) {
            return Err(LspError::Protocol("LSP server has exited".to_string()));
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = JsonRpcRequest { jsonrpc: "2.0", id, method: method.to_string(), params };

        let body = serde_json::to_string(&request)
            .map_err(|e| LspError::Protocol(format!("Serialize error: {e}")))?;

        let message = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        // Register pending response channel (re-check dead flag under lock)
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if self.server_dead.load(Ordering::Acquire) {
                return Err(LspError::Protocol("LSP server has exited".to_string()));
            }
            pending.insert(id, tx);
        }

        // Send request; if write fails, remove the pending channel to avoid leaks.
        if let Err(e) = self.write_message(&message).await {
            self.pending.lock().await.remove(&id);
            return Err(e);
        }

        // Wait for response with timeout
        let timeout = tokio::time::Duration::from_secs(30);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(LspError::Protocol("Response channel closed".to_string())),
            Err(_) => {
                // Clean up pending entry
                self.pending.lock().await.remove(&id);
                Err(LspError::Timeout(30))
            }
        }
    }

    /// Send a notification (no response expected)
    ///
    /// # Errors
    /// Returns error if the notification cannot be sent
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), LspError> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(Value::Null)
        });

        let body = serde_json::to_string(&notification)
            .map_err(|e| LspError::Protocol(format!("Serialize error: {e}")))?;

        let message = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        self.write_message(&message).await
    }

    /// Notify the server about a file being opened
    ///
    /// # Errors
    /// Returns error if the notification fails
    pub async fn open_file(&self, path: &Path) -> Result<(), LspError> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| LspError::Protocol(format!("Read file error: {e}")))?;

        let language_id = detect_language_id(path);

        self.notify(
            "textDocument/didOpen",
            Some(serde_json::json!({
                "textDocument": {
                    "uri": path_to_file_uri(path),
                    "languageId": language_id,
                    "version": 1,
                    "text": content
                }
            })),
        )
        .await
    }

    /// Shut down the server gracefully
    ///
    /// # Errors
    /// Returns error if shutdown fails
    pub async fn shutdown(&self) -> Result<(), LspError> {
        let _ = self.request("shutdown", None).await;
        let _ = self.notify("exit", None).await;
        Ok(())
    }
}

/// Maximum allowed Content-Length (16 MiB). Protects against OOM from
/// a buggy or malicious server advertising an absurdly large body.
const MAX_CONTENT_LENGTH: usize = 16 * 1024 * 1024;

/// Maximum number of headers to read before the empty-line separator.
/// LSP typically sends only `Content-Length` and optionally `Content-Type`.
const MAX_HEADER_COUNT: usize = 32;

/// Maximum length of a single header line (8 KiB).
const MAX_HEADER_LINE_LEN: usize = 8 * 1024;
/// Timeout when writing data into the language server stdin pipe.
const STDIN_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum stderr line length captured in logs.
const MAX_STDERR_LINE_LEN: usize = 4 * 1024;

/// Read LSP responses from a stream and dispatch to pending channels.
///
/// The parser is protocol-resilient: it ignores non-protocol stdout noise and
/// attempts to resynchronize on the next valid `Content-Length` frame instead of
/// failing hard on malformed header blocks.
#[allow(clippy::too_many_lines)]
async fn read_responses<R>(stdout: R, pending: PendingMap) -> Result<(), LspError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stdout);

    loop {
        // Scan stdout until we hit a valid `Content-Length` header.
        let length = loop {
            let mut header = String::new();
            let bytes_read = reader
                .read_line(&mut header)
                .await
                .map_err(|e| LspError::Protocol(format!("Header read error: {e}")))?;

            if bytes_read == 0 {
                return Ok(()); // EOF — server closed
            }

            // Guard against unbounded lines. Treat as noise and keep scanning.
            if header.len() > MAX_HEADER_LINE_LEN {
                tracing::warn!(
                    len = header.len(),
                    max = MAX_HEADER_LINE_LEN,
                    "Skipping oversized stdout line while searching LSP frame"
                );
                continue;
            }

            let header = header.trim_end_matches(['\r', '\n']);
            if header.is_empty() {
                continue;
            }

            let Some(len_str) = header.strip_prefix("Content-Length:") else {
                tracing::debug!("Skipping non-protocol stdout line while waiting for LSP header");
                continue;
            };

            let length = match len_str.trim().parse::<usize>() {
                Ok(len) => len,
                Err(e) => {
                    tracing::warn!(
                        value = %len_str.trim(),
                        error = %e,
                        "Invalid Content-Length value, skipping candidate frame"
                    );
                    continue;
                }
            };

            // Read the rest of the header block. If malformed, drop and resync.
            let mut header_count = 1usize;
            let mut malformed_headers = false;
            loop {
                let mut extra = String::new();
                let n = reader
                    .read_line(&mut extra)
                    .await
                    .map_err(|e| LspError::Protocol(format!("Header read error: {e}")))?;
                if n == 0 {
                    return Ok(());
                }

                if extra.len() > MAX_HEADER_LINE_LEN {
                    malformed_headers = true;
                }

                let extra = extra.trim_end_matches(['\r', '\n']);
                if extra.is_empty() {
                    break;
                }

                header_count += 1;
                if header_count > MAX_HEADER_COUNT {
                    malformed_headers = true;
                }

                // Strictly require header-like lines to reduce false framing on stdout logs.
                if !extra.contains(':') {
                    malformed_headers = true;
                }
            }

            if malformed_headers {
                tracing::warn!(
                    headers = header_count,
                    max = MAX_HEADER_COUNT,
                    "Dropping malformed LSP header block and resynchronizing"
                );
                continue;
            }
            break length;
        };

        if length > MAX_CONTENT_LENGTH {
            return Err(LspError::Protocol(format!(
                "Content-Length {length} exceeds maximum {MAX_CONTENT_LENGTH}"
            )));
        }

        // Read body
        let mut body = vec![0u8; length];
        reader
            .read_exact(&mut body)
            .await
            .map_err(|e| LspError::Protocol(format!("Body read error: {e}")))?;

        let response: JsonRpcResponse = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                // Log malformed messages for debugging instead of silently dropping
                tracing::debug!(
                    error = %e,
                    body_len = length,
                    "Skipping non-response LSP message (notification or malformed)"
                );
                continue;
            }
        };

        // Dispatch response
        if let Some(id) = response.id {
            let mut pending = pending.lock().await;
            if let Some(tx) = pending.remove(&id) {
                let result = if let Some(error) = response.error {
                    Err(LspError::ServerError { code: error.code, message: error.message })
                } else {
                    Ok(response.result.unwrap_or(Value::Null))
                };
                let _ = tx.send(result);
            }
        }
    }
}

/// Drain stderr from the language server to prevent pipe blocking.
async fn drain_stderr<R>(stderr: R)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stderr);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\r', '\n']);
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.len() > MAX_STDERR_LINE_LEN {
                    tracing::debug!(
                        len = trimmed.len(),
                        max = MAX_STDERR_LINE_LEN,
                        "LSP stderr line too long, truncating"
                    );
                    let mut end = MAX_STDERR_LINE_LEN;
                    while end > 0 && !trimmed.is_char_boundary(end) {
                        end -= 1;
                    }
                    tracing::debug!(stderr = %&trimmed[..end], "LSP stderr");
                } else {
                    tracing::debug!(stderr = %trimmed, "LSP stderr");
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "Failed to read LSP stderr");
                break;
            }
        }
    }
}

/// Convert a filesystem path to a properly encoded `file://` URI.
///
/// Handles spaces and special characters in path components by
/// percent-encoding them, which is required by the LSP specification.
#[must_use]
pub fn path_to_file_uri(path: &Path) -> String {
    use std::fmt::Write as _;

    let path_str = path.to_string_lossy();
    // Percent-encode each path segment individually to preserve `/` separators
    let encoded: String = path_str
        .split('/')
        .map(|segment| {
            // Encode special characters but leave alphanumeric, `-`, `_`, `.` alone
            segment
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
                        c.to_string()
                    } else {
                        let mut buf = [0u8; 4];
                        let encoded_char = c.encode_utf8(&mut buf);
                        encoded_char.bytes().fold(String::new(), |mut acc, b| {
                            let _ = write!(acc, "%{b:02X}");
                            acc
                        })
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("file://{encoded}")
}

/// Detect language ID from file extension
fn detect_language_id(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("ts" | "tsx") => "typescript",
        Some("js" | "jsx") => "javascript",
        Some("py") => "python",
        Some("go") => "go",
        Some("java") => "java",
        Some("c" | "h") => "c",
        Some("cpp" | "hpp" | "cc" | "cxx") => "cpp",
        Some("rb") => "ruby",
        Some("lua") => "lua",
        Some("zig") => "zig",
        _ => "plaintext",
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fmt::Write as _;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::{oneshot, Mutex};

    fn response_frame(id: i64, result: &Value) -> String {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        })
        .to_string();
        format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
    }

    #[test]
    fn test_detect_language_id() {
        assert_eq!(detect_language_id(Path::new("main.rs")), "rust");
        assert_eq!(detect_language_id(Path::new("app.tsx")), "typescript");
        assert_eq!(detect_language_id(Path::new("index.js")), "javascript");
        assert_eq!(detect_language_id(Path::new("script.py")), "python");
        assert_eq!(detect_language_id(Path::new("main.go")), "go");
        assert_eq!(detect_language_id(Path::new("README.md")), "plaintext");
    }

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "initialize".to_string(),
            params: Some(serde_json::json!({"processId": 123})),
        };
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"initialize\""));
    }

    #[test]
    fn test_json_rpc_request_no_params() {
        let req =
            JsonRpcRequest { jsonrpc: "2.0", id: 2, method: "shutdown".to_string(), params: None };
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(!json.contains("params"));
    }

    #[test]
    fn test_path_to_file_uri_simple() {
        let uri = path_to_file_uri(Path::new("/project/src/main.rs"));
        assert_eq!(uri, "file:///project/src/main.rs");
    }

    #[test]
    fn test_path_to_file_uri_with_spaces() {
        let uri = path_to_file_uri(Path::new("/my project/src/main.rs"));
        assert!(uri.starts_with("file://"));
        // Space must be percent-encoded
        assert!(uri.contains("my%20project"));
        assert!(!uri.contains("my project"));
    }

    #[test]
    fn test_path_to_file_uri_with_special_chars() {
        let uri = path_to_file_uri(Path::new("/path/with@special#chars/file.rs"));
        assert!(uri.starts_with("file://"));
        // @ and # should be percent-encoded
        assert!(!uri.contains('@'));
        assert!(!uri.contains('#'));
    }

    #[tokio::test]
    async fn test_read_responses_ignores_stdout_noise() {
        let (reader, mut writer) = tokio::io::duplex(16 * 1024);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(1, tx);

        let pending_clone = pending.clone();
        let reader_task = tokio::spawn(async move { read_responses(reader, pending_clone).await });

        writer
            .write_all(b"rust-analyzer: startup complete\nsome debug line\n")
            .await
            .expect("write noise");
        writer
            .write_all(response_frame(1, &serde_json::json!({"ok": true})).as_bytes())
            .await
            .expect("write response");
        drop(writer);

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("timed out waiting response")
            .expect("channel closed")
            .expect("lsp error");
        assert_eq!(result["ok"], true);

        let done = reader_task.await.expect("join");
        assert!(done.is_ok());
    }

    #[tokio::test]
    async fn test_read_responses_resyncs_after_malformed_headers() {
        let (reader, mut writer) = tokio::io::duplex(16 * 1024);
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        let (tx, rx) = oneshot::channel();
        pending.lock().await.insert(7, tx);

        let pending_clone = pending.clone();
        let reader_task = tokio::spawn(async move { read_responses(reader, pending_clone).await });

        // This block exceeds MAX_HEADER_COUNT and should be dropped instead of crashing.
        let mut malformed = String::from("Content-Length: 0\r\n");
        for i in 0..(MAX_HEADER_COUNT + 4) {
            let _ = write!(malformed, "x-{i}: y\r\n");
        }
        malformed.push_str("\r\n");
        writer.write_all(malformed.as_bytes()).await.expect("write malformed");

        writer
            .write_all(response_frame(7, &serde_json::json!({"recovered": true})).as_bytes())
            .await
            .expect("write response");
        drop(writer);

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), rx)
            .await
            .expect("timed out waiting response")
            .expect("channel closed")
            .expect("lsp error");
        assert_eq!(result["recovered"], true);

        let done = reader_task.await.expect("join");
        assert!(done.is_ok());
    }
}
