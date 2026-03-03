//! Logging configuration.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_log_level() -> String {
    "info".to_string()
}

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error).
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Log file path.
    pub file: Option<PathBuf>,
    /// Output to console (stderr).
    #[serde(default)]
    pub console: bool,
    /// Use JSON format.
    #[serde(default)]
    pub json: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self { level: default_log_level(), file: None, console: false, json: false }
    }
}
