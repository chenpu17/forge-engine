//! OAuth 2.1 client for MCP authentication.
//!
//! Implements the OAuth 2.1 Authorization Code flow with PKCE
//! as required by MCP spec 2025-11-25.
//!
//! Flow:
//! 1. Discover server metadata via `/.well-known/oauth-authorization-server`
//! 2. Generate PKCE `code_verifier` + `code_challenge` (S256)
//! 3. Build authorization URL -> user opens in browser
//! 4. Local HTTP callback receives authorization code
//! 5. Exchange code for `access_token` + `refresh_token`
//! 6. Persist tokens to `~/.forge/mcp_tokens/<server_name>.json`
//! 7. Refresh tokens automatically when expired

use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use thiserror::Error;
use tracing::{debug, warn};

// ============================================================================
// Errors
// ============================================================================

/// OAuth 2.1 error types.
#[derive(Debug, Error)]
pub enum OAuthError {
    /// Server metadata discovery failed.
    #[error("OAuth discovery failed: {0}")]
    DiscoveryFailed(String),

    /// Authorization request failed.
    #[error("Authorization failed: {0}")]
    AuthorizationFailed(String),

    /// Token exchange failed.
    #[error("Token exchange failed: {0}")]
    TokenExchangeFailed(String),

    /// Token refresh failed.
    #[error("Token refresh failed: {0}")]
    RefreshFailed(String),

    /// Token persistence error.
    #[error("Token storage error: {0}")]
    StorageError(String),

    /// HTTP request error.
    #[error("HTTP error: {0}")]
    HttpError(String),
}

/// Result type for OAuth operations.
pub type OAuthResult<T> = std::result::Result<T, OAuthError>;

// ============================================================================
// Configuration
// ============================================================================

/// OAuth 2.1 configuration for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    /// OAuth client ID.
    pub client_id: String,
    /// OAuth client secret (optional for public clients).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Authorization endpoint URL (overrides discovery).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_url: Option<String>,
    /// Token endpoint URL (overrides discovery).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    /// Scopes to request.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Redirect URI (default: `http://127.0.0.1:0/callback`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
}

// ============================================================================
// Server Metadata (RFC 8414)
// ============================================================================

/// OAuth 2.0 Authorization Server Metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMetadata {
    /// Issuer identifier.
    pub issuer: String,
    /// Authorization endpoint.
    pub authorization_endpoint: String,
    /// Token endpoint.
    pub token_endpoint: String,
    /// Supported response types.
    #[serde(default)]
    pub response_types_supported: Vec<String>,
    /// Supported grant types.
    #[serde(default)]
    pub grant_types_supported: Vec<String>,
    /// Supported code challenge methods.
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
    /// Token endpoint auth methods.
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Vec<String>,
    /// Revocation endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,
    /// Registration endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
}

// ============================================================================
// Token Storage
// ============================================================================

/// Persisted OAuth tokens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenData {
    /// Access token.
    pub access_token: String,
    /// Token type (usually "Bearer").
    pub token_type: String,
    /// Refresh token (if provided).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Expiry timestamp (Unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Scopes granted.
    #[serde(default)]
    pub scope: Vec<String>,
}

impl TokenData {
    /// Check if the token is expired (with 60s buffer).
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|expires_at| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // 60-second buffer before actual expiry
            now + 60 >= expires_at
        })
    }
}

/// Token exchange response from the token endpoint.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

// ============================================================================
// PKCE
// ============================================================================

/// PKCE (Proof Key for Code Exchange) parameters.
#[derive(Debug, Clone)]
pub struct PkceParams {
    /// Code verifier (random string).
    pub code_verifier: String,
    /// Code challenge (S256 hash of verifier).
    pub code_challenge: String,
    /// Challenge method (always "S256").
    pub code_challenge_method: String,
}

impl PkceParams {
    /// Generate new PKCE parameters.
    #[must_use]
    pub fn generate() -> Self {
        let code_verifier = generate_code_verifier();
        let code_challenge = generate_code_challenge(&code_verifier);
        Self { code_verifier, code_challenge, code_challenge_method: "S256".to_string() }
    }
}

/// Generate a cryptographically random code verifier (43-128 chars, URL-safe).
fn generate_code_verifier() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.gen::<u8>()).collect();
    base64_url_encode(&bytes)
}

/// Generate S256 code challenge from verifier.
fn generate_code_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    base64_url_encode(&hash)
}

/// Base64 URL-safe encoding without padding.
fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

// ============================================================================
// OAuth 2.1 Client
// ============================================================================

