//! LLM configuration types.

use serde::{Deserialize, Serialize};

/// LLM API mode — determines which API format to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LlmMode {
    /// Anthropic Messages API format.
    #[default]
    #[serde(rename = "anthropic")]
    Anthropic,
    /// `OpenAI` Chat Completions API format.
    #[serde(rename = "openai.chat")]
    OpenAIChat,
    /// `OpenAI` Responses API format (newer).
    #[serde(rename = "openai.responses")]
    OpenAIResponses,
}

impl LlmMode {
    /// Parse from string (for environment variable).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "anthropic" => Some(Self::Anthropic),
            "openai.chat" | "openai" => Some(Self::OpenAIChat),
            "openai.responses" => Some(Self::OpenAIResponses),
            _ => None,
        }
    }

    /// Get default base URL for this mode.
    #[must_use]
    pub const fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::OpenAIChat | Self::OpenAIResponses => "https://api.openai.com",
        }
    }
}

/// LLM provider type (legacy, kept for backward compatibility).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LlmProvider {
    /// Anthropic Claude models.
    #[default]
    Anthropic,
    /// `OpenAI` GPT models (also compatible with OpenAI-compatible APIs).
    OpenAI,
    /// Ollama local models.
    Ollama,
}

impl LlmProvider {
    /// Infer provider from model name.
    #[must_use]
    pub fn from_model(model: &str) -> Self {
        let m = model.to_lowercase();
        if m.starts_with("claude") {
            Self::Anthropic
        } else if m.starts_with("gpt") || m.starts_with("o1") || m.starts_with("o3") {
            Self::OpenAI
        } else if m.starts_with("llama")
            || m.starts_with("mistral")
            || m.starts_with("codellama")
        {
            Self::Ollama
        } else {
            Self::OpenAI
        }
    }

    /// Get default base URL for this provider.
    #[must_use]
    pub const fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::OpenAI => "https://api.openai.com",
            Self::Ollama => "http://localhost:11434",
        }
    }
}

fn default_model() -> String {
    "claude-sonnet-4-5-20250929".to_string()
}

const fn default_max_tokens() -> usize {
    32768
}

const fn default_temperature() -> f32 {
    0.7
}

const fn default_timeout_secs() -> u64 {
    300
}

const fn default_true() -> bool {
    true
}

const fn default_subagent_max_concurrent() -> usize {
    5
}

const fn default_subagent_max_nesting() -> usize {
    2
}

/// LLM-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// LLM API mode (anthropic, openai.chat, openai.responses).
    #[serde(default)]
    pub mode: Option<LlmMode>,
    /// LLM provider (legacy, kept for backward compatibility).
    #[serde(default)]
    pub provider: Option<LlmProvider>,
    /// Default model.
    #[serde(default = "default_model")]
    pub model: String,
    /// API key (use env `FORGE_LLM_API_KEY`).
    pub api_key: Option<String>,
    /// Maximum output tokens.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: usize,
    /// Temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// API base URL (for proxy or custom endpoints).
    pub base_url: Option<String>,
    /// Request timeout (seconds).
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Enable prompt caching (Anthropic only).
    #[serde(default = "default_true")]
    pub enable_cache: bool,
    /// Ignore environment variables for model selection (useful for testing).
    #[serde(default)]
    pub ignore_env: bool,
    /// Sub-agent configuration.
    #[serde(default)]
    pub subagent: SubAgentLlmConfig,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            mode: None,
            provider: None,
            model: default_model(),
            api_key: None,
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            base_url: None,
            timeout_secs: default_timeout_secs(),
            enable_cache: true,
            ignore_env: false,
            subagent: SubAgentLlmConfig::default(),
        }
    }
}

impl LlmConfig {
    /// Get the effective LLM mode.
    ///
    /// Priority: `FORGE_LLM_MODE` env > config > inferred from model.
    #[must_use]
    pub fn effective_mode(&self) -> LlmMode {
        if let Ok(mode_str) = std::env::var("FORGE_LLM_MODE") {
            if let Some(mode) = LlmMode::parse(&mode_str) {
                return mode;
            }
        }
        if let Some(mode) = self.mode {
            return mode;
        }
        let m = self.model.to_lowercase();
        if m.starts_with("claude") {
            LlmMode::Anthropic
        } else {
            LlmMode::OpenAIChat
        }
    }

