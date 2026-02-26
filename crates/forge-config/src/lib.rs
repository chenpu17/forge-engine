//! Configuration system for the Forge AI agent engine.
//!
//! Supports layered configuration:
//! 1. Default values
//! 2. Global config (`~/.forge/config.toml`)
//! 3. Project config (`.forge/config.toml`)
//! 4. Environment variables (`FORGE_*`)

pub mod llm;
pub mod loader;
pub mod logging;
pub mod memory;
pub mod model;
pub mod proxy;
pub mod session;
pub mod tools;
pub mod ui;

pub use llm::{LlmConfig, LlmMode, LlmProvider, SubAgentLlmConfig};
pub use loader::ConfigLoader;
pub use logging::LoggingConfig;
pub use memory::{MemoryMode, MemorySettings};
pub use model::{
    AuthConfig, Capabilities, EndpointConfig, ModelConfig, ProtocolType, ThinkingAdaptor,
    ThinkingCapability, ThinkingConfig, ThinkingEffort, VendorType,
};
pub use proxy::{ProxyAuth, ProxyConfig, ProxyMode};
pub use session::SessionConfig;
pub use tools::{
    EnvPolicy, EnvPolicyMode, McpSettings, OperationType, PermissionRuleConfig, PolicyAction,
    ToolsConfig, TrustLevelConfig, TrustLevelSetting,
};
pub use ui::UiConfig;

use serde::{Deserialize, Serialize};

/// Main configuration structure for the Forge engine.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForgeConfig {
    /// LLM settings.
    #[serde(default)]
    pub llm: LlmConfig,
    /// UI settings.
    #[serde(default)]
    pub ui: UiConfig,
    /// Tool settings.
    #[serde(default)]
    pub tools: ToolsConfig,
    /// Session settings.
    #[serde(default)]
    pub session: SessionConfig,
    /// Logging settings.
    #[serde(default)]
    pub logging: LoggingConfig,
}


impl ForgeConfig {
    /// Load configuration from all sources (convenience method).
    ///
    /// # Errors
    /// Returns error if config files are malformed or unreadable.
    pub fn load() -> Result<Self, ConfigError> {
        ConfigLoader::new().load()
    }

    /// Validate configuration values.
    ///
    /// # Errors
    /// Returns a description of the validation failure.
    pub fn validate(&self) -> Result<(), String> {
        if self.llm.max_tokens == 0 {
            return Err("llm.max_tokens must be > 0".into());
        }
        if !(0.0..=2.0).contains(&self.llm.temperature) {
            return Err("llm.temperature must be between 0.0 and 2.0".into());
        }
        if self.llm.timeout_secs == 0 {
            return Err("llm.timeout_secs must be > 0".into());
        }
        if self.tools.command_timeout == 0 {
            return Err("tools.command_timeout must be > 0".into());
        }
        if self.session.max_history == 0 {
            return Err("session.max_history must be > 0".into());
        }
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        if !valid_levels.contains(&self.logging.level.to_lowercase().as_str()) {
            return Err(format!(
                "logging.level must be one of: {}",
                valid_levels.join(", ")
            ));
        }
        Ok(())
    }
}

/// Configuration error type.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Failed to parse or load configuration.
    #[error("config error: {0}")]
    Load(String),
    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// TOML serialization error.
    #[error("toml error: {0}")]
    Toml(#[from] toml::ser::Error),
    /// TOML deserialization error.
    #[error("toml parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ForgeConfig::default();
        assert_eq!(config.llm.model, "claude-sonnet-4-5-20250929");
        assert_eq!(config.llm.max_tokens, 32768);
        assert_eq!(config.session.autosave_interval_secs, 30);
        assert_eq!(config.logging.level, "info");
    }

    #[test]
    fn test_config_serialization() {
        let config = ForgeConfig::default();
        let toml_str = toml::to_string(&config).expect("serialize");
        let parsed: ForgeConfig = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(config.llm.model, parsed.llm.model);
    }

    #[test]
    fn test_config_validation_valid() {
        let config = ForgeConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_max_tokens() {
        let mut config = ForgeConfig::default();
        config.llm.max_tokens = 0;
        let err = config.validate().unwrap_err();
        assert!(err.contains("max_tokens"));
    }

    #[test]
    fn test_config_validation_invalid_temperature() {
        let mut config = ForgeConfig::default();
        config.llm.temperature = 3.0;
        let err = config.validate().unwrap_err();
        assert!(err.contains("temperature"));
    }
}
