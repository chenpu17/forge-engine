//! Provider Factory — config-driven `LlmProvider` instantiation
//!
//! Bridges `forge_config::ModelConfig` to runtime `LlmProvider` instances.

use std::sync::Arc;

use forge_config::{ModelConfig, ProtocolType};

use crate::{AnthropicProvider, LlmProvider, OllamaProvider, OpenAIProvider};

/// Factory for creating `LlmProvider` instances from `ModelConfig`.
pub struct ProviderFactory;

impl ProviderFactory {
    /// Validate auth configuration and extract the base URL.
    fn validate_and_extract(config: &ModelConfig) -> Result<String, FactoryError> {
        if config.protocol != ProtocolType::Ollama && !config.auth.is_configured() {
            return Err(FactoryError::AuthNotConfigured { model_id: config.id.clone() });
        }
        Ok(config.endpoint.get_base_url().to_string())
    }

    /// Create an `LlmProvider` from a `ModelConfig`.
    pub fn create(config: &ModelConfig) -> Result<Arc<dyn LlmProvider>, FactoryError> {
        let base_url = Self::validate_and_extract(config)?;

        match config.protocol {
            ProtocolType::Openai => {
                let provider = OpenAIProvider::new_with_auth(&config.auth).with_base_url(base_url);
                Ok(Arc::new(provider))
            }
            ProtocolType::Anthropic => {
                let provider =
                    AnthropicProvider::new_with_auth(&config.auth).with_base_url(base_url);
                Ok(Arc::new(provider))
            }
            ProtocolType::Ollama => {
                let provider = OllamaProvider::with_base_url(base_url);
                Ok(Arc::new(provider))
            }
            ProtocolType::Gemini => {
                let provider = crate::gemini::GeminiProvider::new_with_auth(&config.auth)
                    .with_base_url(base_url);
                Ok(Arc::new(provider))
            }
        }
    }

    /// Create a provider with proxy configuration applied.
    pub fn create_with_proxy(
        config: &ModelConfig,
        proxy: Option<&forge_config::ProxyConfig>,
    ) -> Result<Arc<dyn LlmProvider>, FactoryError> {
        let base_url = Self::validate_and_extract(config)?;

        match config.protocol {
            ProtocolType::Openai => {
                let provider = OpenAIProvider::new_with_auth(&config.auth)
                    .with_base_url(base_url)
                    .with_proxy_config(proxy);
                Ok(Arc::new(provider))
            }
            ProtocolType::Anthropic => {
                let provider = AnthropicProvider::new_with_auth(&config.auth)
                    .with_base_url(base_url)
                    .with_proxy_config(proxy);
                Ok(Arc::new(provider))
            }
            ProtocolType::Ollama => {
                let provider = OllamaProvider::with_base_url(base_url);
                Ok(Arc::new(provider))
            }
            ProtocolType::Gemini => {
                let provider = crate::gemini::GeminiProvider::new_with_auth(&config.auth)
                    .with_base_url(base_url)
                    .with_proxy_config(proxy);
                Ok(Arc::new(provider))
            }
        }
    }
}

/// Errors from provider factory operations.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    /// Authentication is required but not configured for this model.
    #[error("Authentication not configured for model '{model_id}'")]
    AuthNotConfigured {
        /// The model config ID that failed.
        model_id: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_config::{
        AuthConfig, Capabilities, EndpointConfig, ModelConfig, ProtocolType, ThinkingAdaptor,
        ThinkingCapability, VendorType,
    };

    fn make_config(protocol: ProtocolType, auth: AuthConfig, base_url: &str) -> ModelConfig {
        ModelConfig {
            id: "test".to_string(),
            name: "Test".to_string(),
            model_id: "test-model".to_string(),
            protocol,
            vendor: VendorType::Custom,
            endpoint: EndpointConfig::new(base_url),
            auth,
            proxy_name: None,
            description: None,
            capabilities: Capabilities::default(),
            thinking: None,
            thinking_capability: ThinkingCapability::Configurable,
            thinking_adaptor: ThinkingAdaptor::Auto,
            max_tokens: None,
        }
    }

    #[test]
    fn test_create_openai_provider() {
        let config = make_config(
            ProtocolType::Openai,
            AuthConfig::Bearer { token: "sk-test".to_string() },
            "https://api.openai.com",
        );
        let provider = ProviderFactory::create(&config).expect("should create");
        assert_eq!(provider.id(), "openai");
    }

    #[test]
    fn test_create_anthropic_provider() {
        let config = make_config(
            ProtocolType::Anthropic,
            AuthConfig::Bearer { token: "sk-ant-test".to_string() },
            "https://api.anthropic.com",
        );
        let provider = ProviderFactory::create(&config).expect("should create");
        assert_eq!(provider.id(), "anthropic");
    }

    #[test]
    fn test_create_ollama_provider() {
        let config = make_config(ProtocolType::Ollama, AuthConfig::None, "http://localhost:11434");
        let provider = ProviderFactory::create(&config).expect("should create");
        assert_eq!(provider.id(), "ollama");
    }

    #[test]
    fn test_create_gemini_provider() {
        let config = make_config(
            ProtocolType::Gemini,
            AuthConfig::Bearer { token: "test-key".to_string() },
            "https://generativelanguage.googleapis.com",
        );
        let provider = ProviderFactory::create(&config).expect("should create");
        assert_eq!(provider.id(), "gemini");
    }

    #[test]
    fn test_auth_not_configured_error() {
        let config = make_config(
            ProtocolType::Openai,
            AuthConfig::Bearer { token: "".to_string() },
            "https://api.openai.com",
        );
        let result = ProviderFactory::create(&config);
        assert!(matches!(result, Err(FactoryError::AuthNotConfigured { .. })));
    }

    #[test]
    fn test_ollama_no_auth_ok() {
        let config = make_config(ProtocolType::Ollama, AuthConfig::None, "http://localhost:11434");
        assert!(ProviderFactory::create(&config).is_ok());
    }

    #[test]
    fn test_auth_none_rejected_for_non_ollama() {
        for protocol in [ProtocolType::Openai, ProtocolType::Anthropic, ProtocolType::Gemini] {
            let config = make_config(protocol, AuthConfig::None, "https://example.com");
            let result = ProviderFactory::create(&config);
            assert!(
                matches!(result, Err(FactoryError::AuthNotConfigured { .. })),
                "AuthConfig::None should be rejected for {protocol:?}"
            );
        }
    }

    #[test]
    fn test_create_with_proxy() {
        let config = make_config(
            ProtocolType::Openai,
            AuthConfig::Bearer { token: "sk-test".to_string() },
            "https://api.openai.com",
        );
        let proxy = forge_config::ProxyConfig::none();
        let provider =
            ProviderFactory::create_with_proxy(&config, Some(&proxy)).expect("should create");
        assert_eq!(provider.id(), "openai");
    }
}
