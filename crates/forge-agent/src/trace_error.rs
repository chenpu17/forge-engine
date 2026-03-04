//! Error types for tracing system.

use thiserror::Error;

/// Tracing error type.
#[derive(Debug, Error)]
pub enum TraceError {
    /// IO error during trace writing.
    #[error("Failed to write trace: {0}")]
    IoError(#[from] std::io::Error),

    /// Trace channel is full.
    #[error("Trace channel is full")]
    ChannelFull,

    /// Trace channel is closed.
    #[error("Trace channel is closed")]
    ChannelClosed,

    /// Failed to serialize event.
    #[error("Failed to serialize event: {0}")]
    SerializationError(#[from] serde_json::Error),
}

/// Result type for tracing operations.
pub type Result<T> = std::result::Result<T, TraceError>;
