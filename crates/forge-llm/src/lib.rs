//! Forge LLM - LLM Provider Adapters
//!
//! This crate provides adapters for different LLM providers,
//! handling streaming, tool calls, and token management.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                   LlmProvider trait                 │
//! │   (unified interface for all providers)             │
//! └───────────────────────────┬─────────────────────────┘
//!                             │
//!         ┌───────────────────┼───────────────────┐
//!         │                   │                   │
//!         ▼                   ▼                   ▼
//! ┌───────────────┐   ┌───────────────┐   ┌───────────────┐
//! │   Anthropic   │   │    OpenAI     │   │   Provider    │
//! │   Provider    │   │   Provider    │   │   Registry    │
//! └───────────────┘   └───────────────┘   └───────────────┘
//! ```

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::module_name_repetitions,
    clippy::must_use_candidate
)]

pub mod anthropic;
pub mod auth;
pub mod base;
pub mod error;
pub mod factory;
pub mod gemini;
pub mod middleware;
pub mod ollama;
pub mod openai;
pub mod provider;
pub mod retry;
pub mod stream;
pub mod structured;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::pin::Pin;

// Re-export key types
pub use anthropic::AnthropicProvider;
pub use auth::AuthRotator;
pub use error::{LlmError, Result};
pub use factory::{FactoryError, ProviderFactory};
pub use gemini::GeminiProvider;
pub use middleware::{InstrumentedProvider, LlmMetrics, RetryNotification};
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
pub use provider::{ModelInfo, ProviderRegistry};
pub use retry::{
    DeduplicationConfig, DeduplicationStatus, RequestDeduplicator, RetryConfig, RetryHandler,
};
pub use stream::{SseEvent, SseProcessor, StopReason, ToolCall, ToolCallParser, Usage};
pub use structured::{
    build_anthropic_structured_tool, build_anthropic_tool_choice, build_openai_response_format,
    build_schema_instruction, validate_json_response, STRUCTURED_TOOL_NAME,
};

/// LLM configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Model identifier
    pub model: String,
    /// Maximum tokens to generate
    pub max_tokens: usize,
    /// Temperature (0.0 - 1.0)
    pub temperature: f64,
    /// System prompt (simple format - use `system_blocks` for multi-block)
    pub system_prompt: Option<String>,
    /// System prompt blocks (multi-block format with individual cache control)
    /// When set, this takes precedence over `system_prompt`
    #[serde(default)]
    pub system_blocks: Option<Vec<SystemBlock>>,
    /// Enable prompt caching (Anthropic only)
    #[serde(default)]
    pub enable_cache: bool,
    /// Thinking mode configuration
    #[serde(default)]
    pub thinking: Option<forge_config::ThinkingConfig>,
    /// Thinking protocol adaptor
    #[serde(default)]
    pub thinking_adaptor: forge_config::ThinkingAdaptor,
    /// Stream read timeout in seconds (default: 300)
    #[serde(default = "LlmConfig::default_stream_timeout_secs")]
    pub stream_timeout_secs: u64,
    /// Response schema for structured output (JSON Schema format).
    #[serde(default)]
    pub response_schema: Option<Value>,
}

impl LlmConfig {
    /// Default stream timeout in seconds (300s = 5 minutes)
    pub const fn default_stream_timeout_secs() -> u64 {
        300
    }
}

/// System prompt block with cache control
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemBlock {
    /// Block content text
    pub text: String,
    /// Block type (for future extensibility)
    #[serde(default = "SystemBlock::default_block_type")]
    pub block_type: SystemBlockType,
    /// Cache control setting
    #[serde(default)]
    pub cache_control: Option<CacheControl>,
}

/// System block type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SystemBlockType {
    /// Text block (default)
    #[default]
    Text,
}

/// Cache control settings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheControl {
    /// Ephemeral cache (5 minutes TTL for Anthropic)
    Ephemeral,
}

impl SystemBlock {
    /// Create a new text system block
    pub fn text(content: impl Into<String>) -> Self {
        Self { text: content.into(), block_type: SystemBlockType::Text, cache_control: None }
    }

    /// Create a cached text system block
    pub fn cached(content: impl Into<String>) -> Self {
        Self {
            text: content.into(),
            block_type: SystemBlockType::Text,
            cache_control: Some(CacheControl::Ephemeral),
        }
    }

    /// Set cache control
    #[must_use]
    pub const fn with_cache(mut self, cache: CacheControl) -> Self {
        self.cache_control = Some(cache);
        self
    }

    const fn default_block_type() -> SystemBlockType {
        SystemBlockType::Text
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 8192,
            temperature: 0.7,
            system_prompt: None,
            system_blocks: None,
            enable_cache: true,
            thinking: None,
            thinking_adaptor: forge_config::ThinkingAdaptor::Auto,
            stream_timeout_secs: Self::default_stream_timeout_secs(),
            response_schema: None,
        }
    }
}

/// Message for LLM conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Message role (user or assistant)
    pub role: ChatRole,
    /// Message content
    pub content: MessageContent,
}

/// Chat message role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    /// User message
    User,
    /// Assistant message
    Assistant,
}

/// Message content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content
    Text(String),
    /// Multiple content blocks
    Blocks(Vec<ContentBlock>),
}

/// Content block
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text block
    Text {
        /// Text content
        text: String,
    },
    /// Tool use block
    ToolUse {
        /// Tool call ID
        id: String,
        /// Tool name
        name: String,
        /// Tool input
        input: Value,
    },
    /// Tool result block
    ToolResult {
        /// Tool call ID this result is for
        tool_use_id: String,
        /// Result content
        content: String,
        /// Whether this is an error result
        is_error: bool,
    },
}

/// Tool definition for LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// Tool parameters JSON schema
    #[serde(rename = "input_schema")]
    pub parameters: Value,
}

/// LLM streaming events
#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// Text content delta
    TextDelta(String),
    /// Thinking content started (Extended Thinking)
    ThinkingStart,
    /// Thinking content delta (Extended Thinking)
    ThinkingDelta(String),
    /// Thinking content ended (Extended Thinking)
    ThinkingEnd,
    /// Tool use started
    ToolUseStart {
        /// Tool call ID
        id: String,
        /// Tool name
        name: String,
    },
    /// Tool use input delta
    ToolUseInputDelta {
        /// Tool call ID
        id: String,
        /// Input delta
        delta: String,
    },
    /// Tool use completed
    ToolUseEnd {
        /// Tool call ID
        id: String,
        /// Tool name
        name: String,
        /// Parsed input
        input: Value,
    },
    /// Message completed
    MessageEnd {
        /// Token usage
        usage: Usage,
    },
    /// Error occurred
    Error(String),
}

/// LLM event stream type
pub type LlmEventStream = Pin<Box<dyn Stream<Item = Result<LlmEvent>> + Send>>;

/// Core LLM provider trait
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Get provider unique identifier
    fn id(&self) -> &str;

    /// Get provider display name
    fn name(&self) -> &str;

    /// Get list of supported models
    fn supported_models(&self) -> Vec<ModelInfo>;

    /// Send a chat request and get a streaming response
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: Vec<ToolDef>,
        config: &LlmConfig,
    ) -> Result<LlmEventStream>;

    /// Get context limit for a model
    fn context_limit(&self, model: &str) -> usize {
        self.supported_models().iter().find(|m| m.id == model).map_or(200_000, |m| m.context_window)
    }

    /// Estimate token count for text
    fn estimate_tokens(&self, text: &str) -> usize {
        // Use character count (not byte length) for correct CJK/Unicode handling
        text.chars().count() / 4
    }
}
