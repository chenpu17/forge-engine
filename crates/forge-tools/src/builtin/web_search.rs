//! `WebSearch` tool for searching the web
//!
//! This tool allows the agent to search the web and use results
//! to answer questions with up-to-date information.
//!
//! # Architecture
//!
//! The `WebSearch` tool uses a pluggable provider architecture:
//! - `SearchProvider` trait defines the interface
//! - Multiple implementations available: Exa AI (default), Brave, `DuckDuckGo`
//! - Provider can be configured via environment variables or code
//!
//! # Providers
//!
//! - **Exa AI** (default): Free, AI-optimized, no API key required
//! - **Brave Search**: Requires `BRAVE_SEARCH_API_KEY`
//! - **`DuckDuckGo`**: Free but may be blocked by bot detection
//!
//! # Configuration
//!
//! Set `FORGE_SEARCH_PROVIDER` environment variable to choose provider:
//! - `exa` (default)
//! - `brave` (requires `BRAVE_SEARCH_API_KEY`)
//! - `duckduckgo`

use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, RetryConfig, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, OnceLock};

/// Build a client safely — falls back to `.no_proxy()` on failure
/// (avoids the `Client::new()` panic on macOS when system proxy query fails).
///
/// `rebuild` is called to produce a fresh builder with the provider's
/// original settings (UA, timeout, etc.) so the fallback path stays consistent.
fn safe_build_client<F>(builder: reqwest::ClientBuilder, rebuild: F) -> reqwest::Client
where
    F: FnOnce() -> reqwest::ClientBuilder,
{
    let primary = catch_unwind(AssertUnwindSafe(|| builder.build()));
    if let Ok(Ok(client)) = primary {
        return client;
    }
    match primary {
        Ok(Err(e)) => tracing::warn!("HTTP client build failed ({e}), retrying with no_proxy"),
        Err(_) => tracing::warn!("HTTP client build panicked, retrying with no_proxy"),
        Ok(Ok(_)) => {}
    }

    let fallback = catch_unwind(AssertUnwindSafe(|| rebuild().no_proxy().build()));
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

/// Build a client with proxy config, falling back safely on error.
///
/// `rebuild` produces a fresh builder with the provider's original settings.
fn build_client_with_proxy<F>(
    builder: reqwest::ClientBuilder,
    proxy: &forge_config::ProxyConfig,
    rebuild: F,
) -> reqwest::Client
where
    F: FnOnce() -> reqwest::ClientBuilder,
{
    match forge_infra::http::configure_http_client_builder(builder, Some(proxy)) {
        Ok(b) => safe_build_client(b, rebuild),
        Err(e) => {
            tracing::warn!("Proxy configuration failed ({e}), using direct connection");
            safe_build_client(rebuild().no_proxy(), reqwest::Client::builder)
        }
    }
}

/// Search result item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Result title
    pub title: String,
    /// Result URL
    pub url: String,
    /// Result snippet/description
    pub snippet: String,
}

impl SearchResult {
    /// Create a new search result
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        url: impl Into<String>,
        snippet: impl Into<String>,
    ) -> Self {
        Self { title: title.into(), url: url.into(), snippet: snippet.into() }
    }

    /// Format as markdown
    #[must_use]
    pub fn to_markdown(&self) -> String {
        format!("### [{}]({})\n\n{}\n", self.title, self.url, self.snippet)
    }
}

/// Search provider trait for pluggable search backends
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Perform a search query
    async fn search(
        &self,
        query: &str,
        allowed_domains: Option<&[String]>,
        blocked_domains: Option<&[String]>,
    ) -> std::result::Result<Vec<SearchResult>, String>;

    /// Get the provider name
    fn name(&self) -> &str;
}

/// Mock search provider for testing
pub struct MockSearchProvider;

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl SearchProvider for MockSearchProvider {
    async fn search(
        &self,
        query: &str,
        _allowed_domains: Option<&[String]>,
        _blocked_domains: Option<&[String]>,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        // Return mock results for testing
        Ok(vec![
            SearchResult::new(
                format!("Result 1 for: {query}"),
                "https://example.com/result1",
                "This is a mock search result snippet for testing purposes.",
            ),
            SearchResult::new(
                format!("Result 2 for: {query}"),
                "https://example.org/result2",
                "Another mock search result with relevant information.",
            ),
        ])
    }

    fn name(&self) -> &str {
        "mock"
    }
}

