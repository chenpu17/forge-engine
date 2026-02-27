//! Common error types for the Forge engine.

use thiserror::Error;

/// Top-level error type for forge operations.
#[derive(Debug, Error)]
pub enum ForgeError {
    /// Tool execution failed.
    #[error("tool error: {0}")]
    Tool(#[from] ToolError),

    /// LLM provider error.
    #[error("llm error: {0}")]
    Llm(String),

    /// Configuration error.
    #[error("config error: {0}")]
    Config(String),

    /// Session error.
    #[error("session error: {0}")]
    Session(String),

    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization error.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Cancelled by user.
    #[error("cancelled")]
    Cancelled,

    /// Generic error with message.
    #[error("{0}")]
    Other(String),
}

/// Tool-specific error type.
#[derive(Debug, Error)]
pub enum ToolError {
    /// Tool not found.
    #[error("tool not found: {0}")]
    NotFound(String),

    /// Tool execution failed.
    #[error("execution failed: {0}")]
    ExecutionFailed(String),

    /// Permission denied.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// Invalid parameters.
    #[error("invalid parameters: {0}")]
    InvalidParams(String),

    /// Timeout.
    #[error("timeout after {0}s")]
    Timeout(u64),

    /// Path requires user confirmation before access.
    #[error("path confirmation required: {path} - {reason}")]
    PathConfirmationRequired {
        /// The path that needs confirmation.
        path: String,
        /// Human-readable reason.
        reason: String,
    },

    /// IO error during tool execution.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience Result type for forge operations.
pub type Result<T> = std::result::Result<T, ForgeError>;
