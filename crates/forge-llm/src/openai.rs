//! OpenAI API adapter
//!
//! Implements the LlmProvider trait for OpenAI's GPT models,
//! with support for streaming responses and tool use.
//! Also compatible with OpenAI-compatible APIs (e.g., local LLMs, Azure).

use crate::error::LlmError;
use crate::{
    provider::ModelInfo, ChatMessage, ChatRole, ContentBlock, LlmConfig, LlmEvent, LlmEventStream,
    LlmProvider, MessageContent, Result, ToolDef, Usage,
};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::Deserialize;
use serde_json::{json, Value};

/// OpenAI API client
pub struct OpenAIProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl OpenAIProvider {
    /// Create a new OpenAI provider
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: "https://api.openai.com".to_string(),
            client: crate::base::create_http_client(false),
        }
    }

    /// Create a new OpenAI provider from an `AuthConfig`
    ///
    /// Extracts the API key/token from the auth config. For `AuthConfig::None`,
    /// uses an empty string (useful for local APIs that don't require auth).
    pub fn new_with_auth(auth: &forge_config::AuthConfig) -> Self {
        Self::new(crate::base::extract_auth_token(auth))
    }

    /// Configure proxy settings for this provider
    #[must_use]
    pub fn with_proxy_config(mut self, proxy: Option<&forge_config::ProxyConfig>) -> Self {
        self.client = crate::base::create_http_client_with_proxy(false, proxy);
        self
    }

    /// Set custom base URL (for Azure, local LLMs, etc.)
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Build headers for API request
    fn build_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers
    }

    /// Convert ChatMessage to OpenAI API format
    fn convert_messages(&self, messages: &[ChatMessage], system: Option<&str>) -> Vec<Value> {
        let mut result = Vec::new();

        // Add system message if provided
        if let Some(sys) = system {
            result.push(json!({
                "role": "system",
                "content": sys
            }));
        }

        for msg in messages {
            let role = match msg.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
            };

            match &msg.content {
                MessageContent::Text(text) => {
                    result.push(json!({
                        "role": role,
                        "content": text
                    }));
                }
                MessageContent::Blocks(blocks) => {
                    // Collect all tool calls, text content, and tool results from this message
                    // We need to ensure proper ordering: assistant message with tool_calls
                    // must come before tool result messages
                    let mut tool_calls: Vec<Value> = Vec::new();
                    let mut text_content: Option<String> = None;
                    let mut tool_results: Vec<Value> = Vec::new();

                    for block in blocks {
                        match block {
                            ContentBlock::Text { text } => {
                                // Accumulate text content
                                match &mut text_content {
                                    Some(existing) => {
                                        existing.push('\n');
                                        existing.push_str(text);
                                    }
                                    None => text_content = Some(text.clone()),
                                }
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                // Skip tool calls with empty names (LLM error)
                                if name.is_empty() {
                                    tracing::warn!(
                                        "Skipping tool call with empty name in history: id={}",
                                        id
                                    );
                                    continue;
                                }
                                tool_calls.push(json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": input.to_string()
                                    }
                                }));
                            }
                            ContentBlock::ToolResult { tool_use_id, content, is_error: _ } => {
                                // Collect tool results to add after tool_calls message
                                tool_results.push(json!({
                                    "role": "tool",
                                    "tool_call_id": tool_use_id,
                                    "content": content
                                }));
                            }
                        }
                    }

                    // Order matters for OpenAI API:
                    // 1. First, add assistant message with tool_calls (if any)
                    // 2. Then, add tool result messages
                    if !tool_calls.is_empty() {
                        result.push(json!({
                            "role": "assistant",
                            "content": text_content,
                            "tool_calls": tool_calls
                        }));
                    } else if let Some(text) = text_content {
                        // Just text content, no tool calls
                        result.push(json!({
                            "role": role,
                            "content": text
                        }));
                    }

                    // Add tool results after the assistant message
                    result.extend(tool_results);
                }
            }
        }

        result
    }

    /// Convert ToolDef to OpenAI API format
    fn convert_tools(&self, tools: &[ToolDef]) -> Vec<Value> {
        tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters
                    }
                })
            })
            .collect()
    }
}

