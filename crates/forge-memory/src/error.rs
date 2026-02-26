//! Memory system error types.

use thiserror::Error;

/// Errors that can occur during memory operations.
#[derive(Debug, Error)]
pub enum MemoryError {
    /// Memory file not found (for operations requiring existence like delete/move).
    /// Note: `read_file` returns `Ok(None)` when file doesn't exist.
    #[error("Memory file not found: {0}")]
    FileNotFound(String),

    /// Failed to parse YAML frontmatter.
    #[error("Failed to parse frontmatter in {path}: {reason}")]
    ParseError {
        /// File path where parsing failed.
        path: String,
        /// Reason for the parse failure.
        reason: String,
    },

    /// Index file is corrupted or has invalid format.
    #[error("Index file corrupted: {0}")]
    IndexCorrupted(String),

    /// Path traversal attempt detected (security).
    #[error("Path traversal rejected: {0}")]
    PathTraversal(String),

    /// File content exceeds token limit.
    #[error("File too large (>{0} tokens)")]
    FileTooLarge(usize),

    /// Content contains sensitive data (API keys, tokens, etc.).
    #[error(
        "Sensitive content detected: {0}. Use environment variables or a secrets manager instead."
    )]
    SensitiveContent(String),

    /// Invalid operation (e.g., writing to auto-maintained index.md).
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    /// Underlying IO error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl MemoryError {
    /// Create a `ParseError` from path and reason.
    pub fn parse_error(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::ParseError { path: path.into(), reason: reason.into() }
    }

    /// Create a `FileNotFound` error.
    pub fn file_not_found(path: impl Into<String>) -> Self {
        Self::FileNotFound(path.into())
    }

    /// Create a `PathTraversal` error.
    pub fn path_traversal(path: impl Into<String>) -> Self {
        Self::PathTraversal(path.into())
    }

    /// Create an `IndexCorrupted` error.
    pub fn index_corrupted(reason: impl Into<String>) -> Self {
        Self::IndexCorrupted(reason.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = MemoryError::file_not_found("preferences.md");
        assert_eq!(err.to_string(), "Memory file not found: preferences.md");

        let err = MemoryError::parse_error("index.md", "missing scope field");
        assert!(err.to_string().contains("index.md"));
        assert!(err.to_string().contains("missing scope field"));

        let err = MemoryError::path_traversal("../../etc/passwd");
        assert!(err.to_string().contains("Path traversal"));

        let err = MemoryError::FileTooLarge(2000);
        assert!(err.to_string().contains("2000"));
    }
}