    /// Get the effective provider (explicit or inferred from model).
    #[must_use]
    pub fn effective_provider(&self) -> LlmProvider {
        self.provider
            .unwrap_or_else(|| LlmProvider::from_model(&self.model))
    }

    /// Get the effective base URL.
    ///
    /// Priority: `FORGE_LLM_BASE_URL` env > config > mode default.
    #[must_use]
    pub fn effective_base_url(&self) -> String {
        if let Ok(url) = std::env::var("FORGE_LLM_BASE_URL") {
            return url;
        }
        if let Some(ref url) = self.base_url {
            return url.clone();
        }
        self.effective_mode().default_base_url().to_string()
    }

    /// Get API key from environment variable or config.
    ///
    /// Priority: `FORGE_LLM_API_KEY` env > config > provider-specific env vars.
    #[must_use]
    pub fn effective_api_key(&self) -> Option<String> {
        if let Ok(key) = std::env::var("FORGE_LLM_API_KEY") {
            return Some(key);
        }
        if let Some(ref key) = self.api_key {
            return Some(key.clone());
        }
        match self.effective_provider() {
            LlmProvider::Anthropic => std::env::var("ANTHROPIC_API_KEY").ok(),
            LlmProvider::OpenAI => std::env::var("OPENAI_API_KEY").ok(),
            LlmProvider::Ollama => None,
        }
    }

    /// Get the effective model.
    ///
    /// Priority: `FORGE_LLM_MODEL` env > config (unless `ignore_env` is true).
    #[must_use]
    pub fn effective_model(&self) -> String {
        if !self.ignore_env {
            if let Ok(model) = std::env::var("FORGE_LLM_MODEL") {
                return model;
            }
        }
        self.model.clone()
    }
}

/// Sub-agent LLM configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentLlmConfig {
    /// Fast model for quick tasks.
    #[serde(default)]
    pub fast_model: Option<String>,
    /// Default model for sub-agents (inherits from parent if not set).
    #[serde(default)]
    pub default_model: Option<String>,
    /// Powerful model for complex tasks.
    #[serde(default)]
    pub powerful_model: Option<String>,
    /// Maximum concurrent sub-agent tasks.
    #[serde(default = "default_subagent_max_concurrent")]
    pub max_concurrent: usize,
    /// Maximum nesting depth for sub-agent calls.
    #[serde(default = "default_subagent_max_nesting")]
    pub max_nesting_depth: usize,
}

impl Default for SubAgentLlmConfig {
    fn default() -> Self {
        Self {
            fast_model: None,
            default_model: None,
            powerful_model: None,
            max_concurrent: default_subagent_max_concurrent(),
            max_nesting_depth: default_subagent_max_nesting(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_inference() {
        assert_eq!(LlmProvider::from_model("claude-sonnet-4-5"), LlmProvider::Anthropic);
        assert_eq!(LlmProvider::from_model("gpt-4o"), LlmProvider::OpenAI);
        assert_eq!(LlmProvider::from_model("o1-preview"), LlmProvider::OpenAI);
        assert_eq!(LlmProvider::from_model("llama3"), LlmProvider::Ollama);
        assert_eq!(LlmProvider::from_model("unknown-model"), LlmProvider::OpenAI);
    }

    #[test]
    fn test_effective_provider() {
        let config = LlmConfig {
            provider: Some(LlmProvider::OpenAI),
            model: "claude-sonnet".to_string(),
            ..Default::default()
        };
        assert_eq!(config.effective_provider(), LlmProvider::OpenAI);

        let config = LlmConfig {
            provider: None,
            model: "gpt-4o".to_string(),
            ..Default::default()
        };
        assert_eq!(config.effective_provider(), LlmProvider::OpenAI);
    }
}
