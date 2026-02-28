//! HTTP client factory with proxy support
//!
//! Provides utilities for creating HTTP clients with configurable proxy settings.

use crate::secret::SecretStore;
use forge_config::{ProxyConfig, ProxyMode};
use thiserror::Error;

/// HTTP client errors
#[derive(Debug, Error)]
pub enum HttpError {
    /// Failed to build HTTP client
    #[error("Failed to build HTTP client: {0}")]
    BuildError(String),

    /// Invalid proxy URL
    #[error("Invalid proxy URL: {0}")]
    InvalidProxyUrl(String),

    /// Invalid proxy configuration
    #[error("Invalid proxy configuration: {0}")]
    InvalidProxyConfig(String),

    /// Proxy authentication failed
    #[error("Proxy authentication failed")]
    ProxyAuthFailed,
}

/// Result type for HTTP operations
pub type HttpResult<T> = Result<T, HttpError>;

fn host_matches_no_proxy(host: &str, pattern: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }

    if pattern == host {
        return true;
    }

    // Support "*.domain" pattern
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return host == suffix || host.ends_with(&format!(".{suffix}"));
    }

    // Support ".domain" pattern (leading dot, common in NO_PROXY)
    if let Some(suffix) = pattern.strip_prefix('.') {
        return host == suffix || host.ends_with(&format!(".{suffix}"));
    }

    false
}

fn should_bypass_proxy(url: &reqwest::Url, no_proxy: &[String]) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    no_proxy.iter().any(|entry| host_matches_no_proxy(host, entry))
}

/// Configure an HTTP client builder with proxy settings
///
/// # Errors
/// Returns error if proxy configuration is invalid
pub fn configure_http_client_builder(
    mut builder: reqwest::ClientBuilder,
    proxy: Option<&ProxyConfig>,
) -> HttpResult<reqwest::ClientBuilder> {
    let proxy_config = proxy.cloned().unwrap_or_default();

    proxy_config.validate().map_err(HttpError::InvalidProxyConfig)?;

    let mode = proxy_config.effective_mode();
    let tls_insecure = proxy_config.effective_danger_accept_invalid_certs();

    if tls_insecure {
        tracing::warn!("TLS certificate validation is disabled");
        builder = builder.danger_accept_invalid_certs(true);
    }

    match mode {
        ProxyMode::None => {
            builder = builder.no_proxy();
        }
        ProxyMode::System => {
            #[cfg(not(target_os = "windows"))]
            {
                builder = builder.no_proxy();
            }
        }
        ProxyMode::Environment => {
            // reqwest's default behavior reads HTTP_PROXY/HTTPS_PROXY
        }
        ProxyMode::Manual => {
            let no_proxy = proxy_config.effective_no_proxy();
            let http_url = proxy_config
                .effective_http_url()
                .map(|s| reqwest::Url::parse(&s).map_err(|e| HttpError::InvalidProxyUrl(e.to_string())))
                .transpose()?;
            let https_url = proxy_config
                .effective_https_url()
                .map(|s| reqwest::Url::parse(&s).map_err(|e| HttpError::InvalidProxyUrl(e.to_string())))
                .transpose()?;

            if http_url.is_some() || https_url.is_some() {
                let proxy = reqwest::Proxy::custom(move |url| {
                    if should_bypass_proxy(url, &no_proxy) {
                        return None;
                    }
                    match url.scheme() {
                        "http" => http_url.clone(),
                        "https" => https_url.clone(),
                        _ => None,
                    }
                });

                let proxy = if let Some(ref auth) = proxy_config.auth {
                    let keychain_key = auth
                        .password_keychain_key
                        .as_deref()
                        .unwrap_or(crate::secret::GLOBAL_PROXY_KEY);

                    let password = auth.resolve_password(keychain_key, |key| {
                        crate::secret::default_store().get(key).ok().flatten()
                    });

                    if let Some(pwd) = password {
                        proxy.basic_auth(&auth.username, &pwd)
                    } else {
                        proxy
                    }
                } else {
                    proxy
                };

                builder = builder.proxy(proxy);
            }
        }
    }

    Ok(builder)
}

/// Create an HTTP client with the specified proxy configuration
///
/// # Errors
/// Returns error if client construction fails
pub fn create_http_client(proxy: Option<&ProxyConfig>) -> HttpResult<reqwest::Client> {
    let builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .connect_timeout(std::time::Duration::from_secs(30));

    configure_http_client_builder(builder, proxy)?
        .build()
        .map_err(|e| HttpError::BuildError(e.to_string()))
}

/// Create an HTTP client with no proxy (direct connection)
///
/// # Errors
/// Returns error if client construction fails
pub fn create_direct_client() -> HttpResult<reqwest::Client> {
    create_http_client(Some(&ProxyConfig::none()))
}

/// Create an HTTP client using system proxy settings
///
/// # Errors
/// Returns error if client construction fails
pub fn create_system_proxy_client() -> HttpResult<reqwest::Client> {
    create_http_client(Some(&ProxyConfig::system()))
}

/// Create an HTTP client for long-lived connections (SSE, WebSocket, etc.)
///
/// No global timeout — suitable for indefinite streams.
///
/// # Errors
/// Returns error if client construction fails
pub fn create_streaming_client(proxy: Option<&ProxyConfig>) -> HttpResult<reqwest::Client> {
    let builder = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30));

    configure_http_client_builder(builder, proxy)?
        .build()
        .map_err(|e| HttpError::BuildError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_direct_client() {
        assert!(create_direct_client().is_ok());
    }

    #[test]
    fn test_create_system_proxy_client() {
        assert!(create_system_proxy_client().is_ok());
    }

    #[test]
    fn test_create_client_with_manual_proxy() {
        let proxy = ProxyConfig::manual("http://localhost:8080");
        assert!(create_http_client(Some(&proxy)).is_ok());
    }

    #[test]
    fn test_create_client_default() {
        assert!(create_http_client(None).is_ok());
    }

    #[test]
    fn test_host_matches_no_proxy() {
        assert!(host_matches_no_proxy("localhost", "localhost"));
        assert!(host_matches_no_proxy("api.example.com", "*.example.com"));
        assert!(host_matches_no_proxy("api.example.com", ".example.com"));
        assert!(!host_matches_no_proxy("other.com", "*.example.com"));
        assert!(!host_matches_no_proxy("host", ""));
    }
}
