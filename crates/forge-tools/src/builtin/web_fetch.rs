//! `WebFetch` tool for fetching and processing web page content
//!
//! This tool fetches content from a URL, converts HTML to markdown,
//! and processes it with a prompt to extract relevant information.
//!
//! Design based on Claude Code's `WebFetch` implementation:
//! - Takes URL and prompt as input
//! - Converts HTML to markdown for better readability
//! - Supports caching to avoid repeated fetches
//! - Handles redirects gracefully
//! - Auto-upgrades HTTP to HTTPS

use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, RetryConfig, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use futures::StreamExt;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Maximum content length to return (in bytes)
const MAX_CONTENT_LENGTH: usize = 100_000;

/// Maximum response size to read from network (denial-of-service protection)
/// Stop reading after this many bytes to prevent memory exhaustion
const MAX_RESPONSE_READ_SIZE: usize = 10_000_000; // 10 MB

/// Default timeout for HTTP requests (in seconds)
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Cache TTL in seconds (15 minutes, matching Claude Code)
const CACHE_TTL_SECS: u64 = 900;

/// Safely truncate a UTF-8 string to at most `max_bytes` bytes
/// without cutting through multi-byte characters.
fn truncate_utf8_safe(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Find the last valid UTF-8 character boundary at or before max_bytes
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r#"Fetch content from a URL and process it with a prompt.

This tool:
- Fetches content from the specified URL
- Converts HTML to readable markdown format
- Processes the content based on your prompt

Usage notes:
- HTTP URLs are automatically upgraded to HTTPS
- Includes a 15-minute cache for faster repeated access
- When a URL redirects to a different host, you'll be notified and should make a new request with the redirect URL
- The prompt should describe what information you want to extract from the page

Example prompts:
- "Extract the main documentation content"
- "Find the API endpoint definitions"
- "Summarize the key features described on this page""#;

/// Cached response entry
#[derive(Debug, Clone)]
struct CacheEntry {
    content: String,
    fetched_at: Instant,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.fetched_at.elapsed().as_secs() > CACHE_TTL_SECS
    }
}

/// URL fetch cache
#[derive(Debug, Default)]
pub struct FetchCache {
    entries: HashMap<String, CacheEntry>,
}

impl FetchCache {
    /// Create a new cache
    #[must_use]
    pub fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    /// Get cached content if not expired
    #[must_use]
    pub fn get(&self, url: &str) -> Option<String> {
        self.entries.get(url).and_then(|entry| {
            if entry.is_expired() {
                None
            } else {
                Some(entry.content.clone())
            }
        })
    }

    /// Store content in cache
    pub fn set(&mut self, url: String, content: String) {
        self.entries.insert(url, CacheEntry { content, fetched_at: Instant::now() });
    }

    /// Remove expired entries
    pub fn cleanup(&mut self) {
        self.entries.retain(|_, entry| !entry.is_expired());
    }
}

/// `WebFetch` tool for fetching and processing web pages
///
/// Based on Claude Code's `WebFetch` implementation:
/// - Fetches content from URL
/// - Converts HTML to markdown
/// - Processes with a prompt to extract relevant information
/// - Includes 15-minute cache for faster repeated access
pub struct WebFetchTool {
    /// HTTP client
    client: Client,
    /// Response cache
    cache: Arc<RwLock<FetchCache>>,
}

impl WebFetchTool {
    /// Build the base `ClientBuilder` with web-fetch defaults.
    fn base_builder() -> reqwest::ClientBuilder {
        Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .user_agent("Mozilla/5.0 (compatible; Forge/1.0; +https://github.com/anthropics/forge)")
            .redirect(reqwest::redirect::Policy::limited(10))
    }

    /// Build a client safely — falls back to `.no_proxy()` on failure
    /// (avoids the `Client::new()` panic on macOS when system proxy query fails).
    fn safe_build(builder: reqwest::ClientBuilder) -> Client {
        let primary = catch_unwind(AssertUnwindSafe(|| builder.build()));
        if let Ok(Ok(client)) = primary {
            return client;
        }
        match primary {
            Ok(Err(e)) => tracing::warn!("HTTP client build failed ({e}), retrying with no_proxy"),
            Err(_) => tracing::warn!("HTTP client build panicked, retrying with no_proxy"),
            Ok(Ok(_)) => {}
        }

        let fallback = catch_unwind(AssertUnwindSafe(|| Self::base_builder().no_proxy().build()));
        match fallback {
            Ok(Ok(client)) => client,
            Ok(Err(e)) => {
                tracing::error!(
                    "no_proxy client build also failed ({e}); \
                     this usually means the TLS backend is broken"
                );
                panic!("Cannot create any HTTP client: {e}");
            }
            Err(_) => {
                tracing::error!("no_proxy client build panicked");
                panic!("Cannot create any HTTP client: no_proxy builder panicked");
            }
        }
    }

