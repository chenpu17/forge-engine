//! Proxy configuration types.

use serde::{Deserialize, Serialize};

/// Proxy mode — determines how to configure HTTP proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ProxyMode {
    /// No proxy, direct connection (default on macOS/Linux).
    #[default]
    None,
    /// Use system proxy settings (default on Windows).
    System,
    /// Use `HTTP_PROXY`/`HTTPS_PROXY` environment variables.
    Environment,
    /// Manual proxy configuration.
    Manual,
}


impl ProxyMode {
    /// Parse from string (for environment variable).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "none" | "direct" => Some(Self::None),
            "system" => Some(Self::System),
            "environment" | "env" => Some(Self::Environment),
            "manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

/// Proxy authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyAuth {
    /// Username for proxy authentication.
    pub username: String,
    /// Plain text password (not recommended, for development only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Environment variable name to read password from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_env: Option<String>,
    /// Whether to read password from system keychain.
    #[serde(default)]
    pub password_from_keychain: bool,
    /// Override which keychain entry to use for password lookup.
    #[serde(skip)]
    pub password_keychain_key: Option<String>,
}

impl ProxyAuth {
    /// Resolve the actual password from various sources.
    ///
    /// Priority: keychain > env var > plain text.
    pub fn resolve_password<F>(&self, keychain_key: &str, keychain_lookup: F) -> Option<String>
    where
        F: FnOnce(&str) -> Option<String>,
    {
        if self.password_from_keychain {
            if let Some(pwd) = keychain_lookup(keychain_key) {
                return Some(pwd);
            }
        }
        if let Some(ref env_var) = self.password_env {
            if let Ok(pwd) = std::env::var(env_var) {
                return Some(pwd);
            }
        }
        self.password.clone()
    }
}

fn default_no_proxy() -> Vec<String> {
    vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ]
}

fn parse_env_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Proxy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Proxy mode.
    #[serde(default)]
    pub mode: ProxyMode,
    /// HTTP proxy URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_url: Option<String>,
    /// HTTPS proxy URL (defaults to `http_url` if not specified).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub https_url: Option<String>,
    /// Proxy authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<ProxyAuth>,
    /// Addresses that should bypass the proxy.
    #[serde(default = "default_no_proxy")]
    pub no_proxy: Vec<String>,
    /// Disable TLS certificate validation (INSECURE).
    #[serde(default)]
    pub danger_accept_invalid_certs: bool,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            mode: ProxyMode::default(),
            http_url: None,
            https_url: None,
            auth: None,
            no_proxy: default_no_proxy(),
            danger_accept_invalid_certs: false,
        }
    }
}

impl ProxyConfig {
    /// Create a "no proxy" configuration.
    #[must_use]
    pub fn none() -> Self {
        Self {
            mode: ProxyMode::None,
            ..Default::default()
        }
    }

    /// Create a manual proxy configuration.
    #[must_use]
    pub fn manual(http_url: impl Into<String>) -> Self {
        Self {
            mode: ProxyMode::Manual,
            http_url: Some(http_url.into()),
            ..Default::default()
        }
    }

    /// Get the effective proxy mode.
    ///
    /// Priority: `FORGE_PROXY_MODE` env > config.
    #[must_use]
    pub fn effective_mode(&self) -> ProxyMode {
        if let Ok(mode_str) = std::env::var("FORGE_PROXY_MODE") {
            if let Some(mode) = ProxyMode::parse(&mode_str) {
                return mode;
            }
        }
        self.mode
    }

    /// Get the effective HTTP proxy URL.
    ///
    /// Priority: `FORGE_HTTP_PROXY` > `HTTP_PROXY` > config.
    #[must_use]
    pub fn effective_http_url(&self) -> Option<String> {
        if let Ok(url) = std::env::var("FORGE_HTTP_PROXY") {
            return Some(url);
        }
        if let Ok(url) = std::env::var("HTTP_PROXY") {
            return Some(url);
        }
        if let Ok(url) = std::env::var("http_proxy") {
            return Some(url);
        }
        self.http_url.clone()
    }

    /// Get the effective HTTPS proxy URL.
    ///
    /// Priority: `FORGE_HTTPS_PROXY` > `HTTPS_PROXY` > config > `http_url`.
    #[must_use]
    pub fn effective_https_url(&self) -> Option<String> {
        if let Ok(url) = std::env::var("FORGE_HTTPS_PROXY") {
            return Some(url);
        }
        if let Ok(url) = std::env::var("HTTPS_PROXY") {
            return Some(url);
        }
        if let Ok(url) = std::env::var("https_proxy") {
            return Some(url);
        }
        self.https_url.clone().or_else(|| self.http_url.clone())
    }

    /// Check if proxy is effectively enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        match self.effective_mode() {
            ProxyMode::None => false,
            ProxyMode::System | ProxyMode::Environment => true,
            ProxyMode::Manual => self.effective_http_url().is_some(),
        }
    }

    /// Check if TLS certificate validation should be disabled.
    ///
    /// Priority: `FORGE_TLS_INSECURE` env > config.
    #[must_use]
    pub fn effective_danger_accept_invalid_certs(&self) -> bool {
        if let Ok(value) = std::env::var("FORGE_TLS_INSECURE") {
            if let Some(parsed) = parse_env_bool(&value) {
                return parsed;
            }
        }
        self.danger_accept_invalid_certs
    }
}
