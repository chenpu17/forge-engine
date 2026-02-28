//! Forge Infrastructure — foundational utilities.
//!
//! This crate provides cross-cutting infrastructure:
//! - Directory structure management ([`ForgeDirectories`])
//! - Key-value storage abstraction ([`KvStore`], [`JsonFileStore`])
//! - Structured logging initialization ([`init_logging`])
//! - Token estimation utilities ([`estimate_tokens`])
//!
//! Configuration types live in `forge-config`; domain types in `forge-domain`.

pub mod http;
pub mod i18n;
pub mod logging;
pub mod sandbox;
pub mod secret;
pub mod storage;
pub mod token;

pub use logging::{init_logging, init_logging_simple};
pub use storage::{ForgeDirectories, JsonFileStore, KvStore};
pub use token::{estimate_tokens, estimate_tokens_by_ratio, estimate_tokens_fast};

/// Infrastructure error type.
#[derive(Debug, thiserror::Error)]
pub enum InfraError {
    /// Configuration error.
    #[error("config error: {0}")]
    Config(String),
    /// Storage error.
    #[error("storage error: {0}")]
    Storage(String),
    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for infrastructure operations.
pub type Result<T> = std::result::Result<T, InfraError>;

/// Get the Forge data directory (`~/.forge`).
#[must_use]
pub fn data_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".forge")
}

/// Get the Forge config directory.
///
/// Returns `~/.forge` on all systems (unified with data directory).
#[must_use]
pub fn config_dir() -> std::path::PathBuf {
    data_dir()
}

/// Initialize the infrastructure.
///
/// Creates standard directories, loads config, and initializes logging.
///
/// # Errors
/// Returns error if initialization fails.
pub fn init() -> Result<(forge_config::ForgeConfig, ForgeDirectories)> {
    let dirs = ForgeDirectories::get_or_create()?;
    let config = forge_config::ConfigLoader::new()
        .load()
        .map_err(|e| InfraError::Config(e.to_string()))?;
    init_logging(&config.logging)?;
    Ok((config, dirs))
}

/// Initialize infrastructure with a pre-loaded config.
///
/// # Errors
/// Returns error if initialization fails.
pub fn init_with_config(config: &forge_config::ForgeConfig) -> Result<ForgeDirectories> {
    let dirs = ForgeDirectories::get_or_create()?;
    init_logging(&config.logging)?;
    Ok(dirs)
}
