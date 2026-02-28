//! Configuration bindings for NAPI

use napi_derive::napi;
use std::path::PathBuf;

/// Forge SDK configuration.
#[allow(missing_docs)]
#[napi]
#[derive(Debug, Clone)]
pub struct ForgeConfig {
    inner: forge_sdk::ForgeConfig,
}

#[allow(missing_docs)]
#[napi]
impl ForgeConfig {
    /// Create a new config with defaults.
    #[napi(constructor)]
    pub fn new() -> Self {
        Self { inner: forge_sdk::ForgeConfig::default() }
    }

    /// Create config from environment variables.
    #[napi]
    pub fn from_env() -> napi::Result<Self> {
        let api_key = std::env::var("FORGE_LLM_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .ok();
        let model = std::env::var("FORGE_LLM_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-20250514".to_string());
        let base_url = std::env::var("FORGE_LLM_BASE_URL").ok();
        let mode = std::env::var("FORGE_LLM_MODE")
            .unwrap_or_else(|_| "anthropic.messages".to_string());
        let provider = if mode.starts_with("openai") { "openai" } else { "anthropic" };

        let mut config = forge_sdk::ForgeConfig::default();
        config.llm.provider = provider.to_string();
        config.llm.model = model;
        config.llm.api_key = api_key;
        config.llm.base_url = base_url;
        Ok(Self { inner: config })
    }

    /// Set the LLM provider name.
    #[napi]
    pub fn set_provider(&mut self, provider: String) { self.inner.llm.provider = provider; }
    /// Set the LLM model name.
    #[napi]
    pub fn set_model(&mut self, model: String) { self.inner.llm.model = model; }
    /// Set the API key.
    #[napi]
    pub fn set_api_key(&mut self, api_key: String) { self.inner.llm.api_key = Some(api_key); }
    /// Set the base URL for the LLM provider.
    #[napi]
    pub fn set_base_url(&mut self, base_url: String) { self.inner.llm.base_url = Some(base_url); }
    /// Set the working directory.
    #[napi]
    pub fn set_working_dir(&mut self, dir: String) { self.inner.working_dir = PathBuf::from(dir); }
    /// Set the maximum tokens for generation.
    #[napi]
    pub fn set_max_tokens(&mut self, max_tokens: u32) { self.inner.llm.max_tokens = max_tokens as usize; }
    /// Set the temperature for generation.
    #[napi]
    pub fn set_temperature(&mut self, temperature: f64) { self.inner.llm.temperature = Some(temperature); }
    /// Set the bash command timeout in seconds.
    #[napi]
    pub fn set_bash_timeout(&mut self, timeout: u32) { self.inner.tools.bash_timeout = timeout as u64; }
    /// Set the maximum tool output size in bytes.
    #[napi]
    pub fn set_max_output_size(&mut self, size: u32) { self.inner.tools.max_output_size = size as usize; }
    /// Enable or disable MCP servers.
    #[napi]
    pub fn set_mcp_enabled(&mut self, enabled: bool) { self.inner.tools.mcp.mcp_enabled = enabled; }
    /// Set the MCP configuration file path.
    #[napi]
    pub fn set_mcp_config_path(&mut self, path: String) {
        self.inner.tools.mcp.mcp_config_path = Some(PathBuf::from(path));
        self.inner.tools.mcp.mcp_enabled = true;
    }
    /// Set the prompts directory.
    #[napi]
    pub fn set_prompts_dir(&mut self, dir: String) { self.inner.prompts_dir = Some(PathBuf::from(dir)); }
    /// Set the default persona name.
    #[napi]
    pub fn set_default_persona(&mut self, persona: String) { self.inner.default_persona = persona; }
    /// Set whether to trust project-level skills.
    #[napi]
    pub fn set_trust_project_skills(&mut self, trust: bool) { self.inner.trust_project_skills = trust; }

    /// Enable or disable thinking mode with optional budget.
    #[napi]
    pub fn set_thinking_enabled(&mut self, enabled: bool, budget_tokens: Option<u32>) {
        self.inner.llm.thinking = Some(forge_config::ThinkingConfig {
            enabled,
            budget_tokens: if enabled { budget_tokens.map(|t| t as usize) } else { None },
            effort: None,
            preserve_history: None,
        });
    }

    /// Set the thinking effort level ("low", "medium", "high").
    #[napi]
    pub fn set_thinking_effort(&mut self, effort: String) {
        let effort_enum = match effort.to_lowercase().as_str() {
            "low" => forge_config::ThinkingEffort::Low,
            "high" => forge_config::ThinkingEffort::High,
            _ => forge_config::ThinkingEffort::Medium,
        };
        if let Some(ref mut thinking) = self.inner.llm.thinking {
            thinking.effort = Some(effort_enum);
        } else {
            self.inner.llm.thinking = Some(forge_config::ThinkingConfig {
                enabled: true, budget_tokens: None,
                effort: Some(effort_enum), preserve_history: None,
            });
        }
    }

    /// Set the thinking protocol adaptor.
    #[napi]
    pub fn set_thinking_adaptor(&mut self, adaptor: String) {
        self.inner.llm.thinking_adaptor = match adaptor.to_lowercase().as_str() {
            "openaireasoning" => forge_config::ThinkingAdaptor::OpenaiReasoning,
            "glmthinking" => forge_config::ThinkingAdaptor::GlmThinking,
            "deepseekqwen" => forge_config::ThinkingAdaptor::DeepseekQwen,
            "minimaxtags" => forge_config::ThinkingAdaptor::MiniMaxTags,
            "none" => forge_config::ThinkingAdaptor::None,
            _ => forge_config::ThinkingAdaptor::Auto,
        };
    }

    /// Get the effective model name.
    #[napi]
    pub fn get_model(&self) -> String { self.inner.llm.effective_model() }
    /// Get the working directory path.
    #[napi]
    pub fn get_working_dir(&self) -> String { self.inner.working_dir.to_string_lossy().to_string() }

    /// Clone the inner SDK config.
    pub(crate) fn clone_inner(&self) -> forge_sdk::ForgeConfig { self.inner.clone() }
}

impl Default for ForgeConfig {
    fn default() -> Self { Self::new() }
}