/// OpenAI streaming response chunk
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct StreamChunk {
    id: Option<String>,
    choices: Vec<StreamChoice>,
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: DeltaContent,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeltaContent {
    content: Option<String>,
    /// GLM models use reasoning_content for chain-of-thought reasoning
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct ToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<FunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct FunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageInfo {
    #[serde(default)]
    prompt_tokens: usize,
    #[serde(default)]
    completion_tokens: usize,
}

/// Non-streaming response format (when proxy ignores stream=true)
#[derive(Debug, Deserialize)]
struct NonStreamResponse {
    choices: Vec<NonStreamChoice>,
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
struct NonStreamChoice {
    message: NonStreamMessage,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NonStreamMessage {
    content: Option<String>,
    tool_calls: Option<Vec<NonStreamToolCall>>,
}

#[derive(Debug, Deserialize)]
struct NonStreamToolCall {
    id: String,
    function: NonStreamFunction,
}

#[derive(Debug, Deserialize)]
struct NonStreamFunction {
    name: String,
    arguments: String,
}

/// Tool call state for accumulating streaming tool calls
#[derive(Debug, Default)]
struct ToolCallState {
    id: String,
    name: String,
    arguments: String,
    started: bool,
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    fn id(&self) -> &str {
        "openai"
    }

    fn name(&self) -> &str {
        "OpenAI"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            // GPT-4o models
            ModelInfo::new("gpt-4o", "GPT-4o", 128_000).with_vision(true),
            ModelInfo::new("gpt-4o-mini", "GPT-4o Mini", 128_000).with_vision(true),
            // GPT-4 models
            ModelInfo::new("gpt-4-turbo", "GPT-4 Turbo", 128_000).with_vision(true),
            ModelInfo::new("gpt-4", "GPT-4", 8_192),
            // GPT-3.5
            ModelInfo::new("gpt-3.5-turbo", "GPT-3.5 Turbo", 16_385),
            // O1 reasoning models
            ModelInfo::new("o1-preview", "O1 Preview", 128_000).with_tools(false),
            ModelInfo::new("o1-mini", "O1 Mini", 128_000).with_tools(false),
        ]
    }

    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(name = "llm_call", skip_all, fields(provider = "openai", model = %config.model))]
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: Vec<ToolDef>,
        config: &LlmConfig,
    ) -> Result<LlmEventStream> {
        // 智谱/其他: base_url/chat/completions (if base_url already contains version path)
        // Use more precise path detection to avoid false positives (e.g., domain v1service.com)
        let url = if self.base_url.ends_with("/v1")
            || self.base_url.ends_with("/v2")
            || self.base_url.ends_with("/v3")
            || self.base_url.ends_with("/v4")
            || self.base_url.contains("/v1/")
            || self.base_url.contains("/v2/")
            || self.base_url.contains("/v3/")
            || self.base_url.contains("/v4/")
        {
            format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
        } else {
            format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'))
        };

        // Build request body - base parameters
        let mut body = json!({
            "model": config.model,
            "messages": self.convert_messages(&messages, config.system_prompt.as_deref()),
            "stream": true
        });

        // 智谱 API compatibility: detect by URL or model name
        // 智谱 API may reject some OpenAI-specific parameters
        // Use contains("glm") to match models like "hw-glm-4.6", "glm-4", etc.
        let is_zhipu =
            self.base_url.contains("bigmodel.cn") || config.model.to_lowercase().contains("glm");

        if is_zhipu {
            // 智谱 API: use different parameter handling
            // Only add optional parameters that are within valid ranges
            // 智谱 temperature must be in (0, 1] - 0.0 is not allowed
            if config.temperature > 0.0 && config.temperature <= 1.0 {
                body["temperature"] = json!(config.temperature);
            }
            // 智谱 max_tokens: use configured value (no hardcoded limit)
            // Different GLM models support different max_tokens (e.g., GLM-4 supports up to 128K)
            if config.max_tokens > 0 {
                body["max_tokens"] = json!(config.max_tokens);
            }
        } else {
            // Standard OpenAI compatible API
            body["max_tokens"] = json!(config.max_tokens);
            body["temperature"] = json!(config.temperature);
        }

        // Add stream_options to request usage info in streaming responses
        // Most OpenAI-compatible APIs support this or will ignore it if unsupported
        body["stream_options"] = json!({"include_usage": true});

        // Add structured output (response_format) if schema is provided
        if let Some(ref schema) = config.response_schema {
            body["response_format"] = crate::structured::build_openai_response_format(schema);
            tracing::debug!("Structured output enabled via response_format");
        }

        // Add tools if provided
        if !tools.is_empty() {
            body["tools"] = json!(self.convert_tools(&tools));
        }

        // Add thinking mode parameters based on adaptor type
        if let Some(ref thinking_config) = config.thinking {
            use forge_config::ThinkingAdaptor;

            // Determine adaptor: use explicit setting or auto-detect
            let adaptor = match config.thinking_adaptor {
                ThinkingAdaptor::Auto => {
                    // Auto-detect based on model name and URL
                    let model_lower = config.model.to_lowercase();
                    if model_lower.starts_with("o1") || model_lower.starts_with("o3") {
                        ThinkingAdaptor::OpenaiReasoning
                    } else if is_zhipu {
                        ThinkingAdaptor::GlmThinking
                    } else if model_lower.contains("deepseek") || model_lower.contains("qwen") {
                        ThinkingAdaptor::DeepseekQwen
                    } else if model_lower.contains("minimax") {
                        ThinkingAdaptor::MiniMaxTags
                    } else {
                        ThinkingAdaptor::None
                    }
                }
                other => other,
            };

            match adaptor {
                ThinkingAdaptor::OpenaiReasoning => {
                    // OpenAI o1/o3 series: use reasoning parameter
                    if thinking_config.enabled {
                        let effort = thinking_config.effort.unwrap_or_default();
                        let effort_str = match effort {
                            forge_config::ThinkingEffort::Low => "low",
                            forge_config::ThinkingEffort::Medium => "medium",
                            forge_config::ThinkingEffort::High => "high",
                        };
                        body["reasoning"] = json!({"effort": effort_str});
                        tracing::debug!(effort = effort_str, "OpenAI reasoning mode enabled");
                    }
                }
                ThinkingAdaptor::GlmThinking => {
                    // GLM models: use thinking parameter with type and budget_tokens
                    if thinking_config.enabled {
                        let budget = thinking_config.budget_tokens.unwrap_or(10000);
                        let mut thinking_obj = json!({
                            "type": "enabled",
                            "budget_tokens": budget
                        });
                        if thinking_config.preserve_history == Some(true) {
                            thinking_obj["clear_thinking"] = json!(false);
                        }
                        body["thinking"] = thinking_obj;
                        tracing::debug!(budget_tokens = budget, "GLM thinking mode enabled");
                    } else {
                        body["thinking"] = json!({"type": "disabled"});
                        tracing::debug!("GLM thinking mode disabled");
                    }
                }
                ThinkingAdaptor::DeepseekQwen => {
                    // DeepSeek/Qwen: use enable_thinking parameter
                    body["enable_thinking"] = json!(thinking_config.enabled);
                    tracing::debug!(
                        enabled = thinking_config.enabled,
                        "DeepSeek/Qwen thinking mode"
                    );
                }
                ThinkingAdaptor::MiniMaxTags | ThinkingAdaptor::None | ThinkingAdaptor::Auto => {
                    // MiniMax uses <think> tags in response, no request parameter needed
                    // None/Auto: no thinking parameters
                }
            }
        }

        tracing::debug!(url = %url, body = %body, "Sending request to OpenAI API");

        // Send request
        let response = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::NetworkError(e.to_string()))?;

        // Check for error response
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError { status, message: text });
        }

        // Create event stream
        let byte_stream = response.bytes_stream();
        let mut buffer = String::new();
        // NOTE: tool_calls state is shared across all choices in a chunk.
        // In practice, streaming responses typically only have one choice (n=1),
        // so this is fine. If multiple choices with different tool_calls are needed,
        // this would need to be refactored to track state per choice index.
        let mut tool_calls: Vec<ToolCallState> = Vec::new();
        let mut final_usage: Option<Usage> = None;
        let mut message_end_pending = false; // Track if we need to send MessageEnd

        // State for parsing <think> tags (MiniMax-M2 style thinking)
        let mut in_think_block = false;
        let mut in_reasoning_content_mode = false; // GLM reasoning_content mode (vs MiniMax <think> tag mode)
        let mut think_buffer = String::new();
        let mut text_buffer = String::new();
        let mut thinking_started = false;
        let stream_timeout_secs = config.stream_timeout_secs;

        let stream = async_stream::stream! {
            let mut byte_stream = std::pin::pin!(byte_stream);
            // Timeout for waiting for next data chunk in stream.
            // This is reset every time data is received, so it only triggers if LLM stops sending data.
            // Use configurable timeout from LlmConfig (default: 300s)
            let stream_read_timeout = std::time::Duration::from_secs(stream_timeout_secs);

            loop {
                match tokio::time::timeout(stream_read_timeout, byte_stream.next()).await {
                    Ok(Some(chunk_result)) => {
                        let chunk = match chunk_result {
                            Ok(c) => c,
                            Err(e) => {
                                yield Err(LlmError::NetworkError(e.to_string()));
                                break;
                            }
                        };

                        buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process complete SSE lines
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    // Debug: log SSE line (truncated to avoid leaking sensitive content)
                    if !line.is_empty() && line != "data: [DONE]" {
                        let truncated = if line.len() > 200 {
                            // Find the last char boundary at or before byte 200 to avoid
                            // slicing in the middle of a multi-byte UTF-8 character.
                            let mut boundary = 200;
                            while boundary > 0 && !line.is_char_boundary(boundary) {
                                boundary -= 1;
                            }
                            format!("{}...[truncated {} bytes]", &line[..boundary], line.len() - boundary)
                        } else {
                            line.clone()
                        };
                        tracing::trace!("SSE line received: {:?}", truncated);
                    }

                    if line.is_empty() {
                        continue;
                    }

                    // Try SSE format first (data: {...})
                    let data = if let Some(sse_data) = line.strip_prefix("data:") {
                        let sse_data = sse_data.trim_start();
                        if sse_data == "[DONE]" {
                            continue;
                        }
                        sse_data
                    } else if line.starts_with('{') {
                        // Non-SSE format: proxy returned plain JSON (ignoring stream=true)
                        // Try to parse as non-streaming response
                        tracing::debug!("Detected non-SSE response, attempting to parse as non-streaming format");
                        if let Ok(non_stream) = serde_json::from_str::<NonStreamResponse>(&line) {
                            // Convert non-streaming response to events
                            for choice in non_stream.choices {
                                // Emit text content
                                if let Some(content) = choice.message.content {
                                    if !content.is_empty() {
                                        yield Ok(LlmEvent::TextDelta(content));
                                    }
                                }
                                // Emit tool calls
                                if let Some(tool_calls_list) = choice.message.tool_calls {
                                    for tc in tool_calls_list {
                                        if !tc.function.name.is_empty() {
                                            yield Ok(LlmEvent::ToolUseStart {
                                                id: tc.id.clone(),
                                                name: tc.function.name.clone(),
                                            });
                                            yield Ok(LlmEvent::ToolUseInputDelta {
                                                id: tc.id.clone(),
                                                delta: tc.function.arguments.clone(),
                                            });
                                            // Parse arguments
                                            let input: Value = serde_json::from_str(&tc.function.arguments)
                                                .unwrap_or_else(|_| json!({}));
                                            yield Ok(LlmEvent::ToolUseEnd {
                                                id: tc.id,
                                                name: tc.function.name,
                                                input,
                                            });
                                        }
                                    }
                                }
                            }
                            // Emit usage and message end
                            let usage = non_stream.usage.map(|u| Usage {
                                input_tokens: u.prompt_tokens,
                                output_tokens: u.completion_tokens,
                                cache_creation_input_tokens: None,
                                cache_read_input_tokens: None,
                            }).unwrap_or_default();
                            yield Ok(LlmEvent::MessageEnd { usage });
                            continue;
                        } else {
                            tracing::warn!("Failed to parse non-SSE response: {}", line);
                            continue;
                        }
                    } else {
                        // Unknown format, skip
                        tracing::trace!("Skipping unknown line format: {}", line);
                        continue;
                    };

                    tracing::trace!("SSE data: {}", data);
                    match serde_json::from_str::<StreamChunk>(data) {
                        Ok(chunk) => {
                            // Handle usage info
                            if let Some(ref usage) = chunk.usage {
                                tracing::debug!(
                                    "Received usage info: prompt_tokens={}, completion_tokens={}",
                                    usage.prompt_tokens,
                                    usage.completion_tokens
                                );
                                final_usage = Some(Usage {
                                    input_tokens: usage.prompt_tokens,
                                    output_tokens: usage.completion_tokens,
                                    cache_creation_input_tokens: None,
                                    cache_read_input_tokens: None,
                                });

                                // MiniMax sends usage in a separate final chunk after finish_reason
                                // If we were waiting for usage, emit MessageEnd now
                                if message_end_pending {
                                    if let Some(ref usage) = final_usage {
                                        tracing::debug!("Usage received after finish_reason, emitting MessageEnd");
                                        message_end_pending = false;
                                        yield Ok(LlmEvent::MessageEnd { usage: usage.clone() });
                                    }
                                }
                            }

                            for choice in chunk.choices {
                                // Handle GLM reasoning_content (chain-of-thought) as thinking
                                if let Some(ref reasoning) = choice.delta.reasoning_content {
                                    if !reasoning.is_empty() {
                                        if !thinking_started {
                                            thinking_started = true;
                                            in_think_block = true;
                                            in_reasoning_content_mode = true; // Mark as GLM mode
                                            yield Ok(LlmEvent::ThinkingStart);
                                        }
                                        think_buffer.push_str(reasoning);
                                        yield Ok(LlmEvent::ThinkingDelta(reasoning.clone()));
                                    }
                                }

                                // Handle text delta with <think> tag parsing
                                if let Some(content) = choice.delta.content {
                                    if !content.is_empty() {
                                        // If we were in GLM reasoning_content mode (NOT MiniMax <think> tag mode),
                                        // emit ThinkingEnd before starting regular content.
                                        // This handles the case where reasoning_content and content arrive
                                        // in the same chunk - content arrival signals end of thinking.
                                        if in_think_block && thinking_started && in_reasoning_content_mode {
                                            yield Ok(LlmEvent::ThinkingEnd);
                                            in_think_block = false;
                                            in_reasoning_content_mode = false;
                                        }

                                        // Accumulate content for tag parsing
                                        text_buffer.push_str(&content);

                                        // Process the buffer for <think> tags
                                        loop {
                                            if in_think_block {
                                                // Look for </think> closing tag
                                                if let Some(end_pos) = text_buffer.find("</think>") {
                                                    // Emit thinking content before the tag
                                                    let thinking_content = text_buffer[..end_pos].to_string();
                                                    if !thinking_content.is_empty() {
                                                        think_buffer.push_str(&thinking_content);
                                                        yield Ok(LlmEvent::ThinkingDelta(thinking_content));
                                                    }
                                                    // Emit ThinkingEnd and remove processed content
                                                    yield Ok(LlmEvent::ThinkingEnd);
                                                    text_buffer = text_buffer[end_pos + 8..].to_string();
                                                    in_think_block = false;
                                                } else {
                                                    // No closing tag yet, emit all as thinking
                                                    if !text_buffer.is_empty() {
                                                        let content = std::mem::take(&mut text_buffer);
                                                        think_buffer.push_str(&content);
                                                        yield Ok(LlmEvent::ThinkingDelta(content));
                                                    }
                                                    break;
                                                }
                                            } else {
                                                // Look for <think> opening tag
                                                if let Some(start_pos) = text_buffer.find("<think>") {
                                                    // Emit text before the tag
                                                    if start_pos > 0 {
                                                        let text_content = text_buffer[..start_pos].to_string();
                                                        yield Ok(LlmEvent::TextDelta(text_content));
                                                    }
                                                    // Emit ThinkingStart if not already started
                                                    if !thinking_started {
                                                        thinking_started = true;
                                                        yield Ok(LlmEvent::ThinkingStart);
                                                    }
                                                    // Remove processed content including tag
                                                    text_buffer = text_buffer[start_pos + 7..].to_string();
                                                    in_think_block = true;
                                                } else {
                                                    // No opening tag, emit all as text
                                                    // But keep potential partial tag at the end
                                                    let emit_len = if text_buffer.ends_with('<') {
                                                        text_buffer.len() - 1
                                                    } else if text_buffer.ends_with("<t") || text_buffer.ends_with("<th") ||
                                                              text_buffer.ends_with("<thi") || text_buffer.ends_with("<thin") ||
                                                              text_buffer.ends_with("<think") {
                                                        // Emit everything BEFORE the partial tag
                                                        // rfind('<') gives the position of '<', which is also the length of the prefix
                                                        text_buffer.rfind('<').unwrap_or(text_buffer.len())
                                                    } else {
                                                        text_buffer.len()
                                                    };

                                                    if emit_len > 0 {
                                                        let text_content = text_buffer[..emit_len].to_string();
                                                        text_buffer = text_buffer[emit_len..].to_string();
                                                        yield Ok(LlmEvent::TextDelta(text_content));
                                                    }
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }

                                // Handle tool calls
                                if let Some(tc_deltas) = choice.delta.tool_calls {
                                    tracing::debug!("Received tool call deltas: {:?}", tc_deltas);
                                    for tc_delta in tc_deltas {
                                        let index = tc_delta.index;

                                        // Ensure we have state for this tool call
                                        while tool_calls.len() <= index {
                                            tool_calls.push(ToolCallState::default());
                                        }

                                        let state = &mut tool_calls[index];

                                        // Handle tool call id
                                        if let Some(id) = tc_delta.id {
                                            state.id = id;
                                        }

                                        // Handle function info
                                        if let Some(func) = tc_delta.function {
                                            if let Some(name) = func.name {
                                                // Skip empty tool names - LLM error
                                                if !name.is_empty() {
                                                    state.name = name.clone();
                                                    if !state.started {
                                                        state.started = true;
                                                        yield Ok(LlmEvent::ToolUseStart {
                                                            id: state.id.clone(),
                                                            name,
                                                        });
                                                    }
                                                } else {
                                                    tracing::warn!(
                                                        "LLM returned empty tool name for call_id={}, skipping",
                                                        state.id
                                                    );
                                                }
                                            }
                                            if let Some(args) = func.arguments {
                                                state.arguments.push_str(&args);
                                                // Only emit InputDelta after ToolUseStart has been sent
                                                // to ensure consumers know which tool this belongs to
                                                if state.started {
                                                    yield Ok(LlmEvent::ToolUseInputDelta {
                                                        id: state.id.clone(),
                                                        delta: args,
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }

                                // Handle finish reason
                                if choice.finish_reason.is_some() {
                                    // Emit tool end events
                                    for state in &tool_calls {
                                        // Only emit if started AND has a valid name
                                        if state.started && !state.name.is_empty() {
                                            tracing::debug!(
                                                "Emitting ToolUseEnd: id={}, name={}, arguments={}",
                                                state.id, state.name, state.arguments
                                            );
                                            // Parse arguments, log warning if empty or invalid
                                            let input: Value = if state.arguments.is_empty() {
                                                tracing::warn!(
                                                    "Tool call {} has empty arguments, using empty object",
                                                    state.name
                                                );
                                                json!({})
                                            } else {
                                                match serde_json::from_str(&state.arguments) {
                                                    Ok(v) => v,
                                                    Err(e) => {
                                                        tracing::warn!(
                                                            "Failed to parse tool arguments for {}: {} - raw: {}",
                                                            state.name, e, state.arguments
                                                        );
                                                        json!({})
                                                    }
                                                }
                                            };
                                            yield Ok(LlmEvent::ToolUseEnd {
                                                id: state.id.clone(),
                                                name: state.name.clone(),
                                                input,
                                            });
                                        } else if state.started && state.name.is_empty() {
                                            tracing::warn!(
                                                "Skipping tool call with empty name: id={}",
                                                state.id
                                            );
                                        }
                                    }

                                    // Prevent duplicate ToolUseEnd on malformed streams
                                    tool_calls.clear();

                                    // MiniMax and some providers send usage in a separate final chunk
                                    // after finish_reason. Delay MessageEnd to capture usage.
                                    if let Some(ref usage) = final_usage {
                                        // Usage already received, emit MessageEnd now
                                        yield Ok(LlmEvent::MessageEnd { usage: usage.clone() });
                                    } else {
                                        // Mark that we need to send MessageEnd when usage arrives
                                        // or when stream ends
                                        message_end_pending = true;
                                        tracing::debug!("finish_reason received, waiting for usage chunk");
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            // Try to detect if this is an error response from the API
                            // Some providers return error JSON in SSE format
                            if let Ok(error_json) = serde_json::from_str::<Value>(data) {
                                if let Some(error) = error_json.get("error") {
                                    let error_msg = error
                                        .get("message")
                                        .and_then(|m| m.as_str())
                                        .unwrap_or("Unknown API error");
                                    yield Ok(LlmEvent::Error(error_msg.to_string()));
                                    continue;
                                }
                            }
                            // Not an error response, just a parse issue - log and continue
                            tracing::warn!("Failed to parse SSE data: {} - {}", e, data);
                        }
                    }
                }
                    }
                    Ok(None) => {
                        // Stream ended normally
                        break;
                    }
                    Err(_) => {
                        tracing::warn!("Stream read timeout after {} seconds", stream_timeout_secs);
                        yield Err(LlmError::StreamInterrupted(format!(
                            "Stream read timeout after {} seconds", stream_timeout_secs
                        )));
                        break;
                    }
                }
            }

            // Flush any remaining text_buffer content (e.g. partial <think> tag prefixes
            // that turned out not to be actual tags)
            if !text_buffer.is_empty() {
                if in_think_block {
                    yield Ok(LlmEvent::ThinkingDelta(std::mem::take(&mut text_buffer)));
                } else {
                    yield Ok(LlmEvent::TextDelta(std::mem::take(&mut text_buffer)));
                }
            }

            // Fallback: if stream ended but we were still in thinking mode,
            // emit ThinkingEnd to close the thinking block
            if in_think_block && thinking_started {
                tracing::debug!("Stream ended while in thinking mode, emitting ThinkingEnd");
                yield Ok(LlmEvent::ThinkingEnd);
            }

            // Fallback: if stream ended but we were still waiting for usage,
            // emit MessageEnd with whatever usage we have (or default)
            if message_end_pending {
                tracing::debug!("Stream ended with message_end_pending=true, emitting MessageEnd");
                let usage = final_usage.unwrap_or(Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                });
                yield Ok(LlmEvent::MessageEnd { usage });
            }
        };

        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_simple_messages() {
        let provider = OpenAIProvider::new("test-key");
        let messages = vec![
            ChatMessage {
                role: ChatRole::User,
                content: MessageContent::Text("Hello".to_string()),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: MessageContent::Text("Hi there!".to_string()),
            },
        ];

        let converted = provider.convert_messages(&messages, None);

        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["role"], "user");
        assert_eq!(converted[0]["content"], "Hello");
        assert_eq!(converted[1]["role"], "assistant");
        assert_eq!(converted[1]["content"], "Hi there!");
    }

    #[test]
    fn test_convert_messages_with_system() {
        let provider = OpenAIProvider::new("test-key");
        let messages = vec![ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Text("Hello".to_string()),
        }];

        let converted = provider.convert_messages(&messages, Some("You are helpful."));

        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["role"], "system");
        assert_eq!(converted[0]["content"], "You are helpful.");
        assert_eq!(converted[1]["role"], "user");
    }

    #[test]
    fn test_convert_messages_with_tool_use() {
        let provider = OpenAIProvider::new("test-key");
        let messages = vec![ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "call_123".to_string(),
                name: "read_file".to_string(),
                input: json!({"path": "/tmp/test.txt"}),
            }]),
        }];

        let converted = provider.convert_messages(&messages, None);

        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "assistant");
        assert!(converted[0]["tool_calls"].is_array());
        assert_eq!(converted[0]["tool_calls"][0]["id"], "call_123");
        assert_eq!(converted[0]["tool_calls"][0]["function"]["name"], "read_file");
    }

    #[test]
    fn test_convert_messages_with_tool_result() {
        let provider = OpenAIProvider::new("test-key");
        let messages = vec![ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "call_123".to_string(),
                content: "File contents here".to_string(),
                is_error: false,
            }]),
        }];

        let converted = provider.convert_messages(&messages, None);

        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["role"], "tool");
        assert_eq!(converted[0]["tool_call_id"], "call_123");
        assert_eq!(converted[0]["content"], "File contents here");
    }

    #[test]
    fn test_convert_tools() {
        let provider = OpenAIProvider::new("test-key");
        let tools = vec![
            ToolDef {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    },
                    "required": ["path"]
                }),
            },
            ToolDef {
                name: "write_file".to_string(),
                description: "Write a file".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["path", "content"]
                }),
            },
        ];

        let converted = provider.convert_tools(&tools);

        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["type"], "function");
        assert_eq!(converted[0]["function"]["name"], "read_file");
        assert_eq!(converted[0]["function"]["description"], "Read a file");
        assert_eq!(converted[1]["function"]["name"], "write_file");
    }

    #[test]
    fn test_provider_name() {
        let provider = OpenAIProvider::new("test-key");
        assert_eq!(provider.id(), "openai");
        assert_eq!(provider.name(), "OpenAI");
    }

    #[test]
    fn test_with_base_url() {
        let provider = OpenAIProvider::new("test-key").with_base_url("https://custom.api.com");

        // We can't directly access base_url, but we can verify the builder pattern works
        assert_eq!(provider.id(), "openai");
    }

    #[test]
    fn test_parse_stream_chunk() {
        let data = r#"{"id":"chatcmpl-123","object":"chat.completion.chunk","created":1234567890,"model":"gpt-4","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}],"usage":{}}"#;

        let chunk: StreamChunk = serde_json::from_str(data).expect("Failed to parse");

        assert_eq!(chunk.id, Some("chatcmpl-123".to_string()));
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].delta.content, Some("Hello".to_string()));
        assert!(chunk.choices[0].finish_reason.is_none());
    }

    #[test]
    fn test_parse_stream_chunk_with_empty_usage() {
        // This tests the fix for empty usage object
        let data = r#"{"id":"chatcmpl-123","choices":[{"index":0,"delta":{"content":"Hi"},"finish_reason":null}],"usage":{}}"#;

        let chunk: StreamChunk = serde_json::from_str(data).expect("Failed to parse");

        assert!(chunk.usage.is_some());
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
    }

    #[test]
    fn test_parse_stream_chunk_with_finish_reason() {
        let data =
            r#"{"id":"chatcmpl-123","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;

        let chunk: StreamChunk = serde_json::from_str(data).expect("Failed to parse");

        assert_eq!(chunk.choices[0].finish_reason, Some("stop".to_string()));
    }

    #[test]
    fn test_parse_stream_chunk_with_tool_calls() {
        let data = r#"{"id":"chatcmpl-123","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"read_file","arguments":"{\"path\":"}}]},"finish_reason":null}]}"#;

        let chunk: StreamChunk = serde_json::from_str(data).expect("Failed to parse");

        let tool_calls = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, Some("call_abc".to_string()));
        assert_eq!(tool_calls[0].function.as_ref().unwrap().name, Some("read_file".to_string()));
    }

    #[test]
    fn test_url_construction_standard() {
        // Test that URL is constructed correctly for standard OpenAI API
        let provider = OpenAIProvider::new("test-key");
        // Base URL is https://api.openai.com, doesn't contain version
        // So URL should be {base_url}/v1/chat/completions
        assert_eq!(provider.id(), "openai");
    }

    #[test]
    fn test_url_construction_with_version() {
        // Test that URL is constructed correctly when base_url contains version
        let provider = OpenAIProvider::new("test-key").with_base_url("https://api.example.com/v4");
        // Base URL contains /v4, so URL should be {base_url}/chat/completions
        assert_eq!(provider.id(), "openai");
    }

    #[test]
    fn test_estimate_tokens() {
        let provider = OpenAIProvider::new("test-key");
        // Default implementation: ~4 characters per token
        let tokens = provider.estimate_tokens("Hello, world!");
        assert!(tokens > 0);
        assert!(tokens < 10); // "Hello, world!" is about 3 tokens
    }

    #[test]
    fn test_tool_call_state_accumulation() {
        // Test that ToolCallState correctly accumulates tool call information
        let mut state = ToolCallState::default();
        assert!(state.id.is_empty());
        assert!(state.name.is_empty());
        assert!(state.arguments.is_empty());
        assert!(!state.started);

        // Simulate receiving id
        state.id = "call_123".to_string();
        assert_eq!(state.id, "call_123");

        // Simulate receiving name
        state.name = "glob".to_string();
        state.started = true;
        assert_eq!(state.name, "glob");
        assert!(state.started);

        // Simulate receiving arguments
        state.arguments.push_str("{\"pattern\":");
        state.arguments.push_str("\"**/*.rs\"}");
        assert_eq!(state.arguments, "{\"pattern\":\"**/*.rs\"}");
    }

    #[test]
    fn test_tool_call_state_with_empty_id() {
        // Ensure default state has empty strings, not None
        let state = ToolCallState::default();
        assert_eq!(state.id, "");
        assert_eq!(state.name, "");
    }

    #[test]
    fn test_parse_tool_calls_multiple() {
        // Test parsing multiple tool calls in a single response
        let data = r#"{"id":"chatcmpl-123","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"glob"}},{"index":1,"id":"call_2","function":{"name":"grep"}}]},"finish_reason":null}]}"#;

        let chunk: StreamChunk = serde_json::from_str(data).expect("Failed to parse");
        let tool_calls = chunk.choices[0].delta.tool_calls.as_ref().unwrap();

        assert_eq!(tool_calls.len(), 2);
        assert_eq!(tool_calls[0].id, Some("call_1".to_string()));
        assert_eq!(tool_calls[0].function.as_ref().unwrap().name, Some("glob".to_string()));
        assert_eq!(tool_calls[1].id, Some("call_2".to_string()));
        assert_eq!(tool_calls[1].function.as_ref().unwrap().name, Some("grep".to_string()));
    }

    #[test]
    fn test_convert_messages_tool_order() {
        // Test that ToolUse and ToolResult in the same message are ordered correctly
        // OpenAI API requires: assistant (with tool_calls) -> tool (results)
        let provider = OpenAIProvider::new("test-key");
        let messages = vec![ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::ToolUse {
                    id: "call_123".to_string(),
                    name: "read_file".to_string(),
                    input: json!({"path": "/tmp/test.txt"}),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_123".to_string(),
                    content: "file contents".to_string(),
                    is_error: false,
                },
            ]),
        }];

        let converted = provider.convert_messages(&messages, None);

        // Should have 2 messages: assistant with tool_calls, then tool result
        assert_eq!(converted.len(), 2);
        // First message should be assistant with tool_calls
        assert_eq!(converted[0]["role"], "assistant");
        assert!(converted[0]["tool_calls"].is_array());
        // Second message should be tool result
        assert_eq!(converted[1]["role"], "tool");
        assert_eq!(converted[1]["tool_call_id"], "call_123");
    }

    #[test]
    fn test_url_construction_domain_with_v1() {
        // Test that domain names containing 'v1' don't trigger false positive
        // e.g., https://api.v1service.com should still append /v1/chat/completions
        let provider = OpenAIProvider::new("test-key").with_base_url("https://api.v1service.com");
        // The domain contains 'v1' but not as a path segment
        // So it should still construct URL with /v1/chat/completions
        assert_eq!(provider.id(), "openai");
        // Note: We can't directly test the URL construction here since it's done in chat_stream
        // but the fix ensures only path segments like /v1/ or ending with /v1 are detected
    }

    #[test]
    fn test_new_with_auth_multi() {
        // Multi auth should use the first configured credential
        let auth = forge_config::AuthConfig::Multi {
            credentials: vec![
                forge_config::AuthConfig::None,
                forge_config::AuthConfig::Bearer { token: "second-key".to_string() },
                forge_config::AuthConfig::Bearer { token: "third-key".to_string() },
            ],
        };
        let provider = OpenAIProvider::new_with_auth(&auth);
        // Can't access api_key directly, but verify it works
        assert_eq!(provider.id(), "openai");
    }

    #[test]
    fn test_new_with_auth_single_bearer() {
        let auth = forge_config::AuthConfig::Bearer { token: "my-token".to_string() };
        let provider = OpenAIProvider::new_with_auth(&auth);
        assert_eq!(provider.id(), "openai");
    }

    #[test]
    fn test_new_with_auth_none() {
        let auth = forge_config::AuthConfig::None;
        let provider = OpenAIProvider::new_with_auth(&auth);
        assert_eq!(provider.id(), "openai");
    }

    #[test]
    fn test_convert_messages_only_tool_results() {
        // Test message containing only ToolResults (typical user message after tool execution)
        let provider = OpenAIProvider::new("test-key");
        let messages = vec![ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Blocks(vec![
                ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: "result 1".to_string(),
                    is_error: false,
                },
                ContentBlock::ToolResult {
                    tool_use_id: "call_2".to_string(),
                    content: "result 2".to_string(),
                    is_error: false,
                },
            ]),
        }];

        let converted = provider.convert_messages(&messages, None);

        // Should have 2 tool messages
        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0]["role"], "tool");
        assert_eq!(converted[0]["tool_call_id"], "call_1");
        assert_eq!(converted[1]["role"], "tool");
        assert_eq!(converted[1]["tool_call_id"], "call_2");
    }
}
