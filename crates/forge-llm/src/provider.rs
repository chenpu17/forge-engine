//! LLM Provider Registry
//!
//! Manages multiple LLM providers and provides model-to-provider routing.

use crate::LlmProvider;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::factory::{FactoryError, ProviderFactory};
use forge_config::ModelConfig;

/// Model information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Model identifier (e.g., "claude-sonnet-4-5-20250929")
    pub id: String,
    /// Model display name
    pub name: String,
    /// Context window size in tokens
    pub context_window: usize,
    /// Maximum output tokens
    pub max_output_tokens: usize,
    /// Whether the model supports tool use
    pub supports_tools: bool,
    /// Whether the model supports vision/images
    pub supports_vision: bool,
}

impl ModelInfo {
    /// Create a new ModelInfo with basic settings
    pub fn new(id: impl Into<String>, name: impl Into<String>, context_window: usize) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            context_window,
            max_output_tokens: 8192,
            supports_tools: true,
            supports_vision: false,
        }
    }

    /// Set max output tokens
    #[must_use]
    pub fn with_max_output(mut self, max_output: usize) -> Self {
        self.max_output_tokens = max_output;
        self
    }

    /// Set vision support
    #[must_use]
    pub fn with_vision(mut self, supports: bool) -> Self {
        self.supports_vision = supports;
        self
    }

    /// Set tool support
    #[must_use]
    pub fn with_tools(mut self, supports: bool) -> Self {
        self.supports_tools = supports;
        self
    }
}

/// Provider Registry for managing multiple LLM providers
#[derive(Clone)]
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
    model_mapping: HashMap<String, String>,
    default_provider: Option<String>,
}

