//! Tool-related configuration (general-purpose parts).

use serde::{Deserialize, Serialize};

use crate::memory::MemorySettings;
use crate::proxy::ProxyConfig;

const fn default_command_timeout() -> u64 {
    120
}

fn default_dangerous_commands() -> Vec<String> {
    vec![
        "rm -rf".to_string(),
        "sudo".to_string(),
        "mkfs".to_string(),
    ]
}

/// Tool-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// Default command timeout (seconds).
    #[serde(default = "default_command_timeout")]
    pub command_timeout: u64,
    /// Commands that always require confirmation.
    #[serde(default = "default_dangerous_commands")]
    pub dangerous_commands: Vec<String>,
    /// Global proxy configuration for tools.
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// Memory system settings.
    #[serde(default)]
    pub memory: MemorySettings,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            command_timeout: default_command_timeout(),
            dangerous_commands: default_dangerous_commands(),
            proxy: ProxyConfig::default(),
            memory: MemorySettings::default(),
        }
    }
}