// =============================================================================
// Exa AI Search Provider (Default)
// =============================================================================

/// Exa AI search provider using MCP protocol
/// Free, AI-optimized search results, no API key required
/// API endpoint: <https://mcp.exa.ai/mcp>
pub struct ExaSearchProvider {
    client: reqwest::Client,
}

impl ExaSearchProvider {
    /// Base builder with Exa defaults.
    fn base_builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .user_agent("Forge/1.0")
            .timeout(std::time::Duration::from_secs(25))
    }

    /// Create a new Exa AI search provider (safe fallback, no proxy)
    #[must_use]
    pub fn new() -> Self {
        let client = safe_build_client(Self::base_builder(), Self::base_builder);
        Self { client }
    }

    /// Create with proxy configuration
    #[must_use]
    pub fn with_proxy(proxy: &forge_config::ProxyConfig) -> Self {
        let client = build_client_with_proxy(Self::base_builder(), proxy, Self::base_builder);
        Self { client }
    }
}

impl Default for ExaSearchProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl SearchProvider for ExaSearchProvider {
    async fn search(
        &self,
        query: &str,
        _allowed_domains: Option<&[String]>,
        _blocked_domains: Option<&[String]>,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        let start = std::time::Instant::now();
        tracing::debug!(provider = "exa", query_len = query.len(), "web_search request start");

        // Build MCP JSON-RPC request for Exa AI
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "web_search_exa",
                "arguments": {
                    "query": query,
                    "type": "auto",
                    "numResults": 8,
                    "livecrawl": "fallback",
                    "contextMaxCharacters": 10000
                }
            }
        });

        let response = self
            .client
            .post("https://mcp.exa.ai/mcp")
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("Exa search request failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(
                provider = "exa",
                status = %status,
                elapsed_ms = start.elapsed().as_millis(),
                "web_search request failed with non-success status"
            );
            return Err(format!("Exa search failed with status {status}: {body}"));
        }

        let response_text =
            response.text().await.map_err(|e| format!("Failed to read Exa response: {e}"))?;
        tracing::debug!(
            provider = "exa",
            elapsed_ms = start.elapsed().as_millis(),
            response_len = response_text.len(),
            "web_search response received"
        );

        // Parse SSE response - Exa returns Server-Sent Events
        for line in response_text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(json) = serde_json::from_str::<Value>(data) {
                    // Extract results from MCP response
                    if let Some(content) =
                        json.get("result").and_then(|r| r.get("content")).and_then(|c| c.as_array())
                    {
                        for item in content {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                return Ok(vec![SearchResult::new(
                                    format!("Search results for: {query}"),
                                    "https://exa.ai",
                                    text,
                                )]);
                            }
                        }
                    }
                }
            }
        }

        // Fallback: return raw response if parsing failed
        Ok(vec![SearchResult::new(
            format!("Search results for: {query}"),
            "https://exa.ai",
            &response_text,
        )])
    }

    fn name(&self) -> &str {
        "exa"
    }
}

// =============================================================================
// Brave Search Provider
// =============================================================================

/// Brave Search API provider
/// Get a free API key at: <https://brave.com/search/api/>
pub struct BraveSearchProvider {
    client: reqwest::Client,
    api_key: Option<String>,
}

impl BraveSearchProvider {
    /// Base builder with Brave defaults.
    fn base_builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .user_agent("Forge/1.0")
            .timeout(std::time::Duration::from_secs(30))
    }

    /// Create a new Brave Search provider (safe fallback, no proxy)
    #[must_use]
    pub fn new() -> Self {
        let api_key = std::env::var("BRAVE_SEARCH_API_KEY").ok();
        let client = safe_build_client(Self::base_builder(), Self::base_builder);
        Self { client, api_key }
    }

    /// Create with proxy configuration
    #[must_use]
    pub fn with_proxy(proxy: &forge_config::ProxyConfig) -> Self {
        let api_key = std::env::var("BRAVE_SEARCH_API_KEY").ok();
        let client = build_client_with_proxy(Self::base_builder(), proxy, Self::base_builder);
        Self { client, api_key }
    }

    /// Create with explicit API key
    #[must_use]
    pub fn with_api_key(api_key: impl Into<String>) -> Self {
        let client = safe_build_client(Self::base_builder(), Self::base_builder);
        Self { client, api_key: Some(api_key.into()) }
    }
}

