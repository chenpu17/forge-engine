//! Model configuration types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Complete model configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    /// Unique identifier for this model configuration.
    pub id: String,
    /// Display name shown in UI.
    pub name: String,
    /// Actual model ID to send in API requests.
    pub model_id: String,
    /// API protocol format.
    pub protocol: ProtocolType,
    /// Service vendor.
    pub vendor: VendorType,
    /// API endpoint configuration.
    pub endpoint: EndpointConfig,
    /// Authentication configuration.
    pub auth: AuthConfig,
    /// Proxy name for LLM requests.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_name: Option<String>,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Model capabilities.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Thinking mode configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    /// Model's thinking capability.
    #[serde(default, skip_serializing_if = "is_default_thinking_capability")]
    pub thinking_capability: ThinkingCapability,
    /// Thinking protocol adaptor.
    #[serde(default, skip_serializing_if = "is_default_thinking_adaptor")]
    pub thinking_adaptor: ThinkingAdaptor,
    /// Maximum output tokens for this model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
}

/// API protocol type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolType {
    /// `OpenAI` Chat Completions API format.
    Openai,
    /// Anthropic Messages API format.
    Anthropic,
    /// Ollama API format.
    Ollama,
    /// Google Gemini API format.
    Gemini,
}

/// Service vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VendorType {
    /// Anthropic (Claude).
    Anthropic,
    /// `OpenAI` (GPT).
    Openai,
    /// Google (Gemini).
    Google,
    /// Zhipu AI (GLM).
    Zhipu,
    /// `DeepSeek`.
    Deepseek,
    /// Moonshot (Kimi).
    Moonshot,
    /// Ollama (local).
    Ollama,
    /// Custom/Other.
    Custom,
}

impl VendorType {
    /// Get UI icon for this vendor.
    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Anthropic => "🟠",
            Self::Openai => "🟢",
            Self::Google => "🔷",
            Self::Zhipu => "🔵",
            Self::Deepseek => "🟣",
            Self::Moonshot => "🌙",
            Self::Ollama => "🦙",
            Self::Custom => "🤖",
        }
    }
}

/// API endpoint configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointConfig {
    /// Base URL for the API.
    pub base_url: String,
}

impl EndpointConfig {
    /// Get the base URL.
    #[must_use]
    pub fn get_base_url(&self) -> &str {
        &self.base_url
    }

    /// Create a new endpoint with the given base URL.
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self { base_url: base_url.into() }
    }
}

/// Authentication configuration.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AuthConfig {
    /// Bearer token authentication.
    Bearer {
        /// The bearer token.
        token: String,
    },
    /// API key in custom header.
    ApiKey {
        /// Custom header name.
        header_name: String,
        /// The API key value.
        api_key: String,
    },
    /// Custom headers.
    Header {
        /// Map of header names to values.
        headers: HashMap<String, String>,
    },
    /// Multiple credentials for rotation / failover.
    Multi {
        /// Ordered list of credentials to rotate through.
        credentials: Vec<Self>,
    },
    /// No authentication.
    None,
}

impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bearer { .. } => f.debug_struct("Bearer").field("token", &"***").finish(),
            Self::ApiKey { header_name, .. } => f
                .debug_struct("ApiKey")
                .field("header_name", header_name)
                .field("api_key", &"***")
                .finish(),
            Self::Header { headers } => {
                let redacted: HashMap<&String, &str> = headers.keys().map(|k| (k, "***")).collect();
                f.debug_struct("Header").field("headers", &redacted).finish()
            }
            Self::Multi { credentials } => {
                f.debug_struct("Multi").field("credentials", credentials).finish()
            }
            Self::None => write!(f, "None"),
        }
    }
}

impl AuthConfig {
    /// Apply authentication to HTTP headers.
    pub fn apply_headers(&self, headers: &mut HashMap<String, String>) {
        match self {
            Self::Bearer { token } => {
                headers.insert("Authorization".to_string(), format!("Bearer {token}"));
            }
            Self::ApiKey { header_name, api_key } => {
                headers.insert(header_name.clone(), api_key.clone());
            }
            Self::Header { headers: custom_headers } => {
                headers.extend(custom_headers.clone());
            }
            Self::Multi { credentials } => {
                if let Some(first) = credentials.iter().find(|c| c.is_configured()) {
                    first.apply_headers(headers);
                }
            }
            Self::None => {}
        }
    }

