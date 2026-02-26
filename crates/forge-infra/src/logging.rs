//! Logging utilities with tracing integration.

use crate::{InfraError, Result};
use forge_config::LoggingConfig;
use std::fs::OpenOptions;
use tracing_subscriber::{
    fmt::{self, format::FmtSpan},
    layer::SubscriberExt,
    util::SubscriberInitExt,
    EnvFilter,
};

/// Initialize the logging system.
///
/// # Errors
/// Returns error if logging initialization fails.
pub fn init_logging(config: &LoggingConfig) -> Result<()> {
    let filter = EnvFilter::try_from_env("FORGE_LOG")
        .or_else(|_| EnvFilter::try_from_env("RUST_LOG"))
        .unwrap_or_else(|_| EnvFilter::new(&config.level));

    match (&config.file, config.console) {
        (Some(log_path), _) => {
            if let Some(parent) = log_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)?;
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    fmt::layer()
                        .with_writer(file)
                        .with_ansi(false)
                        .with_target(true)
                        .with_file(true)
                        .with_line_number(true)
                        .with_span_events(FmtSpan::CLOSE),
                )
                .try_init()
                .map_err(|e| {
                    InfraError::Config(format!("Failed to init logging: {e}"))
                })?;
        }
        (None, true) => {
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    fmt::layer()
                        .with_target(true)
                        .with_thread_ids(false)
                        .with_file(true)
                        .with_line_number(true)
                        .with_span_events(FmtSpan::CLOSE),
                )
                .try_init()
                .map_err(|e| {
                    InfraError::Config(format!("Failed to init logging: {e}"))
                })?;
        }
        (None, false) => {
            tracing_subscriber::registry()
                .with(filter)
                .try_init()
                .map_err(|e| {
                    InfraError::Config(format!("Failed to init logging: {e}"))
                })?;
        }
    }

    Ok(())
}

/// Initialize logging with a simple level string.
///
/// # Errors
/// Returns error if logging initialization fails.
pub fn init_logging_simple(level: &str) -> Result<()> {
    let config = LoggingConfig {
        level: level.to_string(),
        file: None,
        console: true,
        json: false,
    };
    init_logging(&config)
}