    /// Create a new `WebFetchTool` (no proxy, safe fallback)
    #[must_use]
    pub fn new() -> Self {
        let client = Self::safe_build(Self::base_builder());
        Self { client, cache: Arc::new(RwLock::new(FetchCache::new())) }
    }

    /// Create with proxy configuration from `forge-infra`
    #[must_use]
    pub fn with_proxy(proxy: &forge_config::ProxyConfig) -> Self {
        let builder = Self::base_builder();
        let client = match forge_infra::http::configure_http_client_builder(builder, Some(proxy)) {
            Ok(b) => Self::safe_build(b),
            Err(e) => {
                tracing::warn!("Proxy configuration failed ({e}), using direct connection");
                Self::safe_build(Self::base_builder().no_proxy())
            }
        };
        Self { client, cache: Arc::new(RwLock::new(FetchCache::new())) }
    }

    /// Create with custom client
    #[must_use]
    pub fn with_client(client: Client) -> Self {
        Self { client, cache: Arc::new(RwLock::new(FetchCache::new())) }
    }

    /// Normalize URL (upgrade HTTP to HTTPS)
    fn normalize_url(url: &str) -> String {
        url.strip_prefix("http://")
            .map_or_else(|| url.to_string(), |stripped| format!("https://{stripped}"))
    }

    /// Fetch URL content
    #[allow(clippy::too_many_lines)]
    async fn fetch_url(
        &self,
        url: &str,
        timeout_secs: u64,
    ) -> std::result::Result<(String, Option<String>), ToolError> {
        // Normalize URL
        let normalized_url = Self::normalize_url(url);

        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(content) = cache.get(&normalized_url) {
                tracing::debug!("Cache hit for URL: {}", normalized_url);
                return Ok((content, None));
            }
        }

        // Validate URL
        let parsed_url = reqwest::Url::parse(&normalized_url)
            .map_err(|e| ToolError::InvalidParams(format!("Invalid URL: {e}")))?;

        // Only allow https (http is auto-upgraded)
        if parsed_url.scheme() != "https" {
            return Err(ToolError::InvalidParams(format!(
                "Unsupported URL scheme: {}. Only http/https are allowed.",
                parsed_url.scheme()
            )));
        }