    /// Check if authentication is configured.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        match self {
            Self::Bearer { token } => !token.trim().is_empty(),
            Self::ApiKey { api_key, .. } => !api_key.trim().is_empty(),
            Self::Header { headers } => !headers.is_empty(),
            Self::Multi { credentials } => credentials.iter().any(Self::is_configured),
            Self::None => false,
        }
    }

    /// Check if this auth type requires credentials.
    #[must_use]
    pub const fn requires_auth(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// Get the number of credentials in this config.
    #[must_use]
    pub const fn credential_count(&self) -> usize {
        match self {
            Self::Multi { credentials } => credentials.len(),
            Self::None => 0,
            _ => 1,
        }
    }

    /// Flatten into a list of individual (non-Multi) credentials.
    #[must_use]
    pub fn flatten(&self) -> Vec<&Self> {
        match self {
            Self::Multi { credentials } => credentials.iter().flat_map(Self::flatten).collect(),
            Self::None => vec![],
            other => vec![other],
        }
    }
}

/// Model capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Supports streaming responses.
    #[serde(default = "default_true")]
    pub streaming: bool,
    /// Supports tool/function calling.
    #[serde(default = "default_true")]
    pub tools: bool,
    /// Supports vision/image inputs.
    #[serde(default)]
    pub vision: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self { streaming: true, tools: true, vision: false }
    }
}

const fn default_true() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde skip_serializing_if requires &T
fn is_default_thinking_capability(cap: &ThinkingCapability) -> bool {
    *cap == ThinkingCapability::Configurable
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde skip_serializing_if requires &T
fn is_default_thinking_adaptor(adaptor: &ThinkingAdaptor) -> bool {
    *adaptor == ThinkingAdaptor::Auto
}

/// Thinking mode effort level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingEffort {
    /// Fast response, minimal reasoning.
    Low,
    /// Balanced mode (default).
    #[default]
    Medium,
    /// Deep reasoning, slowest but most accurate.
    High,
}

/// Model's thinking capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingCapability {
    /// Thinking can be toggled on/off.
    #[default]
    Configurable,
    /// Thinking is always on, cannot be disabled.
    AlwaysOn,
    /// Model does not support thinking mode.
    NotSupported,
}

/// Thinking protocol adaptor type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingAdaptor {
    /// Auto-detect based on model name and URL.
    #[default]
    Auto,
    /// `OpenAI` o1/o3 series reasoning format.
    OpenaiReasoning,
    /// GLM series thinking format.
    GlmThinking,
    /// DeepSeek/Qwen thinking format.
    DeepseekQwen,
    /// `MiniMax`: parses `<think>` tags in response.
    MiniMaxTags,
    /// Model does not support thinking mode.
    None,
}

/// Thinking mode configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    /// Whether thinking mode is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Token budget for thinking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<usize>,
    /// Reasoning effort level.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<ThinkingEffort>,
    /// Preserve thinking history across turns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preserve_history: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint() {
        let ep = EndpointConfig::new("https://api.openai.com");
        assert_eq!(ep.get_base_url(), "https://api.openai.com");
    }

    #[test]
    fn test_auth_bearer() {
        let auth = AuthConfig::Bearer { token: "test-token".to_string() };
        let mut headers = HashMap::new();
        auth.apply_headers(&mut headers);
        assert_eq!(headers.get("Authorization"), Some(&"Bearer test-token".to_string()));
    }

    #[test]
    fn test_auth_is_configured() {
        assert!(AuthConfig::Bearer { token: "t".to_string() }.is_configured());
        assert!(!AuthConfig::Bearer { token: "  ".to_string() }.is_configured());
        assert!(!AuthConfig::None.is_configured());
    }

    #[test]
    fn test_auth_debug_redacts_secrets() {
        let bearer = AuthConfig::Bearer { token: "sk-secret-key-12345".to_string() };
        let debug_str = format!("{bearer:?}");
        assert!(
            !debug_str.contains("sk-secret-key-12345"),
            "Debug output must not contain the actual token: {debug_str}"
        );
        assert!(debug_str.contains("***"), "Debug output should show redacted marker");

        let api_key = AuthConfig::ApiKey {
            header_name: "X-Api-Key".to_string(),
            api_key: "my-secret-api-key".to_string(),
        };
        let debug_str = format!("{api_key:?}");
        assert!(
            !debug_str.contains("my-secret-api-key"),
            "Debug output must not contain the actual api_key: {debug_str}"
        );
    }
}
