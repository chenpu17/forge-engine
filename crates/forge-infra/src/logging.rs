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

/// Build a registry with filter + layer, apply common fields, and initialize.
///
/// The JSON and non-JSON layers have different generic types (`Json` vs `Full`),
/// so we use a macro to avoid boxing overhead while eliminating the repeated
/// `registry().with(filter).with(layer.with_target(...).with_file(...)).try_init()` pattern.
macro_rules! init_with_layer {
    ($filter:expr, $layer:expr, $init_err:expr) => {
        tracing_subscriber::registry()
            .with($filter)
            .with(
                $layer
                    .with_target(true)
                    .with_file(true)
                    .with_line_number(true)
                    .with_span_events(FmtSpan::CLOSE),
            )
            .try_init()
            .map_err($init_err)
    };
}

/// Initialize the logging system.
///
/// When `config.json` is true, output is formatted as structured JSON.
///
/// # Errors
/// Returns error if logging initialization fails.
pub fn init_logging(config: &LoggingConfig) -> Result<()> {
    let filter = EnvFilter::try_from_env("FORGE_LOG")
        .or_else(|_| EnvFilter::try_from_env("RUST_LOG"))
        .unwrap_or_else(|_| EnvFilter::new(&config.level));

    let init_err = |e| InfraError::Config(format!("Failed to init logging: {e}"));

    match (&config.file, config.console) {
        (Some(log_path), _) => {
            if let Some(parent) = log_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new().create(true).append(true).open(log_path)?;

            if config.json {
                init_with_layer!(
                    filter,
                    fmt::layer().json().with_writer(file).with_ansi(false),
                    init_err
                )?;
            } else {
                init_with_layer!(
                    filter,
                    fmt::layer().with_writer(file).with_ansi(false),
                    init_err
                )?;
            }
        }
        (None, true) => {
            if config.json {
                init_with_layer!(filter, fmt::layer().json(), init_err)?;
            } else {
                init_with_layer!(filter, fmt::layer(), init_err)?;
            }
        }
        // No file, no console — register filter only so spans/events are silently discarded.
        // This is intentional: the caller explicitly opted out of all output.
        (None, false) => {
            tracing_subscriber::registry().with(filter).try_init().map_err(init_err)?;
        }
    }

    Ok(())
}

/// Initialize logging with a simple level string.
///
/// # Errors
/// Returns error if logging initialization fails.
pub fn init_logging_simple(level: &str) -> Result<()> {
    let config = LoggingConfig { level: level.to_string(), file: None, console: true, json: false };
    init_logging(&config)
}
