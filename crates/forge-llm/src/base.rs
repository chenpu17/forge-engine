//! Base utilities for LLM providers
//!
//! Provides common functionality shared across different LLM provider implementations,
//! reducing code duplication and ensuring consistent behavior.

/// Create an HTTP client with standard configuration (direct, no proxy)
///
/// # Arguments
/// * `use_http1_only` - If true, forces HTTP/1.1 only (useful for proxy servers)
///
/// # Returns
/// A configured `reqwest::Client` instance
#[must_use]
pub fn create_http_client(use_http1_only: bool) -> reqwest::Client {
    create_http_client_with_proxy(use_http1_only, None)
}

/// Create an HTTP client with optional proxy configuration
///
/// # Arguments
/// * `use_http1_only` - If true, forces HTTP/1.1 only (useful for proxy servers)
/// * `proxy` - Optional proxy configuration (None = direct connection)
///
/// # Returns
/// A configured `reqwest::Client` instance
#[must_use]
pub fn create_http_client_with_proxy(
    use_http1_only: bool,
    proxy: Option<&forge_config::ProxyConfig>,
) -> reqwest::Client {
    let mut builder = reqwest::Client::builder();
    if use_http1_only {
        builder = builder.http1_only();
    }

    let direct_proxy = forge_config::ProxyConfig::none();
    let proxy_config = proxy.unwrap_or(&direct_proxy);

    // Configure proxy based on mode
    use forge_config::ProxyMode;
    match proxy_config.effective_mode() {
        ProxyMode::None => {
            builder = builder.no_proxy();
        }
        ProxyMode::System => {
            // reqwest uses system proxy by default, no extra config needed
        }
        ProxyMode::Environment => {
            // reqwest reads HTTP_PROXY/HTTPS_PROXY by default
        }
        ProxyMode::Manual => {
            if let Some(url) = proxy_config.effective_http_url() {
                if let Ok(p) = reqwest::Proxy::http(&url) {
                    builder = builder.proxy(p);
                }
            }
            if let Some(url) = proxy_config.effective_https_url() {
                if let Ok(p) = reqwest::Proxy::https(&url) {
                    builder = builder.proxy(p);
                }
            }
        }
    }

    if proxy_config.effective_danger_accept_invalid_certs() {
        builder = builder.danger_accept_invalid_certs(true);
    }

    builder.build().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "Failed to build HTTP client, falling back to default");
        reqwest::Client::new()
    })
}

/// Extract the primary API key/token from an [`AuthConfig`](forge_config::AuthConfig).
///
/// Shared utility used by all provider `new_with_auth` methods to avoid
/// duplicating the same match logic (and the recursive `Multi` handling).
///
/// Returns an empty string for `Header` and `None` variants.
#[must_use]
pub fn extract_auth_token(auth: &forge_config::AuthConfig) -> String {
    match auth {
        forge_config::AuthConfig::Bearer { token } => token.clone(),
        forge_config::AuthConfig::ApiKey { api_key, .. } => api_key.clone(),
        forge_config::AuthConfig::Multi { credentials } => credentials
            .iter()
            .find(|c| c.is_configured())
            .map(|c| extract_auth_token(c))
            .unwrap_or_default(),
        forge_config::AuthConfig::Header { .. } => {
            tracing::warn!(
                "extract_auth_token called with Header auth config; \
                 returning empty token — provider may fail with 401"
            );
            String::new()
        }
        forge_config::AuthConfig::None => String::new(),
    }
}

/// Estimate token count from text using character ratio.
///
/// Delegates to [`forge_infra::token::estimate_tokens_by_ratio`] — the single
/// source of truth for this calculation across the codebase.
#[must_use]
pub fn estimate_tokens_by_ratio(text: &str, chars_per_token: f64) -> usize {
    forge_infra::token::estimate_tokens_by_ratio(text, chars_per_token)
}

/// Default characters per token for Claude models
pub const CLAUDE_CHARS_PER_TOKEN: f64 = 3.5;

/// Default characters per token for GPT models
pub const GPT_CHARS_PER_TOKEN: f64 = 4.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_http_client() {
        let client = create_http_client(false);
        assert!(client.get("https://example.com").build().is_ok());

        let client_http1 = create_http_client(true);
        assert!(client_http1.get("https://example.com").build().is_ok());

        let proxy = forge_config::ProxyConfig::none();
        let client_with_proxy = create_http_client_with_proxy(false, Some(&proxy));
        assert!(client_with_proxy.get("https://example.com").build().is_ok());
    }

    #[test]
    fn test_estimate_tokens_by_ratio() {
        // 100 characters at 4 chars/token = 25 tokens
        let text = "a".repeat(100);
        assert_eq!(estimate_tokens_by_ratio(&text, 4.0), 25);

        // 100 characters at 3.5 chars/token = 29 tokens (ceiling)
        assert_eq!(estimate_tokens_by_ratio(&text, 3.5), 29);

        // Empty string = 0 tokens
        assert_eq!(estimate_tokens_by_ratio("", 4.0), 0);
    }
}
