//! Session configuration.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const fn default_autosave_interval() -> u64 {
    30
}

const fn default_max_history() -> usize {
    1000
}

/// Session-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Session storage directory.
    pub storage_dir: Option<PathBuf>,
    /// Auto-save interval (seconds).
    #[serde(default = "default_autosave_interval")]
    pub autosave_interval_secs: u64,
    /// Maximum history entries.
    #[serde(default = "default_max_history")]
    pub max_history: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            storage_dir: None,
            autosave_interval_secs: default_autosave_interval(),
            max_history: default_max_history(),
        }
    }
}
