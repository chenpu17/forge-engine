//! Anthropic API adapter
//!
//! Implements the `LlmProvider` trait for Anthropic's Claude models,
//! with support for streaming responses and tool use.

use crate::error::LlmError;
use crate::{
    provider::ModelInfo, CacheControl, ChatMessage, ChatRole, ContentBlock, LlmConfig, LlmEvent,
    LlmEventStream, LlmProvider, MessageContent, Result, SseProcessor, SystemBlock, ToolCallParser,
    ToolDef, Usage,
};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Anthropic API version
const API_VERSION: &str = "2023-06-01";

/// Anthropic API client
pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.anthropic.com".to_string(),
            client: crate::base::create_http_client(false),
        }
    }

    /// Create a new Anthropic provider from an `AuthConfig`
    ///
    /// Extracts the API key/token from the auth config.
    /// For official API, uses `x-api-key` header; for proxies, uses Bearer token.
    pub fn new_with_auth(auth: &forge_config::AuthConfig) -> Self {
        Self::new(crate::base::extract_auth_token(auth))
    }

    /// Set custom base URL
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        // Use HTTP/1.1 for proxy servers (they may not support HTTP/2)
        if !self.is_official_api() {
            self.client = crate::base::create_http_client(true);
        }
        self
    }

    /// Configure proxy settings for this provider
    #[must_use]
    pub fn with_proxy_config(mut self, proxy: Option<&forge_config::ProxyConfig>) -> Self {
        let use_http1_only = !self.is_official_api();
        self.client = crate::base::create_http_client_with_proxy(use_http1_only, proxy);
        self
    }

    /// Check if using official Anthropic API
    fn is_official_api(&self) -> bool {
        self.base_url.contains("api.anthropic.com")
    }

    /// Build headers for API request
    fn build_headers(&self, enable_cache: bool, thinking_enabled: bool) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        // Use Bearer token for proxy servers, x-api-key for official API
        if self.is_official_api() {
            headers.insert(
                "x-api-key",
                HeaderValue::from_str(&self.api_key)
                    .unwrap_or_else(|_| HeaderValue::from_static("")),
            );
            headers.insert("anthropic-version", HeaderValue::from_static(API_VERSION));
            // Add beta headers for features
            let mut beta_features = Vec::new();
            if enable_cache {
                beta_features.push("prompt-caching-2024-07-31");
            }
            if thinking_enabled {
                beta_features.push("interleaved-thinking-2025-05-14");
            }
            if !beta_features.is_empty() {
                if let Ok(value) = HeaderValue::from_str(&beta_features.join(",")) {
                    headers.insert("anthropic-beta", value);
                }
            }
        } else {
            // Proxy servers use Bearer token and may not need anthropic-specific headers
            let bearer = format!("Bearer {}", self.api_key);
            if let Ok(value) = HeaderValue::from_str(&bearer) {
                headers.insert(reqwest::header::AUTHORIZATION, value);
            }
        }

        headers
    }

    /// Convert `ChatMessage` to Anthropic API format
    #[allow(clippy::unused_self)]
    fn convert_messages(&self, messages: &[ChatMessage]) -> Vec<Value> {
        messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                };

                let content = match &msg.content {
                    MessageContent::Text(text) => json!(text),
                    MessageContent::Blocks(blocks) => {
                        let converted: Vec<Value> = blocks
                            .iter()
                            .map(|block| match block {
                                ContentBlock::Text { text } => {
                                    json!({"type": "text", "text": text})
                                }
                                ContentBlock::ToolUse { id, name, input } => {
                                    json!({"type": "tool_use", "id": id, "name": name, "input": input})
                                }
                                ContentBlock::ToolResult {
                                    tool_use_id,
                                    content,
                                    is_error,
                                } => {
                                    json!({
                                        "type": "tool_result",
                                        "tool_use_id": tool_use_id,
                                        "content": content,
                                        "is_error": is_error
                                    })
                                }
                            })
                            .collect();
                        json!(converted)
                    }
                };

                json!({
                    "role": role,
                    "content": content
                })
            })
            .collect()
    }

    /// Convert `ToolDef` to Anthropic API format
    #[allow(clippy::unused_self)]
    fn convert_tools(&self, tools: &[ToolDef], enable_cache: bool) -> Vec<Value> {
        let len = tools.len();
        tools
            .iter()
            .enumerate()
            .map(|(i, tool)| {
                let mut tool_json = json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.parameters
                });
                // Add cache_control to the last tool
                if enable_cache && i == len - 1 {
                    tool_json["cache_control"] = json!({"type": "ephemeral"});
                }
                tool_json
            })
            .collect()
    }

    /// Convert system blocks to Anthropic API format
    #[allow(clippy::unused_self)]
    fn convert_system_blocks(&self, blocks: &[SystemBlock]) -> Vec<Value> {
        blocks
            .iter()
            .map(|block| {
                let mut block_json = json!({
                    "type": "text",
                    "text": block.text
                });
                // Add cache_control if specified
                if let Some(cache) = &block.cache_control {
                    let cache_type = match cache {
                        CacheControl::Ephemeral => "ephemeral",
                    };
                    block_json["cache_control"] = json!({"type": cache_type});
                }
                block_json
            })
            .collect()
    }

    /// Build system prompt value for API request
    fn build_system_value(&self, config: &LlmConfig) -> Option<Value> {
        // System blocks take precedence over simple system_prompt
        if let Some(ref blocks) = config.system_blocks {
            if !blocks.is_empty() {
                return Some(json!(self.convert_system_blocks(blocks)));
            }
        }

        // Fall back to simple system_prompt
        if let Some(ref system) = config.system_prompt {
            if config.enable_cache {
                // Use array format with cache_control for caching
                return Some(json!([
                    {
                        "type": "text",
                        "text": system,
                        "cache_control": {"type": "ephemeral"}
                    }
                ]));
            }
            return Some(json!(system));
        }

        None
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn id(&self) -> &str {
        "anthropic"
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "Anthropic"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            // Claude 4.5 models
            ModelInfo::new("claude-sonnet-4-5-20250929", "Claude Sonnet 4.5", 200_000)
                .with_vision(true),
            // Claude 3.5 models
            ModelInfo::new("claude-3-5-sonnet-20241022", "Claude 3.5 Sonnet", 200_000)
                .with_vision(true),
            ModelInfo::new("claude-3-5-haiku-20241022", "Claude 3.5 Haiku", 200_000)
                .with_vision(true),
            // Claude 3 models
            ModelInfo::new("claude-3-opus-20240229", "Claude 3 Opus", 200_000).with_vision(true),
            ModelInfo::new("claude-3-sonnet-20240229", "Claude 3 Sonnet", 200_000)
                .with_vision(true),
            ModelInfo::new("claude-3-haiku-20240307", "Claude 3 Haiku", 200_000).with_vision(true),
        ]
    }

    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    #[tracing::instrument(name = "llm_call", skip_all, fields(provider = "anthropic", model = %config.model))]
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: Vec<ToolDef>,
        config: &LlmConfig,
    ) -> Result<LlmEventStream> {
        // Build URL - handle different API formats
        // If base_url already contains /v1 or ends with /messages, don't append
        let url = if self.base_url.contains("/v1") || self.base_url.ends_with("/messages") {
            if self.base_url.ends_with("/messages") {
                self.base_url.clone()
            } else {
                format!("{}/messages", self.base_url)
            }
        } else {
            format!("{}/v1/messages", self.base_url)
        };
        let enable_cache = config.enable_cache;
        let thinking_enabled = config.thinking.as_ref().is_some_and(|t| t.enabled);

        // Build request body
        let mut body = json!({
            "model": config.model,
            "max_tokens": config.max_tokens,
            "temperature": config.temperature,
            "messages": self.convert_messages(messages),
            "stream": true
        });

        // Add thinking if enabled
        if let Some(ref thinking_config) = config.thinking {
            if thinking_config.enabled {
                let budget = thinking_config.budget_tokens.unwrap_or(10000);
                body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": budget
                });
                tracing::debug!(budget_tokens = budget, "Thinking mode enabled");
            }
        }

        // Add system prompt if provided (supports multi-block format)
        if let Some(system_value) = self.build_system_value(config) {
            body["system"] = system_value;
        }

        // Add structured output via tool-use-as-structured-output pattern
        if let Some(ref schema) = config.response_schema {
            let mut all_tools = self.convert_tools(&tools, enable_cache);
            all_tools.push(crate::structured::build_anthropic_structured_tool(schema));
            body["tools"] = json!(all_tools);
            body["tool_choice"] = crate::structured::build_anthropic_tool_choice();
            tracing::debug!("Structured output enabled via tool-use pattern");
        } else if !tools.is_empty() {
            // Add tools if provided (normal path)
            body["tools"] = json!(self.convert_tools(&tools, enable_cache));
        }

        tracing::debug!(
            model = %config.model,
            messages = messages.len(),
            tools = tools.len(),
            cache_enabled = enable_cache,
            thinking_enabled = thinking_enabled,
            url = %url,
            is_official = self.is_official_api(),
            "Sending request to Anthropic API"
        );
        tracing::debug!(request_body = %serde_json::to_string_pretty(&body).unwrap_or_default(), "Request body");

        // Send request
        let response = self
            .client
            .post(&url)
            .headers(self.build_headers(enable_cache, thinking_enabled))
            .json(&body)
            .send()
            .await?;

        // Check for errors
        let status = response.status();
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();

            // Try to parse error response
            if let Ok(error_json) = serde_json::from_str::<Value>(&error_text) {
                let error_type = error_json["error"]["type"].as_str().unwrap_or("unknown");
                let message =
                    error_json["error"]["message"].as_str().unwrap_or(&error_text).to_string();

                return Err(match error_type {
                    "rate_limit_error" => LlmError::RateLimited { retry_after_secs: 60 },
                    "overloaded_error" => LlmError::ProviderUnavailable(message),
                    "authentication_error" => LlmError::AuthenticationFailed(message),
                    "invalid_request_error" => LlmError::ConfigError(message),
                    _ => LlmError::ApiError { status: status.as_u16(), message },
                });
            }

            return Err(LlmError::ApiError { status: status.as_u16(), message: error_text });
        }

        // Create SSE stream
        let byte_stream = response.bytes_stream();
        let stream_timeout_secs = config.stream_timeout_secs;

        let stream = futures::stream::unfold(
            (byte_stream, SseProcessor::new(), None::<ToolCallParser>, false),
            move |(mut byte_stream, mut sse, mut tool_parser, mut thinking_active)| async move {
                // Timeout for waiting for next data chunk in stream.
                // This is reset every time data is received, so it only triggers if LLM stops sending data.
                // Use configurable timeout from LlmConfig (default: 300s)
                let stream_read_timeout = std::time::Duration::from_secs(stream_timeout_secs);
                loop {
                    match tokio::time::timeout(stream_read_timeout, byte_stream.next()).await {
                        Ok(Some(Ok(chunk))) => {
                            let chunk_str = String::from_utf8_lossy(&chunk);
                            let events = sse.process_chunk(&chunk_str);

                            for event in events {
                                match parse_sse_event(
                                    &event.data,
                                    &mut tool_parser,
                                    &mut thinking_active,
                                ) {
                                    Ok(Some(llm_event)) => {
                                        return Some((
                                            Ok(llm_event),
                                            (byte_stream, sse, tool_parser, thinking_active),
                                        ));
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        return Some((
                                            Err(e),
                                            (byte_stream, sse, tool_parser, thinking_active),
                                        ));
                                    }
                                }
                            }
                        }
                        Ok(Some(Err(e))) => {
                            return Some((
                                Err(LlmError::NetworkError(e.to_string())),
                                (byte_stream, sse, tool_parser, thinking_active),
                            ));
                        }
                        Ok(None) => return None,
                        Err(_) => {
                            // Timeout - no data received within configured timeout
                            tracing::warn!(
                                "Stream read timeout after {stream_timeout_secs} seconds"
                            );
                            return Some((
                                Err(LlmError::StreamInterrupted(format!(
                                    "Stream read timeout after {stream_timeout_secs} seconds"
                                ))),
                                (byte_stream, sse, tool_parser, thinking_active),
                            ));
                        }
                    }
                }
            },
        );

        Ok(Box::pin(stream))
    }

    fn estimate_tokens(&self, text: &str) -> usize {
        // Claude uses roughly 3.5 characters per token
        let chars = text.chars().count();
        (chars * 10).div_ceil(35)
    }
}

