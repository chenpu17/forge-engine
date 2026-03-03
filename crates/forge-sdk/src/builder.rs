//! Builder for `ForgeSDK`.
//!
//! Provides a fluent API for constructing [`ForgeSDK`] instances with custom
//! configuration, providers, and tools.

use crate::config::ForgeConfig;
use crate::error::Result;
use forge_llm::LlmProvider;
use forge_session::SessionManager;
use forge_tools::Tool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Builder for constructing [`ForgeSDK`] instances.
///
/// # Example
///
/// ```ignore
/// use forge_sdk::ForgeSDKBuilder;
///
/// let sdk = ForgeSDKBuilder::new()
///     .working_dir("/my/project")
///     .model("claude-sonnet-4-20250514")
///     .with_builtin_tools()
///     .build()?;
/// ```
pub struct ForgeSDKBuilder {
    config: ForgeConfig,
    provider: Option<Arc<dyn LlmProvider>>,
    session_manager: Option<Arc<dyn SessionManager>>,
    tools: Vec<Arc<dyn Tool>>,
    use_builtin_tools: bool,
    memory_dir_override: Option<PathBuf>,
}

impl ForgeSDKBuilder {
    /// Create a new builder with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: ForgeConfig::default(),
            provider: None,
            session_manager: None,
            tools: Vec::new(),
            use_builtin_tools: false,
            memory_dir_override: None,
        }
    }

    /// Create a builder pre-populated with an existing [`ForgeConfig`].
    ///
    /// Unlike [`new`](Self::new), this preserves **all** fields in the provided
    /// config — including `api_key`, `base_url`, `thinking`, `temperature`, and
    /// every other setting — without requiring individual setter calls.
    ///
    /// Use this when you already have a fully-configured [`ForgeConfig`] and
    /// want to pass it straight through to the SDK (e.g. from NAPI or PyO3
    /// bindings).
    #[must_use]
    pub fn from_forge_config(config: ForgeConfig) -> Self {
        Self {
            config,
            provider: None,
            session_manager: None,
            tools: Vec::new(),
            use_builtin_tools: false,
            memory_dir_override: None,
        }
    }

    // ---------------------------------------------------------------
    // LLM settings
    // ---------------------------------------------------------------

    /// Set the LLM model.
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.llm.model = model.into();
        self
    }

    /// Set the LLM provider name (e.g. `"anthropic"`, `"openai"`).
    #[must_use]
    pub fn provider_name(mut self, provider: impl Into<String>) -> Self {
        self.config.llm.provider = provider.into();
        self
    }

    /// Set a custom [`LlmProvider`] instance.
    #[must_use]
    pub fn provider(mut self, provider: Arc<dyn LlmProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Set the API key for the LLM provider.
    #[must_use]
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.config.llm.api_key = Some(key.into());
        self
    }

    /// Set the base URL for the LLM provider.
    #[must_use]
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.config.llm.base_url = Some(url.into());
        self
    }

    /// Set max tokens for generation.
    #[must_use]
    pub fn max_tokens(mut self, tokens: usize) -> Self {
        self.config.llm.max_tokens = tokens;
        self
    }

    /// Set temperature for generation.
    #[must_use]
    pub fn temperature(mut self, temp: f64) -> Self {
        self.config.llm.temperature = Some(temp);
        self
    }

    // ---------------------------------------------------------------
    // Working directory & persona
    // ---------------------------------------------------------------

    /// Set the working directory.
    #[must_use]
    pub fn working_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.working_dir = path.into();
        self
    }

    /// Set the prompts directory.
    #[must_use]
    pub fn prompts_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.prompts_dir = Some(path.into());
        self
    }

    /// Set the default persona.
    #[must_use]
    pub fn default_persona(mut self, persona: impl Into<String>) -> Self {
        self.config.default_persona = persona.into();
        self
    }

    /// Set the project prompt (from FORGE.md / CLAUDE.md).
    #[must_use]
    pub fn project_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.project_prompt = Some(prompt.into());
        self
    }

    // ---------------------------------------------------------------
    // Session
    // ---------------------------------------------------------------

    /// Set a custom [`SessionManager`].
    #[must_use]
    pub fn session_manager(mut self, manager: Arc<dyn SessionManager>) -> Self {
        self.session_manager = Some(manager);
        self
    }

    /// Set the session persistence format.
    #[must_use]
    pub fn session_persistence_format(
        mut self,
        format: forge_session::SessionPersistenceFormat,
    ) -> Self {
        self.config.session.persistence_format = format;
        self
    }

    /// Override the user-level data directory used for memory.
    ///
    /// Default: `~/.forge` (via `forge_infra::data_dir()`).
    #[must_use]
    pub fn memory_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.memory_dir_override = Some(path.into());
        self
    }

    // ---------------------------------------------------------------
    // Tools
    // ---------------------------------------------------------------

    /// Add a custom tool.
    #[must_use]
    pub fn tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Enable all built-in tools.
    #[must_use]
    pub fn with_builtin_tools(mut self) -> Self {
        self.use_builtin_tools = true;
        self
    }

    /// Disable specific tools by name.
    #[must_use]
    pub fn disable_tools(mut self, tools: Vec<String>) -> Self {
        self.config.tools.disabled = tools;
        self
    }

    /// Set bash timeout in seconds.
    #[must_use]
    pub fn bash_timeout(mut self, timeout: u64) -> Self {
        self.config.tools.bash_timeout = timeout;
        self
    }

    /// Set maximum tool output size in bytes.
    #[must_use]
    pub fn max_output_size(mut self, bytes: usize) -> Self {
        self.config.tools.max_output_size = bytes;
        self
    }

    /// Disable confirmation prompts.
    #[must_use]
    pub fn no_confirmation(mut self) -> Self {
        self.config.tools.require_confirmation = false;
        self
    }

    // ---------------------------------------------------------------
    // MCP
    // ---------------------------------------------------------------

    /// Enable MCP (Model Context Protocol) servers.
    #[must_use]
    pub fn with_mcp(mut self) -> Self {
        self.config.tools.mcp.mcp_enabled = true;
        self
    }

    /// Set the path to MCP configuration file.
    #[must_use]
    pub fn mcp_config_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.tools.mcp.mcp_config_path = Some(path.into());
        self.config.tools.mcp.mcp_enabled = true;
        self
    }

    // ---------------------------------------------------------------
    // Tool descriptions
    // ---------------------------------------------------------------

    /// Set a custom description for a specific tool.
    #[must_use]
    pub fn tool_description(
        mut self,
        tool_name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        self.config.tools.tool_descriptions.insert(tool_name.into(), description.into());
        self
    }

    /// Set custom descriptions for multiple tools at once.
    #[must_use]
    pub fn tool_descriptions(mut self, descriptions: HashMap<String, String>) -> Self {
        self.config.tools.tool_descriptions.extend(descriptions);
        self
    }

    /// Set a custom directory for tool description markdown files.
    #[must_use]
    pub fn tool_descriptions_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.config.tools.tool_descriptions_dir = Some(path.into());
        self
    }

    // ---------------------------------------------------------------
    // Build
    // ---------------------------------------------------------------

    /// Build the [`ForgeSDK`] instance.
    ///
    /// Validates configuration, creates the provider and session manager
    /// (if not supplied), and registers tools.
    ///
    /// **Note:** This creates a temporary tokio runtime internally.
    /// If you are already inside an async context, use [`build_async`](Self::build_async)
    /// instead to avoid nested-runtime panics.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeError`](crate::ForgeError) if configuration validation
    /// fails, provider creation fails, or tool registration fails.
    pub fn build(self) -> Result<crate::ForgeSDK> {
        // Apply custom tool descriptions BEFORE creating tools
        Self::apply_tool_descriptions(&self.config);

        let memory_dir = self.memory_dir_override.clone().unwrap_or_else(forge_infra::data_dir);

        crate::ForgeSDK::from_builder(
            self.config,
            self.provider,
            self.session_manager,
            self.tools,
            self.use_builtin_tools,
            memory_dir,
        )
    }

    /// Async version of [`build`](Self::build).
    ///
    /// Use this when already inside a tokio runtime (e.g. from an `async fn main`
    /// or a spawned task) to avoid the nested-runtime panic that `build()` would cause.
    ///
    /// # Errors
    ///
    /// Returns [`ForgeError`](crate::ForgeError) if configuration validation
    /// fails, provider creation fails, or tool registration fails.
    pub async fn build_async(self) -> Result<crate::ForgeSDK> {
        Self::apply_tool_descriptions(&self.config);

        let memory_dir = self.memory_dir_override.clone().unwrap_or_else(forge_infra::data_dir);

        crate::ForgeSDK::from_builder_async(
            self.config,
            self.provider,
            self.session_manager,
            self.tools,
            self.use_builtin_tools,
            memory_dir,
        )
        .await
    }

    /// Apply custom tool descriptions from config.
    fn apply_tool_descriptions(config: &ForgeConfig) {
        use forge_tools::ToolDescriptions;

        // 1. Load from custom directory if specified
        if let Some(ref dir) = config.tools.tool_descriptions_dir {
            ToolDescriptions::init(Some(dir.as_path()));
        }

        // 2. Apply programmatic overrides (highest priority)
        if !config.tools.tool_descriptions.is_empty() {
            ToolDescriptions::register_overrides(config.tools.tool_descriptions.clone());
        }
    }
}

impl Default for ForgeSDKBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ===================================================================
// Tests
// ===================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_defaults() {
        let builder = ForgeSDKBuilder::new();
        assert_eq!(builder.config.default_persona, "coder");
        assert_eq!(builder.config.llm.provider, "anthropic");
        assert!(!builder.use_builtin_tools);
    }

    #[test]
    fn test_builder_fluent_api() {
        let builder = ForgeSDKBuilder::new()
            .working_dir("/test")
            .model("test-model")
            .default_persona("researcher")
            .max_tokens(4096)
            .api_key("test-key")
            .base_url("https://api.example.com")
            .with_builtin_tools();

        assert_eq!(builder.config.working_dir, PathBuf::from("/test"));
        assert_eq!(builder.config.llm.model, "test-model");
        assert_eq!(builder.config.default_persona, "researcher");
        assert_eq!(builder.config.llm.max_tokens, 4096);
        assert_eq!(builder.config.llm.api_key, Some("test-key".to_string()));
        assert_eq!(builder.config.llm.base_url, Some("https://api.example.com".to_string()));
        assert!(builder.use_builtin_tools);
    }
}
