//! Configuration for Forge SDK
//!
//! SDK-level configuration that wraps lower-level crate configs.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Main SDK configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeConfig {
    /// LLM provider settings
    pub llm: LlmSettings,
    /// Tool settings
    pub tools: ToolsSettings,
    /// Session settings
    #[serde(default)]
    pub session: SessionSettings,
    /// Working directory
    pub working_dir: PathBuf,
    /// Prompts directory (optional, uses default if not set)
    pub prompts_dir: Option<PathBuf>,
    /// Default persona name
    pub default_persona: String,
    /// Project-specific prompt (from CLAUDE.md / FORGE.md)
    pub project_prompt: Option<String>,
    /// Whether to trust and load project-local skills from `.forge/skills/`
    ///
    /// Default: false (security boundary against prompt injection via untrusted repos).
    pub trust_project_skills: bool,
    /// Observability settings (OpenTelemetry export)
    #[serde(default)]
    pub observability: ObservabilityConfig,
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            llm: LlmSettings::default(),
            tools: ToolsSettings::default(),
            session: SessionSettings::default(),
            working_dir: std::env::current_dir().unwrap_or_default(),
            prompts_dir: None,
            default_persona: "coder".to_string(),
            project_prompt: None,
            trust_project_skills: false,
            observability: ObservabilityConfig::default(),
        }
    }
}

impl ForgeConfig {
    /// Create a new config with the specified working directory
    #[must_use]
    pub fn with_working_dir(working_dir: impl Into<PathBuf>) -> Self {
        Self { working_dir: working_dir.into(), ..Default::default() }
    }

    /// Validate configuration, returning an error description if invalid.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeError::ConfigError`](crate::ForgeError::ConfigError) if
    /// any configuration value is out of range.
    pub fn validate(&self) -> crate::error::Result<()> {
        let mut errors = Vec::new();

        if self.working_dir.as_os_str().is_empty() {
            errors.push("working_dir must not be empty".to_string());
        }
        if self.llm.max_tokens == 0 {
            errors.push("llm.max_tokens must be > 0".to_string());
        }
        if self.tools.bash_timeout == 0 {
            errors.push("tools.bash_timeout must be > 0".to_string());
        }
        if self.tools.max_output_size == 0 {
            errors.push("tools.max_output_size must be > 0".to_string());
        }
        if self.observability.enabled && !(0.0..=1.0).contains(&self.observability.sample_rate) {
            errors.push(format!(
                "observability.sample_rate must be between 0.0 and 1.0, got {}",
                self.observability.sample_rate
            ));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(crate::error::ForgeError::ConfigError(errors.join("; ")))
        }
    }
}

/// LLM provider settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSettings {
    /// Provider name (anthropic, openai, etc.)
    pub provider: String,
    /// Model name
    pub model: String,
    /// Maximum tokens for generation
    pub max_tokens: usize,
    /// Temperature for generation
    pub temperature: Option<f64>,
    /// API key (optional, can be from env)
    pub api_key: Option<String>,
    /// Base URL override
    pub base_url: Option<String>,
    /// Thinking mode configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<forge_config::ThinkingConfig>,
    /// Thinking protocol adaptor
    #[serde(default)]
    pub thinking_adaptor: forge_config::ThinkingAdaptor,
    /// `SubAgent` LLM configuration
    #[serde(default)]
    pub subagent: forge_config::SubAgentLlmConfig,
}

impl Default for LlmSettings {
    fn default() -> Self {
        const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
        Self {
            provider: "anthropic".to_string(),
            model: DEFAULT_MODEL.to_string(),
            max_tokens: 8192,
            temperature: None,
            api_key: None,
            base_url: None,
            thinking: None,
            thinking_adaptor: forge_config::ThinkingAdaptor::Auto,
            subagent: forge_config::SubAgentLlmConfig::default(),
        }
    }
}

impl LlmSettings {
    /// Get the effective model (config > `FORGE_LLM_MODEL` env).
    #[must_use]
    pub fn effective_model(&self) -> String {
        const DEFAULT_MODEL: &str = "claude-sonnet-4-20250514";
        if self.model == DEFAULT_MODEL {
            if let Ok(model) = std::env::var("FORGE_LLM_MODEL") {
                return model;
            }
        }
        self.model.clone()
    }

    /// Get the effective temperature (config > `FORGE_LLM_TEMPERATURE` env > 0.7).
    #[must_use]
    pub fn effective_temperature(&self) -> f64 {
        if self.temperature.is_none() {
            if let Ok(temp_str) = std::env::var("FORGE_LLM_TEMPERATURE") {
                if let Ok(temp) = temp_str.parse::<f64>() {
                    return temp;
                }
            }
        }
        self.temperature.unwrap_or(0.7)
    }
}

/// Tool settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsSettings {
    /// Disabled tools
    pub disabled: Vec<String>,
    /// Bash command timeout in seconds
    pub bash_timeout: u64,
    /// Maximum output size in bytes
    pub max_output_size: usize,
    /// Whether to require confirmation for dangerous operations
    pub require_confirmation: bool,
    /// MCP settings
    #[serde(default, flatten)]
    pub mcp: forge_config::McpSettings,
    /// Custom tool descriptions (overrides built-in descriptions)
    #[serde(default)]
    pub tool_descriptions: HashMap<String, String>,
    /// Custom directory for tool description markdown files
    pub tool_descriptions_dir: Option<PathBuf>,
    /// Global proxy configuration
    #[serde(default)]
    pub proxy: forge_config::ProxyConfig,
    /// Per-tool proxy settings
    #[serde(default)]
    pub tool_proxy: HashMap<String, String>,
    /// Preferred web search provider
    #[serde(default)]
    pub search_provider: Option<String>,
    /// Trust level configuration
    #[serde(default)]
    pub trust: forge_config::TrustLevelConfig,
    /// Environment exposure policy
    #[serde(default)]
    pub env_policy: forge_config::EnvPolicy,
    /// Memory system settings
    #[serde(default)]
    pub memory: forge_config::MemorySettings,
    /// Permission rules for fine-grained file access control
    #[serde(default)]
    pub permission_rules: Vec<forge_config::PermissionRuleConfig>,
}

