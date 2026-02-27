//! Error types for LLM operations

use std::time::Duration;
use thiserror::Error;

/// LLM-specific errors
#[derive(Debug, Error)]
pub enum LlmError {
    /// API error with status code
    #[error("API error: {status} - {message}")]
    ApiError {
        /// HTTP status code
        status: u16,
        /// Error message
        message: String,
    },

    /// Rate limited by the provider
    #[error("Rate limited, retry after {retry_after_secs}s")]
    RateLimited {
        /// Suggested retry delay in seconds
        retry_after_secs: u64,
    },

    /// Context length exceeded
    #[error("Context length exceeded: {current} > {max}")]
    ContextLengthExceeded {
        /// Current token count
        current: usize,
        /// Maximum allowed tokens
        max: usize,
    },

    /// Network error
    #[error("Network error: {0}")]
    NetworkError(String),

    /// Parse error (JSON, SSE, etc.)
    #[error("Parse error: {0}")]
    ParseError(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Provider unavailable
    #[error("Provider unavailable: {0}")]
    ProviderUnavailable(String),

    /// Model not supported
    #[error("Model not supported: {0}")]
    ModelNotSupported(String),

    /// Authentication failed
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),

    /// Request timeout
    #[error("Request timeout after {0}s")]
    Timeout(u64),

    /// Stream interrupted
    #[error("Stream interrupted: {0}")]
    StreamInterrupted(String),
}

impl LlmError {
    /// Check if this is an authentication error (401/403).
    pub const fn is_auth_error(&self) -> bool {
        matches!(
            self,
            Self::AuthenticationFailed(_)
                | Self::ApiError { status: 401, .. }
                | Self::ApiError { status: 403, .. }
        )
    }

    /// Check if this is a rate-limit error (429).
    pub const fn is_rate_limited(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::ApiError { status: 429, .. })
    }

    /// Check if the error is retryable
    pub const fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::NetworkError(_)
                | Self::Timeout(_)
                | Self::StreamInterrupted(_)
                | Self::ApiError { status: 500..=599, .. }
                | Self::ApiError { status: 429, .. }
        )
    }

    /// Get suggested retry delay
    pub const fn retry_delay(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after_secs } => {
                Some(Duration::from_secs(*retry_after_secs))
            }
            Self::ApiError { status: 429, .. } => Some(Duration::from_secs(5)),
            Self::NetworkError(_) | Self::Timeout(_) => Some(Duration::from_secs(1)),
            Self::StreamInterrupted(_) => Some(Duration::from_millis(500)),
            Self::ApiError { status: 500..=599, .. } => Some(Duration::from_secs(2)),
            _ => None,
        }
    }

    /// Create from reqwest error
    pub fn from_reqwest(e: &reqwest::Error) -> Self {
        if e.is_timeout() {
            Self::Timeout(30)
        } else if e.is_connect() {
            Self::NetworkError(format!("Connection failed: {e}"))
        } else if let Some(status) = e.status() {
            Self::ApiError { status: status.as_u16(), message: e.to_string() }
        } else {
            Self::NetworkError(e.to_string())
        }
    }
}

impl From<reqwest::Error> for LlmError {
    fn from(e: reqwest::Error) -> Self {
        Self::from_reqwest(&e)
    }
}

impl From<serde_json::Error> for LlmError {
    fn from(e: serde_json::Error) -> Self {
        Self::ParseError(e.to_string())
    }
}

/// Result type for LLM operations
pub type Result<T> = std::result::Result<T, LlmError>;