impl Default for BraveSearchProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl SearchProvider for BraveSearchProvider {
    async fn search(
        &self,
        query: &str,
        _allowed_domains: Option<&[String]>,
        _blocked_domains: Option<&[String]>,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        let start = std::time::Instant::now();
        tracing::debug!(provider = "brave", query_len = query.len(), "web_search request start");

        let api_key = self.api_key.as_ref()
            .ok_or_else(|| "BRAVE_SEARCH_API_KEY environment variable not set. Get a free key at https://brave.com/search/api/".to_string())?;

        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count=10",
            urlencoding::encode(query)
        );

        let response = self
            .client
            .get(&url)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| format!("Search request failed: {e}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(
                provider = "brave",
                status = %status,
                elapsed_ms = start.elapsed().as_millis(),
                "web_search request failed with non-success status"
            );
            return Err(format!("Search failed with status {status}: {body}"));
        }

        let json: serde_json::Value =
            response.json().await.map_err(|e| format!("Failed to parse response: {e}"))?;
        tracing::debug!(
            provider = "brave",
            elapsed_ms = start.elapsed().as_millis(),
            "web_search response parsed"
        );

        // Parse Brave Search API response
        let mut results = Vec::new();

        if let Some(web) = json.get("web").and_then(|w| w.get("results")).and_then(|r| r.as_array())
        {
            for item in web.iter().take(10) {
                let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("");
                let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
                let snippet = item.get("description").and_then(|d| d.as_str()).unwrap_or("");

                if !url.is_empty() {
                    results.push(SearchResult::new(title, url, snippet));
                }
            }
        }

        if results.is_empty() {
            results.push(SearchResult::new(
                "No results found",
                "https://search.brave.com",
                "Try a different search query.",
            ));
        }

        Ok(results)
    }

    fn name(&self) -> &str {
        "brave"
    }
}

// =============================================================================
// DuckDuckGo Search Provider
// =============================================================================

/// `DuckDuckGo` search provider using the HTML interface
/// Note: This may be blocked by `DuckDuckGo`'s bot detection.
/// For production use, consider using `BraveSearchProvider` or another API.
pub struct DuckDuckGoProvider {
    client: reqwest::Client,
}

impl DuckDuckGoProvider {
    /// Base builder with DuckDuckGo defaults (browser-like UA to avoid bot detection).
    fn base_builder() -> reqwest::ClientBuilder {
        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .timeout(std::time::Duration::from_secs(30))
    }

    /// Create a new `DuckDuckGo` provider (safe fallback, no proxy)
    #[must_use]
    pub fn new() -> Self {
        let client = safe_build_client(Self::base_builder(), Self::base_builder);
        Self { client }
    }

    /// Create with proxy configuration
    #[must_use]
    pub fn with_proxy(proxy: &forge_config::ProxyConfig) -> Self {
        let client = build_client_with_proxy(Self::base_builder(), proxy, Self::base_builder);
        Self { client }
    }
}

impl Default for DuckDuckGoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl SearchProvider for DuckDuckGoProvider {
    async fn search(
        &self,
        query: &str,
        allowed_domains: Option<&[String]>,
        blocked_domains: Option<&[String]>,
    ) -> std::result::Result<Vec<SearchResult>, String> {
        let start = std::time::Instant::now();
        tracing::debug!(
            provider = "duckduckgo",
            query_len = query.len(),
            "web_search request start"
        );

        // Build search query with domain filters
        let mut search_query = query.to_string();

        // Add site: filters for allowed domains
        if let Some(domains) = allowed_domains {
            if !domains.is_empty() {
                let site_filter =
                    domains.iter().map(|d| format!("site:{d}")).collect::<Vec<_>>().join(" OR ");
                search_query = format!("{search_query} ({site_filter})");
            }
        }

        // Add -site: filters for blocked domains
        if let Some(domains) = blocked_domains {
            for domain in domains {
                search_query = format!("{search_query} -site:{domain}");
            }
        }

        // Use DuckDuckGo HTML search
        let url =
            format!("https://html.duckduckgo.com/html/?q={}", urlencoding::encode(&search_query));

        let response = self
            .client
            .get(&url)
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Accept-Language", "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|e| format!("Search request failed: {e}"))?;