impl ProviderRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self { providers: HashMap::new(), model_mapping: HashMap::new(), default_provider: None }
    }

    /// Register a provider
    pub fn register(&mut self, provider: Arc<dyn LlmProvider>) {
        let provider_id = provider.id().to_string();
        for model in provider.supported_models() {
            self.model_mapping.insert(model.id, provider_id.clone());
        }
        if self.default_provider.is_none() {
            self.default_provider = Some(provider_id.clone());
        }
        self.providers.insert(provider_id, provider);
    }

    /// Register a provider from a `ModelConfig`.
    pub fn register_from_model_config(
        &mut self,
        config: &ModelConfig,
        proxy: Option<&forge_config::ProxyConfig>,
    ) -> std::result::Result<(), FactoryError> {
        let provider = ProviderFactory::create_with_proxy(config, proxy)?;
        let provider_id = provider.id().to_string();
        let registry_key = format!("{provider_id}:{}", config.id);

        self.model_mapping.insert(config.model_id.clone(), registry_key.clone());
        if config.id != config.model_id {
            self.model_mapping.insert(config.id.clone(), registry_key.clone());
        }
        self.providers.insert(registry_key, provider);
        Ok(())
    }

    /// Set the default provider
    pub fn set_default(&mut self, provider_id: impl Into<String>) {
        let id = provider_id.into();
        if self.providers.contains_key(&id) {
            self.default_provider = Some(id);
        }
    }

    /// Get provider by ID
    pub fn get(&self, provider_id: &str) -> Option<Arc<dyn LlmProvider>> {
        self.providers.get(provider_id).cloned()
    }

    /// Get provider for a specific model
    pub fn get_for_model(&self, model: &str) -> Option<Arc<dyn LlmProvider>> {
        if let Some(provider_id) = self.model_mapping.get(model) {
            return self.providers.get(provider_id).cloned();
        }
        if let Some(provider_id) = self.model_mapping.get("*") {
            return self.providers.get(provider_id).cloned();
        }
        for (model_id, provider_id) in &self.model_mapping {
            if model_id != "*" && model.starts_with(model_id) {
                return self.providers.get(provider_id).cloned();
            }
        }
        self.infer_provider(model)
    }

    fn infer_provider(&self, model: &str) -> Option<Arc<dyn LlmProvider>> {
        let model_lower = model.to_lowercase();

        if model_lower.contains("claude") {
            return self.providers.get("anthropic").cloned();
        }
        if model_lower.starts_with("gpt-")
            || model_lower.starts_with("o1-")
            || model_lower.starts_with("o3-")
        {
            return self.providers.get("openai").cloned();
        }
        if model_lower.starts_with("gemini") {
            if let Some(provider) = self.providers.get("gemini").cloned() {
                return Some(provider);
            }
            if let Some(provider) =
                self.providers.iter().find(|(k, _)| k.contains("gemini")).map(|(_, v)| v.clone())
            {
                return Some(provider);
            }
        }
        if model_lower.starts_with("glm-")
            || model_lower.starts_with("qwen")
            || model_lower.starts_with("deepseek")
            || model_lower.starts_with("llama")
            || model_lower.starts_with("mistral")
            || model_lower.starts_with("yi-")
        {
            return self.providers.get("openai").cloned();
        }
        self.default_provider.as_ref().and_then(|id| self.providers.get(id).cloned())
    }

    /// Get the default provider
    pub fn default_provider(&self) -> Option<Arc<dyn LlmProvider>> {
        self.default_provider.as_ref().and_then(|id| self.providers.get(id).cloned())
    }

    /// Get all available models from all providers
    pub fn available_models(&self) -> Vec<ModelInfo> {
        self.providers.values().flat_map(|p| p.supported_models()).collect()
    }

    /// Get all registered provider IDs
    pub fn provider_ids(&self) -> Vec<&str> {
        self.providers.keys().map(String::as_str).collect()
    }

    /// Check if a model is supported by any provider
    pub fn supports_model(&self, model: &str) -> bool {
        self.get_for_model(model).is_some()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, LlmConfig, LlmEventStream, Result, ToolDef};
    use async_trait::async_trait;

    struct MockProvider {
        id: String,
        models: Vec<ModelInfo>,
    }

    impl MockProvider {
        fn new(id: &str, models: Vec<ModelInfo>) -> Self {
            Self { id: id.to_string(), models }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn id(&self) -> &str { &self.id }
        fn name(&self) -> &str { &self.id }
        fn supported_models(&self) -> Vec<ModelInfo> { self.models.clone() }
        async fn chat_stream(
            &self, _messages: &[ChatMessage], _tools: Vec<ToolDef>, _config: &LlmConfig,
        ) -> Result<LlmEventStream> {
            unimplemented!("mock provider")
        }
    }

    #[test]
    fn test_register_provider() {
        let mut registry = ProviderRegistry::new();
        let provider = Arc::new(MockProvider::new(
            "test",
            vec![ModelInfo::new("test-model", "Test Model", 100_000)],
        ));
        registry.register(provider);
        assert!(registry.get("test").is_some());
        assert!(registry.supports_model("test-model"));
    }

    #[test]
    fn test_get_for_model() {
        let mut registry = ProviderRegistry::new();
        let anthropic = Arc::new(MockProvider::new(
            "anthropic",
            vec![ModelInfo::new("claude-3", "Claude 3", 200_000)],
        ));
        let openai =
            Arc::new(MockProvider::new("openai", vec![ModelInfo::new("gpt-4", "GPT-4", 128_000)]));
        registry.register(anthropic);
        registry.register(openai);
        assert_eq!(registry.get_for_model("claude-3").unwrap().id(), "anthropic");
        assert_eq!(registry.get_for_model("gpt-4").unwrap().id(), "openai");
    }

    #[test]
    fn test_infer_provider() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(MockProvider::new("anthropic", vec![])));
        registry.register(Arc::new(MockProvider::new("openai", vec![])));
        assert_eq!(registry.get_for_model("claude-sonnet-4-5-20250929").unwrap().id(), "anthropic");
        assert_eq!(registry.get_for_model("gpt-5-turbo").unwrap().id(), "openai");
    }

    #[test]
    fn test_default_provider() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(MockProvider::new("first", vec![])));
        registry.register(Arc::new(MockProvider::new("second", vec![])));
        assert_eq!(registry.default_provider().unwrap().id(), "first");
        registry.set_default("second");
        assert_eq!(registry.default_provider().unwrap().id(), "second");
    }

    #[test]
    fn test_available_models() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(MockProvider::new(
            "test",
            vec![
                ModelInfo::new("model-1", "Model 1", 100_000),
                ModelInfo::new("model-2", "Model 2", 200_000),
            ],
        )));
        assert_eq!(registry.available_models().len(), 2);
    }

    #[test]
    fn test_register_from_model_config_openai() {
        use forge_config::{
            AuthConfig, Capabilities, EndpointConfig, ModelConfig, ProtocolType, ThinkingAdaptor,
            ThinkingCapability, VendorType,
        };

        let mut registry = ProviderRegistry::new();
        let config = ModelConfig {
            id: "groq-llama".to_string(),
            name: "Groq Llama".to_string(),
            model_id: "llama-3.3-70b-versatile".to_string(),
            protocol: ProtocolType::Openai,
            vendor: VendorType::Custom,
            endpoint: EndpointConfig::new("https://api.groq.com/openai"),
            auth: AuthConfig::Bearer { token: "test-key".to_string() },
            proxy_name: None,
            description: None,
            capabilities: Capabilities::default(),
            thinking: None,
            thinking_capability: ThinkingCapability::Configurable,
            thinking_adaptor: ThinkingAdaptor::Auto,
            max_tokens: None,
        };

        let result = registry.register_from_model_config(&config, None);
        assert!(result.is_ok());
        assert!(registry.supports_model("llama-3.3-70b-versatile"));
        assert!(registry.supports_model("groq-llama"));
    }

    #[test]
    fn test_register_from_model_config_auth_error() {
        use forge_config::{
            AuthConfig, Capabilities, EndpointConfig, ModelConfig, ProtocolType, ThinkingAdaptor,
            ThinkingCapability, VendorType,
        };

        let mut registry = ProviderRegistry::new();
        let config = ModelConfig {
            id: "bad".to_string(),
            name: "Bad".to_string(),
            model_id: "bad-model".to_string(),
            protocol: ProtocolType::Anthropic,
            vendor: VendorType::Custom,
            endpoint: EndpointConfig::new("https://api.anthropic.com"),
            auth: AuthConfig::Bearer { token: "".to_string() },
            proxy_name: None,
            description: None,
            capabilities: Capabilities::default(),
            thinking: None,
            thinking_capability: ThinkingCapability::Configurable,
            thinking_adaptor: ThinkingAdaptor::Auto,
            max_tokens: None,
        };

        let result = registry.register_from_model_config(&config, None);
        assert!(result.is_err());
        assert!(!registry.supports_model("bad-model"));
    }
}
