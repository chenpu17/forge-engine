//! Ollama API adapter
//!
//! Implements the `LlmProvider` trait for Ollama local models.
//! Ollama provides an `OpenAI`-compatible API, so this is largely a wrapper
//! around the `OpenAI` provider with Ollama-specific defaults and model discovery.

use crate::error::LlmError;
use crate::{
    provider::ModelInfo, ChatMessage, LlmConfig, LlmEventStream, LlmProvider, OpenAIProvider,
    Result, ToolDef,
};
use async_trait::async_trait;
use serde::Deserialize;

/// Default Ollama API endpoint
const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Ollama API client
pub struct OllamaProvider {
    /// Underlying OpenAI-compatible provider
    inner: OpenAIProvider,
    /// Ollama base URL
    base_url: String,
    /// Cached model list
    cached_models: Vec<ModelInfo>,
}

impl OllamaProvider {
    /// Create a new Ollama provider with default settings
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_OLLAMA_URL)
    }

    /// Create a new Ollama provider with custom base URL
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        // Ollama doesn't require an API key, but OpenAI provider expects one
        let inner = OpenAIProvider::new("ollama").with_base_url(format!("{base_url}/v1"));

        Self { inner, base_url, cached_models: Vec::new() }
    }

    /// Check if Ollama is running
    pub async fn is_available(&self) -> bool {
        // Use no_proxy() to avoid macOS system proxy detection issues
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let url = format!("{}/api/tags", self.base_url);

        client.get(&url).send().await.is_ok_and(|r| r.status().is_success())
    }

    /// Fetch available models from Ollama
    pub async fn fetch_models(&mut self) -> Result<Vec<ModelInfo>> {
        // Use no_proxy() to avoid macOS system proxy detection issues
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let url = format!("{}/api/tags", self.base_url);

        let response = client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(LlmError::ApiError {
                status: response.status().as_u16(),
                message: "Failed to fetch Ollama models".to_string(),
            });
        }

        let tags: OllamaTagsResponse = response.json().await?;

        let models: Vec<ModelInfo> = tags
            .models
            .into_iter()
            .map(|m| {
                let context_window = estimate_context_window(&m.name);
                ModelInfo::new(&m.name, &m.name, context_window)
            })
            .collect();

        self.cached_models.clone_from(&models);
        Ok(models)
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn id(&self) -> &str {
        "ollama"
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "Ollama"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        if self.cached_models.is_empty() {
            vec![
                ModelInfo::new("llama3.2", "Llama 3.2", 128_000),
                ModelInfo::new("llama3.1", "Llama 3.1", 128_000),
                ModelInfo::new("codellama", "Code Llama", 16_000),
                ModelInfo::new("mistral", "Mistral", 32_000),
                ModelInfo::new("mixtral", "Mixtral", 32_000),
                ModelInfo::new("qwen2.5-coder", "Qwen 2.5 Coder", 32_000),
                ModelInfo::new("deepseek-coder-v2", "DeepSeek Coder V2", 128_000),
            ]
        } else {
            self.cached_models.clone()
        }
    }

    #[tracing::instrument(name = "llm_call", skip_all, fields(provider = "ollama", model = %config.model))]
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: Vec<ToolDef>,
        config: &LlmConfig,
    ) -> Result<LlmEventStream> {
        self.inner.chat_stream(messages, tools, config).await
    }

    fn context_limit(&self, model: &str) -> usize {
        estimate_context_window(model)
    }

    fn estimate_tokens(&self, text: &str) -> usize {
        text.chars().count() / 4
    }
}

/// Response from Ollama /api/tags endpoint
#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

/// Model info from Ollama
#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: String,
    #[allow(dead_code)]
    modified_at: Option<String>,
    #[allow(dead_code)]
    size: Option<u64>,
}

/// Estimate context window based on model name
fn estimate_context_window(model: &str) -> usize {
    let model_lower = model.to_lowercase();

    if model_lower.contains("llama3")
        || model_lower.contains("llama-3")
        || model_lower.contains("deepseek")
    {
        128_000
    } else if model_lower.contains("qwen")
        || model_lower.contains("mixtral")
        || model_lower.contains("mistral")
    {
        32_000
    } else if model_lower.contains("codellama") {
        16_000
    } else if model_lower.contains("phi") {
        4_096
    } else {
        8_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ollama_provider_default() {
        let provider = OllamaProvider::new();
        assert_eq!(provider.id(), "ollama");
        assert_eq!(provider.name(), "Ollama");
        assert_eq!(provider.base_url(), DEFAULT_OLLAMA_URL);
    }

    #[test]
    fn test_ollama_provider_custom_url() {
        let provider = OllamaProvider::with_base_url("http://192.168.1.100:11434");
        assert_eq!(provider.base_url(), "http://192.168.1.100:11434");
    }

    #[test]
    fn test_supported_models_default() {
        let provider = OllamaProvider::new();
        let models = provider.supported_models();
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.id.contains("llama")));
    }

    #[test]
    fn test_estimate_context_window() {
        assert_eq!(estimate_context_window("llama3.2"), 128_000);
        assert_eq!(estimate_context_window("llama-3.1-70b"), 128_000);
        assert_eq!(estimate_context_window("codellama:7b"), 16_000);
        assert_eq!(estimate_context_window("mistral:latest"), 32_000);
        assert_eq!(estimate_context_window("unknown-model"), 8_000);
    }

    #[test]
    fn test_context_limit() {
        let provider = OllamaProvider::new();
        assert_eq!(provider.context_limit("llama3.2"), 128_000);
        assert_eq!(provider.context_limit("codellama"), 16_000);
    }

    #[test]
    fn test_estimate_tokens() {
        let provider = OllamaProvider::new();
        assert_eq!(provider.estimate_tokens("hello world"), 2);
        assert_eq!(provider.estimate_tokens(&"a".repeat(100)), 25);
    }
}
