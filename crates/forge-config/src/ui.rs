//! UI configuration types.

use serde::{Deserialize, Serialize};

fn default_language() -> String {
    "auto".to_string()
}

fn default_theme() -> String {
    "dark".to_string()
}

const fn default_true() -> bool {
    true
}

/// UI-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // config struct with boolean flags
pub struct UiConfig {
    /// Language ("en", "zh", "auto").
    #[serde(default = "default_language")]
    pub language: String,
    /// Theme name.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Enable syntax highlighting.
    #[serde(default = "default_true")]
    pub syntax_highlighting: bool,
    /// Show line numbers.
    #[serde(default = "default_true")]
    pub line_numbers: bool,
    /// Enable mouse support.
    #[serde(default = "default_true")]
    pub mouse_enabled: bool,
    /// Show timestamps in messages.
    #[serde(default)]
    pub show_timestamps: bool,
    /// Maximum message width.
    pub max_message_width: Option<u16>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            language: default_language(),
            theme: default_theme(),
            syntax_highlighting: true,
            line_numbers: true,
            mouse_enabled: true,
            show_timestamps: false,
            max_message_width: None,
        }
    }
}