        if !response.status().is_success() {
            tracing::warn!(
                provider = "duckduckgo",
                status = %response.status(),
                elapsed_ms = start.elapsed().as_millis(),
                "web_search request failed with non-success status"
            );
            return Err(format!("Search failed with status: {}", response.status()));
        }

        let html = response.text().await.map_err(|e| format!("Failed to read response: {e}"))?;
        tracing::debug!(
            provider = "duckduckgo",
            elapsed_ms = start.elapsed().as_millis(),
            response_len = html.len(),
            "web_search response received"
        );

        // Check for bot detection
        if html.contains("anomaly-modal")
            || html.contains("confirm this search was made by a human")
        {
            tracing::warn!(
                provider = "duckduckgo",
                elapsed_ms = start.elapsed().as_millis(),
                "web_search bot detection triggered"
            );
            return Err("DuckDuckGo bot detection triggered. Consider using Brave Search API instead (set BRAVE_SEARCH_API_KEY).".to_string());
        }

        // Parse results from HTML (basic parsing)
        let results = parse_duckduckgo_results(&html);
        tracing::debug!(
            provider = "duckduckgo",
            elapsed_ms = start.elapsed().as_millis(),
            result_count = results.len(),
            "web_search results parsed"
        );

        Ok(results)
    }

    fn name(&self) -> &str {
        "duckduckgo"
    }
}

/// Parse `DuckDuckGo` HTML results
fn parse_duckduckgo_results(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // Simple regex-based parsing for DuckDuckGo HTML results
    // Look for result blocks with class "result"
    let result_pattern =
        regex::Regex::new(r#"<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>([^<]*)</a>"#).ok();

    let snippet_pattern =
        regex::Regex::new(r#"<a[^>]*class="result__snippet"[^>]*>([^<]*(?:<[^>]*>[^<]*)*)</a>"#)
            .ok();

    if let Some(ref pattern) = result_pattern {
        for cap in pattern.captures_iter(html).take(10) {
            let url = cap.get(1).map_or("", |m| m.as_str());
            let title = cap.get(2).map_or("", |m| m.as_str());

            // Skip if URL is empty or not http(s)
            if url.is_empty() || (!url.starts_with("http://") && !url.starts_with("https://")) {
                continue;
            }

            // Try to find corresponding snippet
            let snippet = snippet_pattern.as_ref().map_or_else(String::new, |sp| {
                sp.captures(html)
                    .and_then(|c| c.get(1))
                    .map(|m| strip_html_tags(m.as_str()))
                    .unwrap_or_default()
            });

            results.push(SearchResult::new(html_entities_decode(title), url, snippet));
        }
    }

    // If parsing failed, return a message
    if results.is_empty() {
        results.push(SearchResult::new(
            "Search completed",
            "https://duckduckgo.com",
            "No results could be parsed. Try a different search query or use web_fetch to access specific URLs.",
        ));
    }

    results
}

/// Strip HTML tags from text
#[allow(clippy::expect_used)]
fn strip_html_tags(text: &str) -> String {
    let tag_pattern = regex::Regex::new(r"<[^>]*>").expect("valid regex");
    tag_pattern.replace_all(text, "").trim().to_string()
}

/// Decode basic HTML entities
fn html_entities_decode(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r#"Search the web and use results to answer questions with up-to-date information.

This tool:
- Performs web searches using the provided query
- Returns search results with titles, URLs, and snippets
- Supports domain filtering to include or exclude specific sites

IMPORTANT: After using search results to answer a question, you MUST include a "Sources:" section at the end of your response listing the relevant URLs as markdown hyperlinks.

Example format:
[Your answer here]

Sources:
- [Source Title 1](https://example.com/1)
- [Source Title 2](https://example.com/2)

Use this tool for:
- Finding current information beyond the knowledge cutoff
- Researching documentation and APIs
- Getting up-to-date news and events"#;

/// `WebSearch` tool for searching the web
pub struct WebSearchTool {
    /// Search provider
    provider: Arc<dyn SearchProvider>,
}

const SEARCH_TIMEOUT_SECS: u64 = 40;

// =============================================================================
// WebSearch Tool
// =============================================================================

/// Available search provider types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchProviderType {
    /// Exa AI - Free, AI-optimized (default)
    #[default]
    Exa,
    /// Brave Search - Requires API key
    Brave,
    /// `DuckDuckGo` - Free but may be blocked
    DuckDuckGo,
    /// Mock provider for testing
    Mock,
}

impl SearchProviderType {
    /// Parse from string (case-insensitive)
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "exa" => Some(Self::Exa),
            "brave" => Some(Self::Brave),
            "duckduckgo" | "ddg" => Some(Self::DuckDuckGo),
            "mock" => Some(Self::Mock),
            _ => None,
        }
    }

    /// Get provider name
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Exa => "exa",
            Self::Brave => "brave",
            Self::DuckDuckGo => "duckduckgo",
            Self::Mock => "mock",
        }
    }
}