        // Build request with timeout
        let response = self
            .client
            .get(&normalized_url)
            .timeout(Duration::from_secs(timeout_secs))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ToolError::Timeout(timeout_secs)
                } else if e.is_connect() {
                    ToolError::ExecutionFailed(format!("Connection failed: {e}"))
                } else {
                    ToolError::ExecutionFailed(format!("Request failed: {e}"))
                }
            })?;

        // Check for redirect to different host
        let final_url = response.url().to_string();
        let redirect_notice = if final_url == normalized_url {
            None
        } else {
            let original_host = parsed_url.host_str().unwrap_or("");
            let final_parsed = reqwest::Url::parse(&final_url).ok();
            let final_host = final_parsed.as_ref().and_then(|u| u.host_str()).unwrap_or("");

            if original_host == final_host {
                None
            } else {
                Some(format!(
                    "URL redirected to a different host: {final_url}\nPlease make a new request with this URL if you want to fetch the content."
                ))
            }
        };

        // If redirected to different host, return notice instead of content
        if redirect_notice.is_some() {
            return Ok((String::new(), redirect_notice));
        }

        // Check status
        let status = response.status();
        if !status.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "HTTP error: {} {}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("Unknown")
            )));
        }

        // Get content type (clone to avoid borrow issues)
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/html")
            .to_string();

        // Check Content-Length header for early rejection of huge responses
        if let Some(content_length) = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok())
        {
            if content_length > MAX_RESPONSE_READ_SIZE {
                return Err(ToolError::ExecutionFailed(format!(
                    "Response too large: {content_length} bytes (max: {MAX_RESPONSE_READ_SIZE} bytes). Content-Length header indicates oversized response."
                )));
            }
        }

        // Stream response body with size limit to prevent memory exhaustion
        let mut body_bytes = Vec::with_capacity(MAX_CONTENT_LENGTH.min(MAX_RESPONSE_READ_SIZE));
        let mut total_read: usize = 0;
        let mut stream = response.bytes_stream();
        let mut truncated_by_read_limit = false;

        while let Some(chunk_result) = stream.next().await {
            let chunk = chunk_result.map_err(|e| {
                ToolError::ExecutionFailed(format!("Failed to read response chunk: {e}"))
            })?;

            let remaining = MAX_RESPONSE_READ_SIZE.saturating_sub(total_read);
            if remaining == 0 {
                truncated_by_read_limit = true;
                break;
            }

            let bytes_to_take = chunk.len().min(remaining);
            body_bytes.extend_from_slice(&chunk[..bytes_to_take]);
            total_read += chunk.len();

            if total_read >= MAX_RESPONSE_READ_SIZE {
                truncated_by_read_limit = true;
                break;
            }
        }

        // Convert bytes to string
        let body = String::from_utf8_lossy(&body_bytes).into_owned();

        // Convert based on content type
        let text =
            if content_type.contains("text/html") || content_type.contains("application/xhtml") {
                // Convert HTML to markdown-like text
                html_to_markdown(&body)
            } else if content_type.contains("text/") || content_type.contains("application/json") {
                // Return as-is for text and JSON
                body
            } else {
                // For other types, return a summary
                format!(
                    "[Binary content of type: {}]\n\nContent length: {} bytes",
                    content_type,
                    body.len()
                )
            };

        // Truncate if too long (safely handling UTF-8 boundaries)
        let text = if text.len() > MAX_CONTENT_LENGTH {
            let truncated = truncate_utf8_safe(&text, MAX_CONTENT_LENGTH);
            let truncation_note = if truncated_by_read_limit {
                format!(
                    "\n\n[Content truncated. Read {total_read} bytes from stream (max: {MAX_RESPONSE_READ_SIZE} bytes). Output limited to {MAX_CONTENT_LENGTH} bytes]"
                )
            } else {
                format!("\n\n[Content truncated. Total length: {} bytes]", text.len())
            };
            format!("{truncated}{truncation_note}")
        } else if truncated_by_read_limit {
            format!(
                "{text}\n\n[Response truncated during read. Read {total_read} bytes (max: {MAX_RESPONSE_READ_SIZE} bytes)]"
            )
        } else {
            text
        };

        // Store in cache
        {
            let mut cache = self.cache.write().await;
            cache.cleanup(); // Clean up expired entries
            cache.set(normalized_url, text.clone());
        }

        Ok((text, None))
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert HTML to markdown-like text for better readability
fn html_to_markdown(html: &str) -> String {
    // Use html2text for conversion with reasonable width
    html2text::from_read(html.as_bytes(), 100)
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("web_fetch", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "format": "uri",
                    "description": "The URL to fetch content from"
                },
                "prompt": {
                    "type": "string",
                    "description": "What information to extract or how to process the content"
                }
            },
            "required": ["url", "prompt"]
        })
    }

    fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
        // Web fetching is generally safe but requires one-time confirmation
        ConfirmationLevel::Once
    }

    fn retry_config(&self) -> RetryConfig {
        // Network operations should retry on transient failures
        RetryConfig::NETWORK
    }

    fn is_readonly(&self) -> bool {
        true
    }

    fn requires_network(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        params: Value,
        _ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let url = crate::required_str(&params, "url")?;
        let prompt = crate::required_str(&params, "prompt")?;

        tracing::debug!("Fetching URL: {} with prompt: {}", url, prompt);

        // Fetch the URL
        match self.fetch_url(url, DEFAULT_TIMEOUT_SECS).await {
            Ok((content, redirect_notice)) => {
                redirect_notice.map_or_else(
                    || {
                        if content.is_empty() {
                            Ok(ToolOutput::error("No content retrieved from URL"))
                        } else {
                            // Format output with prompt context
                            let output = format!(
                                "## Content from: {url}\n\n### Prompt: {prompt}\n\n---\n\n{content}"
                            );
                            Ok(ToolOutput::success(output))
                        }
                    },
                    |notice| Ok(ToolOutput::success(notice)),
                )
            }
            Err(e) => {
                tracing::warn!(url = %url, error = ?e, "web_fetch request failed");
                Ok(ToolOutput::error(format!("Failed to fetch {url}: {e}")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    #[test]
    fn test_normalize_url() {
        assert_eq!(WebFetchTool::normalize_url("http://example.com"), "https://example.com");
        assert_eq!(WebFetchTool::normalize_url("https://example.com"), "https://example.com");
        assert_eq!(
            WebFetchTool::normalize_url("http://example.com/path?query=1"),
            "https://example.com/path?query=1"
        );
    }

    #[test]
    fn test_html_to_markdown() {
        let html = r"
            <html>
            <head><title>Test Page</title></head>
            <body>
                <h1>Hello World</h1>
                <p>This is a <strong>test</strong> paragraph.</p>
                <ul>
                    <li>Item 1</li>
                    <li>Item 2</li>
                </ul>
            </body>
            </html>
        ";

        let text = html_to_markdown(html);
        assert!(text.contains("Hello World"));
        assert!(text.contains("test"));
        assert!(text.contains("Item 1"));
    }

    #[test]
    fn test_cache() {
        let mut cache = FetchCache::new();

        // Test set and get
        cache.set("https://example.com".to_string(), "content".to_string());
        assert_eq!(cache.get("https://example.com"), Some("content".to_string()));

        // Test missing key
        assert_eq!(cache.get("https://other.com"), None);
    }

    #[test]
    fn test_tool_name() {
        let tool = WebFetchTool::new();
        assert_eq!(tool.name(), "web_fetch");
    }

    #[test]
    fn test_tool_schema() {
        let tool = WebFetchTool::new();
        let schema = tool.parameters_schema();

        assert!(schema.get("properties").is_some());
        assert!(schema["properties"].get("url").is_some());
        assert!(schema["properties"].get("prompt").is_some());

        let required = schema["required"].as_array().expect("required array");
        assert!(required.contains(&json!("url")));
        assert!(required.contains(&json!("prompt")));
    }

    #[test]
    fn test_confirmation_level() {
        let tool = WebFetchTool::new();
        let params = json!({"url": "https://example.com", "prompt": "test"});
        assert_eq!(tool.confirmation_level(&params), ConfirmationLevel::Once);
    }

    #[tokio::test]
    async fn test_missing_params() {
        let tool = WebFetchTool::new();
        let ctx = ToolContext::default();

        // Missing URL
        let params = json!({"prompt": "test"});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());

        // Missing prompt
        let params = json!({"url": "https://example.com"});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalid_url() {
        let tool = WebFetchTool::new();
        let ctx = ToolContext::default();

        let params = json!({"url": "not-a-valid-url", "prompt": "test"});
        let result = tool.execute(params, &ctx).await.expect("should return ToolOutput");
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_unsupported_scheme() {
        let tool = WebFetchTool::new();
        let ctx = ToolContext::default();

        let params = json!({"url": "ftp://example.com/file", "prompt": "test"});
        let result = tool.execute(params, &ctx).await.expect("should return ToolOutput");
        assert!(result.is_error);
        assert!(result.content.contains("Unsupported URL scheme"));
    }

    // Integration tests that actually fetch URLs should be marked #[ignore]
    #[tokio::test]
    #[ignore]
    async fn test_fetch_real_url() {
        let tool = WebFetchTool::new();
        let ctx = ToolContext::default();

        let params = json!({
            "url": "https://httpbin.org/html",
            "prompt": "Extract the main content"
        });
        let result = tool.execute(params, &ctx).await.expect("should succeed");

        assert!(!result.is_error);
        assert!(result.content.contains("httpbin.org"));
    }

    #[tokio::test]
    #[ignore]
    async fn test_cache_works() {
        let tool = WebFetchTool::new();
        let ctx = ToolContext::default();

        let params = json!({
            "url": "https://httpbin.org/html",
            "prompt": "test"
        });

        // First fetch
        let _ = tool.execute(params.clone(), &ctx).await.expect("first fetch");

        // Second fetch should use cache (would be faster)
        let result = tool.execute(params, &ctx).await.expect("second fetch");
        assert!(!result.is_error);
    }
}