/// OAuth 2.1 client for MCP server authentication.
pub struct OAuth21Client {
    /// HTTP client.
    client: reqwest::Client,
    /// OAuth configuration.
    config: OAuthConfig,
    /// Server name (for token storage).
    server_name: String,
    /// Discovered server metadata.
    metadata: Option<ServerMetadata>,
}

impl OAuth21Client {
    /// Create a new OAuth client.
    #[must_use]
    pub fn new(server_name: impl Into<String>, config: OAuthConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
            server_name: server_name.into(),
            metadata: None,
        }
    }

    /// Discover OAuth server metadata.
    ///
    /// Tries `/.well-known/oauth-authorization-server` first, then falls
    /// back to `/.well-known/openid-configuration`.
    ///
    /// # Errors
    /// Returns `OAuthError::DiscoveryFailed` if metadata cannot be fetched.
    ///
    /// # Panics
    /// This method will not panic in practice; the internal `expect` is guarded
    /// by a preceding assignment that guarantees the value is `Some`.
    pub async fn discover(&mut self, server_url: &str) -> OAuthResult<&ServerMetadata> {
        let base = server_url.trim_end_matches('/');

        // Try MCP-specific well-known first
        let well_known_url = format!("{base}/.well-known/oauth-authorization-server");
        debug!("Attempting OAuth discovery at {}", well_known_url);

        let result = self.fetch_metadata(&well_known_url).await;

        let metadata = if let Ok(m) = result { m } else {
            // Fallback to OpenID Connect discovery
            let oidc_url = format!("{base}/.well-known/openid-configuration");
            debug!("Falling back to OIDC discovery at {}", oidc_url);
            self.fetch_metadata(&oidc_url).await?
        };

        self.metadata = Some(metadata);
        // We just set `self.metadata` to `Some(...)` above, so `as_ref()` is guaranteed `Some`.
        #[allow(clippy::expect_used)]
        Ok(self.metadata.as_ref().expect("metadata was just set"))
    }

    /// Fetch metadata from a URL.
    async fn fetch_metadata(&self, url: &str) -> OAuthResult<ServerMetadata> {
        let response = self
            .client
            .get(url)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| OAuthError::DiscoveryFailed(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(OAuthError::DiscoveryFailed(format!(
                "HTTP {} from {}",
                response.status(),
                url
            )));
        }

        response
            .json::<ServerMetadata>()
            .await
            .map_err(|e| OAuthError::DiscoveryFailed(format!("Failed to parse metadata: {e}")))
    }

    /// Build the authorization URL for the user to visit.
    ///
    /// # Errors
    /// Returns `OAuthError::AuthorizationFailed` if endpoints are not configured.
    pub fn build_authorization_url(
        &self,
        pkce: &PkceParams,
        state: &str,
        redirect_uri: &str,
    ) -> OAuthResult<String> {
        let auth_endpoint = self
            .config
            .authorization_url
            .as_deref()
            .or_else(|| self.metadata.as_ref().map(|m| m.authorization_endpoint.as_str()))
            .ok_or_else(|| {
                OAuthError::AuthorizationFailed(
                    "No authorization endpoint configured or discovered".to_string(),
                )
            })?;

        let scopes = if self.config.scopes.is_empty() {
            String::new()
        } else {
            self.config.scopes.join(" ")
        };

        let mut url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method={}",
            auth_endpoint,
            urlencoding::encode(&self.config.client_id),
            urlencoding::encode(redirect_uri),
            urlencoding::encode(state),
            urlencoding::encode(&pkce.code_challenge),
            urlencoding::encode(&pkce.code_challenge_method),
        );

        if !scopes.is_empty() {
            use std::fmt::Write;
            let _ = write!(url, "&scope={}", urlencoding::encode(&scopes));
        }

        Ok(url)
    }

    /// Exchange an authorization code for tokens.
    ///
    /// # Errors
    /// Returns `OAuthError::TokenExchangeFailed` if the exchange fails.
    pub async fn exchange_code(
        &self,
        code: &str,
        pkce_verifier: &str,
        redirect_uri: &str,
    ) -> OAuthResult<TokenData> {
        let token_endpoint = self
            .config
            .token_url
            .as_deref()
            .or_else(|| self.metadata.as_ref().map(|m| m.token_endpoint.as_str()))
            .ok_or_else(|| {
                OAuthError::TokenExchangeFailed(
                    "No token endpoint configured or discovered".to_string(),
                )
            })?;

        let mut params = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", &self.config.client_id),
            ("code_verifier", pkce_verifier),
        ];

        // Add client_secret if configured (confidential client)
        let secret_ref;
        if let Some(ref secret) = self.config.client_secret {
            secret_ref = secret.clone();
            params.push(("client_secret", &secret_ref));
        }

        let response = self
            .client
            .post(token_endpoint)
            .timeout(std::time::Duration::from_secs(30))
            .form(&params)
            .send()
            .await
            .map_err(|e| OAuthError::TokenExchangeFailed(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(OAuthError::TokenExchangeFailed(format!(
                "HTTP {status}: {body}"
            )));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| OAuthError::TokenExchangeFailed(format!("Failed to parse response: {e}")))?;

        let token_data = Self::token_response_to_data(token_response);
        self.save_tokens(&token_data)?;

        Ok(token_data)
    }

    /// Refresh an access token using a refresh token.
    ///
    /// # Errors
    /// Returns `OAuthError::RefreshFailed` if the refresh fails.
    pub async fn refresh_token(&self, refresh_token: &str) -> OAuthResult<TokenData> {
        let token_endpoint = self
            .config
            .token_url
            .as_deref()
            .or_else(|| self.metadata.as_ref().map(|m| m.token_endpoint.as_str()))
            .ok_or_else(|| {
                OAuthError::RefreshFailed(
                    "No token endpoint configured or discovered".to_string(),
                )
            })?;

        let mut params = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &self.config.client_id),
        ];

        let secret_ref;
        if let Some(ref secret) = self.config.client_secret {
            secret_ref = secret.clone();
            params.push(("client_secret", &secret_ref));
        }

        let response = self
            .client
            .post(token_endpoint)
            .timeout(std::time::Duration::from_secs(30))
            .form(&params)
            .send()
            .await
            .map_err(|e| OAuthError::RefreshFailed(format!("HTTP request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(OAuthError::RefreshFailed(format!("HTTP {status}: {body}")));
        }

        let token_response: TokenResponse = response
            .json()
            .await
            .map_err(|e| OAuthError::RefreshFailed(format!("Failed to parse response: {e}")))?;

        let token_data = Self::token_response_to_data(token_response);
        self.save_tokens(&token_data)?;

        Ok(token_data)
    }

    /// Load persisted tokens for this server.
    ///
    /// # Errors
    /// Returns `OAuthError::StorageError` if tokens cannot be loaded.
    pub fn load_tokens(&self) -> OAuthResult<Option<TokenData>> {
        let path = self.token_path();
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| OAuthError::StorageError(format!("Failed to read token file: {e}")))?;

        let data: TokenData = serde_json::from_str(&content)
            .map_err(|e| OAuthError::StorageError(format!("Failed to parse token file: {e}")))?;

        Ok(Some(data))
    }

    /// Get a valid access token, refreshing if necessary.
    ///
    /// # Errors
    /// Returns an `OAuthError` if no tokens are available or refresh fails.
    pub async fn get_valid_token(&self) -> OAuthResult<String> {
        let tokens = self.load_tokens()?.ok_or_else(|| {
            OAuthError::AuthorizationFailed("No tokens available. Authorization required.".to_string())
        })?;

        if !tokens.is_expired() {
            return Ok(tokens.access_token);
        }

        // Try to refresh
        if let Some(ref refresh_token) = tokens.refresh_token {
            debug!("Access token expired, attempting refresh for server '{}'", self.server_name);
            match self.refresh_token(refresh_token).await {
                Ok(new_tokens) => return Ok(new_tokens.access_token),
                Err(e) => {
                    warn!(
                        "Token refresh failed for server '{}': {}. Re-authorization required.",
                        self.server_name, e
                    );
                    return Err(OAuthError::RefreshFailed(e.to_string()));
                }
            }
        }

        Err(OAuthError::AuthorizationFailed(
            "Access token expired and no refresh token available".to_string(),
        ))
    }

    /// Save tokens to disk.
    fn save_tokens(&self, data: &TokenData) -> OAuthResult<()> {
        let path = self.token_path();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| OAuthError::StorageError(format!("Failed to create directory: {e}")))?;
        }

        let json = serde_json::to_string_pretty(data)
            .map_err(|e| OAuthError::StorageError(format!("Failed to serialize tokens: {e}")))?;

        std::fs::write(&path, json)
            .map_err(|e| OAuthError::StorageError(format!("Failed to write token file: {e}")))?;

        // Set restrictive permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        debug!("Saved OAuth tokens for server '{}' to {:?}", self.server_name, path);
        Ok(())
    }

    /// Delete persisted tokens.
    ///
    /// # Errors
    /// Returns `OAuthError::StorageError` if deletion fails.
    pub fn delete_tokens(&self) -> OAuthResult<()> {
        let path = self.token_path();
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| OAuthError::StorageError(format!("Failed to delete token file: {e}")))?;
        }
        Ok(())
    }

    /// Get the token storage path.
    fn token_path(&self) -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".forge")
            .join("mcp_tokens")
            .join(format!("{}.json", self.server_name))
    }

    /// Convert a token response to token data.
    fn token_response_to_data(response: TokenResponse) -> TokenData {
        let expires_at = response.expires_in.map(|secs| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() + secs)
                .unwrap_or(0)
        });

        let scope = response
            .scope
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        TokenData {
            access_token: response.access_token,
            token_type: response.token_type,
            refresh_token: response.refresh_token,
            expires_at,
            scope,
        }
    }

    /// Get the server name.
    #[must_use]
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Get the OAuth config.
    #[must_use]
    pub const fn config(&self) -> &OAuthConfig {
        &self.config
    }

    /// Get discovered metadata (if any).
    #[must_use]
    pub const fn metadata(&self) -> Option<&ServerMetadata> {
        self.metadata.as_ref()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_pkce_generation() {
        let pkce = PkceParams::generate();
        assert!(!pkce.code_verifier.is_empty());
        assert!(!pkce.code_challenge.is_empty());
        assert_eq!(pkce.code_challenge_method, "S256");
        // Verifier and challenge should be different
        assert_ne!(pkce.code_verifier, pkce.code_challenge);
    }

    #[test]
    fn test_pkce_deterministic_challenge() {
        let verifier = "test_verifier_12345";
        let challenge1 = generate_code_challenge(verifier);
        let challenge2 = generate_code_challenge(verifier);
        assert_eq!(challenge1, challenge2);
    }

    #[test]
    fn test_token_data_not_expired() {
        let future_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() + 3600)
            .unwrap_or(0);

        let data = TokenData {
            access_token: "test".to_string(),
            token_type: "Bearer".to_string(),
            refresh_token: None,
            expires_at: Some(future_time),
            scope: vec![],
        };

        assert!(!data.is_expired());
    }

    #[test]
    fn test_token_data_expired() {
        let data = TokenData {
            access_token: "test".to_string(),
            token_type: "Bearer".to_string(),
            refresh_token: None,
            expires_at: Some(0), // epoch = definitely expired
            scope: vec![],
        };

        assert!(data.is_expired());
    }

    #[test]
    fn test_token_data_no_expiry() {
        let data = TokenData {
            access_token: "test".to_string(),
            token_type: "Bearer".to_string(),
            refresh_token: None,
            expires_at: None,
            scope: vec![],
        };

        assert!(!data.is_expired());
    }

    #[test]
    fn test_oauth_config_serde() {
        let config = OAuthConfig {
            client_id: "test-client".to_string(),
            client_secret: Some("secret".to_string()),
            authorization_url: Some("https://auth.example.com/authorize".to_string()),
            token_url: Some("https://auth.example.com/token".to_string()),
            scopes: vec!["read".to_string(), "write".to_string()],
            redirect_uri: None,
        };

        let json = serde_json::to_string(&config).expect("serialize");
        let parsed: OAuthConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.client_id, "test-client");
        assert_eq!(parsed.scopes.len(), 2);
    }

    #[test]
    fn test_build_authorization_url() {
        let config = OAuthConfig {
            client_id: "my-client".to_string(),
            client_secret: None,
            authorization_url: Some("https://auth.example.com/authorize".to_string()),
            token_url: None,
            scopes: vec!["read".to_string()],
            redirect_uri: None,
        };

        let client = OAuth21Client::new("test-server", config);
        let pkce = PkceParams::generate();
        let url = client
            .build_authorization_url(&pkce, "random-state", "http://127.0.0.1:8080/callback")
            .expect("build url");

        assert!(url.starts_with("https://auth.example.com/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=my-client"));
        assert!(url.contains("code_challenge="));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("scope=read"));
    }

    #[test]
    fn test_build_authorization_url_no_endpoint() {
        let config = OAuthConfig {
            client_id: "my-client".to_string(),
            client_secret: None,
            authorization_url: None,
            token_url: None,
            scopes: vec![],
            redirect_uri: None,
        };

        let client = OAuth21Client::new("test-server", config);
        let pkce = PkceParams::generate();
        let result =
            client.build_authorization_url(&pkce, "state", "http://127.0.0.1:8080/callback");
        assert!(result.is_err());
    }

    #[test]
    fn test_server_metadata_serde() {
        let json = r#"{
            "issuer": "https://auth.example.com",
            "authorization_endpoint": "https://auth.example.com/authorize",
            "token_endpoint": "https://auth.example.com/token",
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "code_challenge_methods_supported": ["S256"]
        }"#;

        let metadata: ServerMetadata = serde_json::from_str(json).expect("parse metadata");
        assert_eq!(metadata.issuer, "https://auth.example.com");
        assert!(metadata.code_challenge_methods_supported.contains(&"S256".to_string()));
    }
}