impl WebSearchTool {
    /// Create a new `WebSearchTool` with the given provider
    #[must_use]
    pub fn new(provider: Arc<dyn SearchProvider>) -> Self {
        Self { provider }
    }

    /// Create with Exa AI provider (default, free, no API key required)
    #[must_use]
    pub fn with_exa() -> Self {
        Self::new(Arc::new(ExaSearchProvider::new()))
    }

    /// Create with Brave Search provider (requires `BRAVE_SEARCH_API_KEY`)
    #[must_use]
    pub fn with_brave() -> Self {
        Self::new(Arc::new(BraveSearchProvider::new()))
    }

    /// Create with `DuckDuckGo` provider (may be blocked by bot detection)
    #[must_use]
    pub fn with_duckduckgo() -> Self {
        Self::new(Arc::new(DuckDuckGoProvider::new()))
    }

    /// Create with mock provider (for testing)
    #[must_use]
    pub fn with_mock() -> Self {
        Self::new(Arc::new(MockSearchProvider))
    }

    /// Create with specified provider type
    #[must_use]
    pub fn with_provider_type(provider_type: SearchProviderType) -> Self {
        match provider_type {
            SearchProviderType::Exa => Self::with_exa(),
            SearchProviderType::Brave => Self::with_brave(),
            SearchProviderType::DuckDuckGo => Self::with_duckduckgo(),
            SearchProviderType::Mock => Self::with_mock(),
        }
    }

    /// Create with specified provider type and proxy configuration
    #[must_use]
    pub fn with_provider_type_and_proxy(
        provider_type: SearchProviderType,
        proxy: &forge_config::ProxyConfig,
    ) -> Self {
        match provider_type {
            SearchProviderType::Exa => Self::new(Arc::new(ExaSearchProvider::with_proxy(proxy))),
            SearchProviderType::Brave => {
                Self::new(Arc::new(BraveSearchProvider::with_proxy(proxy)))
            }
            SearchProviderType::DuckDuckGo => {
                Self::new(Arc::new(DuckDuckGoProvider::with_proxy(proxy)))
            }
            SearchProviderType::Mock => Self::with_mock(),
        }
    }

    /// Resolve provider selection with precedence:
    /// 1) explicit setting
    /// 2) `FORGE_SEARCH_PROVIDER` env
    /// 3) default (`exa`)
    fn resolve_provider_type(explicit_provider: Option<&str>) -> SearchProviderType {
        if let Some(provider) = explicit_provider {
            if let Some(provider_type) = SearchProviderType::from_str(provider) {
                return provider_type;
            }
            tracing::warn!("Unknown configured search provider '{provider}', falling back");
        }

        std::env::var("FORGE_SEARCH_PROVIDER")
            .ok()
            .and_then(|p| {
                SearchProviderType::from_str(&p).or_else(|| {
                    tracing::warn!("Unknown search provider '{p}', using default (exa)");
                    None
                })
            })
            .unwrap_or_default()
    }

    /// Create from explicit settings with optional proxy.
    #[must_use]
    pub fn from_settings(
        provider: Option<&str>,
        proxy: Option<&forge_config::ProxyConfig>,
    ) -> Self {
        let provider_type = Self::resolve_provider_type(provider);
        tracing::info!("Using search provider: {}", provider_type.name());

        if let Some(proxy) = proxy {
            Self::with_provider_type_and_proxy(provider_type, proxy)
        } else {
            Self::with_provider_type(provider_type)
        }
    }