/// Anthropic SSE event types
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum AnthropicEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: MessageInfo },
    #[serde(rename = "content_block_start")]
    ContentBlockStart { index: usize, content_block: ContentBlockInfo },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: DeltaInfo },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },
    #[serde(rename = "message_delta")]
    MessageDelta { delta: MessageDeltaInfo, usage: Option<UsageInfo> },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: ErrorInfo },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MessageInfo {
    id: String,
    model: String,
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum ContentBlockInfo {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::enum_variant_names)]
enum DeltaInfo {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct MessageDeltaInfo {
    stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[allow(clippy::struct_field_names)]
struct UsageInfo {
    input_tokens: Option<usize>,
    output_tokens: Option<usize>,
    #[serde(default)]
    cache_read_input_tokens: Option<usize>,
    #[serde(default)]
    cache_creation_input_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ErrorInfo {
    #[serde(rename = "type")]
    error_type: String,
    message: String,
}

/// Parse an SSE event and convert to `LlmEvent`
fn parse_sse_event(
    data: &str,
    tool_parser: &mut Option<ToolCallParser>,
    thinking_active: &mut bool,
) -> Result<Option<LlmEvent>> {
    // Skip [DONE] or empty data
    if data.trim().is_empty() || data.trim() == "[DONE]" {
        return Ok(None);
    }

    let event: AnthropicEvent = serde_json::from_str(data)?;

    match event {
        AnthropicEvent::MessageStart { .. }
        | AnthropicEvent::MessageStop
        | AnthropicEvent::Ping => Ok(None),
        AnthropicEvent::ContentBlockStart { content_block, .. } => match content_block {
            ContentBlockInfo::Text { .. } => Ok(None),
            ContentBlockInfo::ToolUse { id, name } => {
                *tool_parser = Some(ToolCallParser::new(id.clone(), name.clone()));
                Ok(Some(LlmEvent::ToolUseStart { id, name }))
            }
            ContentBlockInfo::Thinking { .. } => {
                *thinking_active = true;
                Ok(Some(LlmEvent::ThinkingStart))
            }
        },
        AnthropicEvent::ContentBlockDelta { delta, .. } => match delta {
            DeltaInfo::TextDelta { text } => Ok(Some(LlmEvent::TextDelta(text))),
            DeltaInfo::InputJsonDelta { partial_json } => {
                tool_parser.as_mut().map_or(Ok(None), |parser| {
                    let id = parser.id().to_string();
                    parser.append(&partial_json);
                    Ok(Some(LlmEvent::ToolUseInputDelta { id, delta: partial_json }))
                })
            }
            DeltaInfo::ThinkingDelta { thinking } => Ok(Some(LlmEvent::ThinkingDelta(thinking))),
        },
        AnthropicEvent::ContentBlockStop { .. } => {
            // Check if we were in a thinking block
            if *thinking_active {
                *thinking_active = false;
                // Discard any stale tool parser state from a malformed stream
                let _ = tool_parser.take();
                return Ok(Some(LlmEvent::ThinkingEnd));
            }
            // Check if we were in a tool use block
            tool_parser.take().map_or(Ok(None), |parser| {
                let id = parser.id().to_string();
                match parser.finish() {
                    Ok(tool_call) => Ok(Some(LlmEvent::ToolUseEnd {
                        id,
                        name: tool_call.name,
                        input: tool_call.input,
                    })),
                    Err(e) => Ok(Some(LlmEvent::Error(e.to_string()))),
                }
            })
        }
        AnthropicEvent::MessageDelta { usage, .. } => {
            let usage_data = usage.map_or_else(
                || Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                },
                |u| Usage {
                    input_tokens: u.input_tokens.unwrap_or(0),
                    output_tokens: u.output_tokens.unwrap_or(0),
                    cache_read_input_tokens: u.cache_read_input_tokens,
                    cache_creation_input_tokens: u.cache_creation_input_tokens,
                },
            );
            Ok(Some(LlmEvent::MessageEnd { usage: usage_data }))
        }
        AnthropicEvent::Error { error } => {
            Ok(Some(LlmEvent::Error(format!("{}: {}", error.error_type, error.message))))
        }
    }
}
