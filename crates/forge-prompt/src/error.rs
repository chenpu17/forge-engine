//! Prompt management errors.

use thiserror::Error;

/// Prompt management errors.
#[derive(Debug, Error)]
pub enum PromptError {
    /// Failed to load prompt file.
    #[error("failed to load prompt: {0}")]
    Load(String),
    /// Failed to parse config file.
    #[error("failed to parse config: {0}")]
    Parse(String),
    /// Persona not found.
    #[error("persona not found: {0}")]
    PersonaNotFound(String),
    /// Template not found.
    #[error("template not found: {0}")]
    TemplateNotFound(String),
    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for prompt operations.
pub type Result<T> = std::result::Result<T, PromptError>;