    /// Create from environment configuration.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_settings(None, None)
    }

    /// Create from environment configuration with optional proxy.
    #[must_use]
    pub fn from_env_with_proxy(proxy: Option<&forge_config::ProxyConfig>) -> Self {
        Self::from_settings(None, proxy)
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::from_env()
    }
}

#[async_trait]
#[allow(clippy::unnecessary_literal_bound)]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("web_search", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query to use",
                    "minLength": 2
                },
                "allowed_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Only include results from these domains (optional)"
                },
                "blocked_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Exclude results from these domains (optional)"
                }
            },
            "required": ["query"]
        })
    }

    fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
        // Web searching is generally safe
        ConfirmationLevel::None
    }

    fn retry_config(&self) -> RetryConfig {
        // Keep retries limited to avoid long hangs in restricted networks.
        RetryConfig { max_retries: 1, initial_delay_ms: 1000, exponential_backoff: true }
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
        let query = crate::required_str(&params, "query")?;

        if query.len() < 2 {
            return Err(ToolError::InvalidParams(
                "Query must be at least 2 characters".to_string(),
            ));
        }

        // Parse domain filters using helper
        let allowed_domains_vec = crate::string_array(&params, "allowed_domains");
        let blocked_domains_vec = crate::string_array(&params, "blocked_domains");
        let allowed_domains: Option<Vec<String>> =
            if allowed_domains_vec.is_empty() { None } else { Some(allowed_domains_vec) };
        let blocked_domains: Option<Vec<String>> =
            if blocked_domains_vec.is_empty() { None } else { Some(blocked_domains_vec) };

        tracing::info!("Searching for: {} (provider: {})", query, self.provider.name());

        // Perform search
        let results = match tokio::time::timeout(
            std::time::Duration::from_secs(SEARCH_TIMEOUT_SECS),
            self.provider.search(query, allowed_domains.as_deref(), blocked_domains.as_deref()),
        )
        .await
        {
            Ok(Ok(results)) => results,
            Ok(Err(e)) => {
                tracing::warn!(
                    provider = %self.provider.name(),
                    error = %e,
                    "web_search request failed"
                );
                return Err(ToolError::ExecutionFailed(e));
            }
            Err(_) => {
                tracing::warn!(
                    provider = %self.provider.name(),
                    timeout_secs = SEARCH_TIMEOUT_SECS,
                    "web_search request timed out"
                );
                return Err(ToolError::Timeout(SEARCH_TIMEOUT_SECS));
            }
        };

        if results.is_empty() {
            return Ok(ToolOutput::success(format!("No results found for query: {query}")));
        }

        // Format results
        let mut output = format!("## Search Results for: {query}\n\n");

        for (i, result) in results.iter().enumerate() {
            use std::fmt::Write;
            let _ = write!(
                output,
                "{}. **[{}]({})**\n   {}\n\n",
                i + 1,
                result.title,
                result.url,
                result.snippet
            );
        }

        // Add sources section reminder
        output.push_str("\n---\n");
        output.push_str("*Remember to cite sources in your response using the URLs above.*\n");

        Ok(ToolOutput::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    #[test]
    fn test_search_result_new() {
        let result = SearchResult::new("Title", "https://example.com", "Snippet");
        assert_eq!(result.title, "Title");
        assert_eq!(result.url, "https://example.com");
        assert_eq!(result.snippet, "Snippet");
    }

    #[test]
    fn test_search_result_to_markdown() {
        let result = SearchResult::new("Test Title", "https://example.com", "Test snippet");
        let md = result.to_markdown();
        assert!(md.contains("[Test Title](https://example.com)"));
        assert!(md.contains("Test snippet"));
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_html_tags("<p>Hello</p>"), "Hello");
        assert_eq!(strip_html_tags("<a href='#'>Link</a>"), "Link");
        assert_eq!(strip_html_tags("No tags"), "No tags");
    }

    #[test]
    fn test_html_entities_decode() {
        assert_eq!(html_entities_decode("&amp;"), "&");
        assert_eq!(html_entities_decode("&lt;test&gt;"), "<test>");
        assert_eq!(html_entities_decode("&quot;quoted&quot;"), "\"quoted\"");
    }

    #[test]
    fn test_tool_name() {
        let tool = WebSearchTool::with_mock();
        assert_eq!(tool.name(), "web_search");
    }

    #[test]
    fn test_tool_schema() {
        let tool = WebSearchTool::with_mock();
        let schema = tool.parameters_schema();

        assert!(schema.get("properties").is_some());
        assert!(schema["properties"].get("query").is_some());
        assert!(schema["properties"].get("allowed_domains").is_some());
        assert!(schema["properties"].get("blocked_domains").is_some());

        let required = schema["required"].as_array().expect("required array");
        assert!(required.contains(&json!("query")));
    }

    #[test]
    fn test_confirmation_level() {
        let tool = WebSearchTool::with_mock();
        let params = json!({"query": "test"});
        assert_eq!(tool.confirmation_level(&params), ConfirmationLevel::None);
    }

    #[tokio::test]
    async fn test_mock_search() {
        let tool = WebSearchTool::with_mock();
        let ctx = ToolContext::default();

        let params = json!({"query": "rust programming"});
        let result = tool.execute(params, &ctx).await.expect("should succeed");

        assert!(!result.is_error);
        assert!(result.content.contains("rust programming"));
        assert!(result.content.contains("example.com"));
    }

    #[tokio::test]
    async fn test_missing_query() {
        let tool = WebSearchTool::with_mock();
        let ctx = ToolContext::default();

        let params = json!({});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_short_query() {
        let tool = WebSearchTool::with_mock();
        let ctx = ToolContext::default();

        let params = json!({"query": "a"});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_with_domain_filters() {
        let tool = WebSearchTool::with_mock();
        let ctx = ToolContext::default();

        let params = json!({
            "query": "test",
            "allowed_domains": ["example.com"],
            "blocked_domains": ["spam.com"]
        });
        let result = tool.execute(params, &ctx).await.expect("should succeed");

        assert!(!result.is_error);
    }

    // Integration test - requires network
    #[tokio::test]
    #[ignore]
    async fn test_duckduckgo_search() {
        let tool = WebSearchTool::with_duckduckgo();
        let ctx = ToolContext::default();

        let params = json!({"query": "rust programming language"});
        let result = tool.execute(params, &ctx).await.expect("should succeed");

        assert!(!result.is_error);
        println!("Search results:\n{}", result.content);
    }

    // Integration test - requires network (Exa AI)
    #[tokio::test]
    #[ignore]
    async fn test_exa_search() {
        let tool = WebSearchTool::with_exa();
        let ctx = ToolContext::default();

        let params = json!({"query": "rust programming language"});
        let result = tool.execute(params, &ctx).await.expect("should succeed");

        assert!(!result.is_error);
        println!("Exa search results:\n{}", result.content);
    }

    #[test]
    fn test_search_provider_type_from_str() {
        assert_eq!(SearchProviderType::from_str("exa"), Some(SearchProviderType::Exa));
        assert_eq!(SearchProviderType::from_str("EXA"), Some(SearchProviderType::Exa));
        assert_eq!(SearchProviderType::from_str("brave"), Some(SearchProviderType::Brave));
        assert_eq!(
            SearchProviderType::from_str("duckduckgo"),
            Some(SearchProviderType::DuckDuckGo)
        );
        assert_eq!(SearchProviderType::from_str("ddg"), Some(SearchProviderType::DuckDuckGo));
        assert_eq!(SearchProviderType::from_str("mock"), Some(SearchProviderType::Mock));
        assert_eq!(SearchProviderType::from_str("unknown"), None);
    }

    #[test]
    fn test_search_provider_type_name() {
        assert_eq!(SearchProviderType::Exa.name(), "exa");
        assert_eq!(SearchProviderType::Brave.name(), "brave");
        assert_eq!(SearchProviderType::DuckDuckGo.name(), "duckduckgo");
        assert_eq!(SearchProviderType::Mock.name(), "mock");
    }

    #[test]
    fn test_search_provider_type_default() {
        assert_eq!(SearchProviderType::default(), SearchProviderType::Exa);
    }

    #[test]
    fn test_with_provider_type() {
        let tool = WebSearchTool::with_provider_type(SearchProviderType::Mock);
        assert_eq!(tool.provider.name(), "mock");

        let tool = WebSearchTool::with_provider_type(SearchProviderType::Exa);
        assert_eq!(tool.provider.name(), "exa");
    }
}