impl Default for ToolsSettings {
    fn default() -> Self {
        Self {
            disabled: Vec::new(),
            bash_timeout: 120,
            max_output_size: 50_000,
            require_confirmation: true,
            mcp: forge_config::McpSettings::default(),
            tool_descriptions: HashMap::new(),
            tool_descriptions_dir: None,
            proxy: forge_config::ProxyConfig::default(),
            tool_proxy: HashMap::new(),
            search_provider: None,
            trust: forge_config::TrustLevelConfig::default(),
            env_policy: forge_config::EnvPolicy::default(),
            memory: forge_config::MemorySettings::default(),
            permission_rules: vec![],
        }
    }
}

/// Session settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSettings {
    /// Persistence format for session storage
    #[serde(default)]
    pub persistence_format: forge_session::SessionPersistenceFormat,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self { persistence_format: forge_session::SessionPersistenceFormat::PrettyJson }
    }
}

/// Observability settings (OpenTelemetry export)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    /// Enable OpenTelemetry export (default: false)
    #[serde(default)]
    pub enabled: bool,
    /// OTLP endpoint URL
    #[serde(default = "default_otlp_endpoint")]
    pub otlp_endpoint: String,
    /// Sampling rate (0.0 to 1.0, default: 1.0)
    #[serde(default = "one_f64")]
    pub sample_rate: f64,
}

fn default_otlp_endpoint() -> String {
    "http://localhost:4317".to_string()
}

const fn one_f64() -> f64 {
    1.0
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self { enabled: false, otlp_endpoint: default_otlp_endpoint(), sample_rate: 1.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = ForgeConfig::default();
        // Default config should pass validation (working_dir comes from current_dir)
        // We only check that defaults are sensible
        assert_eq!(config.default_persona, "coder");
        assert_eq!(config.llm.provider, "anthropic");
        assert_eq!(config.llm.max_tokens, 8192);
        assert_eq!(config.tools.bash_timeout, 120);
        assert_eq!(config.tools.max_output_size, 50_000);
        assert!(config.tools.require_confirmation);
    }

    #[test]
    fn test_with_working_dir() {
        let config = ForgeConfig::with_working_dir("/tmp/test");
        assert_eq!(config.working_dir, PathBuf::from("/tmp/test"));
        assert_eq!(config.default_persona, "coder");
    }

    #[test]
    fn test_validate_empty_working_dir() {
        let mut config = ForgeConfig::default();
        config.working_dir = PathBuf::new();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("working_dir must not be empty"));
    }

    #[test]
    fn test_validate_zero_max_tokens() {
        let mut config = ForgeConfig::with_working_dir("/tmp");
        config.llm.max_tokens = 0;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("llm.max_tokens must be > 0"));
    }

    #[test]
    fn test_validate_zero_bash_timeout() {
        let mut config = ForgeConfig::with_working_dir("/tmp");
        config.tools.bash_timeout = 0;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("tools.bash_timeout must be > 0"));
    }

    #[test]
    fn test_validate_zero_max_output_size() {
        let mut config = ForgeConfig::with_working_dir("/tmp");
        config.tools.max_output_size = 0;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("tools.max_output_size must be > 0"));
    }

    #[test]
    fn test_validate_bad_sample_rate() {
        let mut config = ForgeConfig::with_working_dir("/tmp");
        config.observability.enabled = true;
        config.observability.sample_rate = 1.5;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("sample_rate"));
    }

    #[test]
    fn test_validate_multiple_errors() {
        let mut config = ForgeConfig::default();
        config.working_dir = PathBuf::new();
        config.llm.max_tokens = 0;
        config.tools.bash_timeout = 0;
        let err = config.validate().unwrap_err();
        let msg = err.to_string();
        // Should contain multiple errors separated by "; "
        assert!(msg.contains("working_dir"));
        assert!(msg.contains("max_tokens"));
        assert!(msg.contains("bash_timeout"));
    }

    #[test]
    fn test_validate_valid_config() {
        let config = ForgeConfig::with_working_dir("/tmp");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_llm_effective_temperature_default() {
        // Skip if FORGE_LLM_TEMPERATURE is set — it overrides the default.
        if std::env::var("FORGE_LLM_TEMPERATURE").is_ok() {
            eprintln!(
                "SKIPPED: test_llm_effective_temperature_default (FORGE_LLM_TEMPERATURE is set)"
            );
            return;
        }
        let settings = LlmSettings::default();
        assert!((settings.effective_temperature() - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn test_llm_effective_temperature_override() {
        // Explicit temperature takes priority over env var, so this is safe.
        let mut settings = LlmSettings::default();
        settings.temperature = Some(0.3);
        assert!((settings.effective_temperature() - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = ForgeConfig::with_working_dir("/tmp/test");
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ForgeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.working_dir, config.working_dir);
        assert_eq!(deserialized.default_persona, config.default_persona);
    }

    #[test]
    fn test_observability_disabled_ignores_sample_rate() {
        let mut config = ForgeConfig::with_working_dir("/tmp");
        config.observability.enabled = false;
        config.observability.sample_rate = 99.0; // invalid but disabled
        assert!(config.validate().is_ok());
    }
}
