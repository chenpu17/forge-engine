//! Google Gemini API adapter
//!
//! Implements the `LlmProvider` trait for Google's Gemini models.
//! Gemini uses a different API format from `OpenAI` (`functionDeclarations` vs tools,
//! different SSE format), requiring a native implementation.

use crate::error::LlmError;
use crate::{
    provider::ModelInfo, ChatMessage, ChatRole, ContentBlock, LlmConfig, LlmEvent, LlmEventStream,
    LlmProvider, MessageContent, Result, ToolDef, Usage,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

/// Default Gemini API base URL
const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";

/// Gemini API client
pub struct GeminiProvider {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl GeminiProvider {
    /// Create a new Gemini provider with an API key
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            client: crate::base::create_http_client(false),
        }
    }

    /// Create a new Gemini provider from an `AuthConfig`
    pub fn new_with_auth(auth: &forge_config::AuthConfig) -> Self {
        Self::new(crate::base::extract_auth_token(auth))
    }

    /// Set custom base URL
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Configure proxy settings
    #[must_use]
    pub fn with_proxy_config(mut self, proxy: Option<&forge_config::ProxyConfig>) -> Self {
        self.client = crate::base::create_http_client_with_proxy(false, proxy);
        self
    }

    /// Build the streaming URL for a model
    fn build_url(&self, model: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            base, model, self.api_key
        )
    }

    /// Build a redacted URL for logging (hides the API key)
    fn build_url_redacted(&self, model: &str) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{base}/v1beta/models/{model}:streamGenerateContent?alt=sse&key=REDACTED")
    }

    /// Convert `ChatMessages` to Gemini `contents` format
    #[allow(clippy::unused_self)]
    fn convert_messages(&self, messages: &[ChatMessage]) -> Vec<Value> {
        // Pre-build a tool_use_id → function_name lookup across all messages.
        // This is needed because ToolResult (in a user message) references a
        // ToolUse (in the preceding assistant message) by tool_use_id.
        let tool_name_map: std::collections::HashMap<&str, &str> = messages
            .iter()
            .flat_map(|m| match &m.content {
                MessageContent::Blocks(blocks) => blocks.as_slice(),
                MessageContent::Text(_) => &[],
            })
            .filter_map(|b| {
                if let ContentBlock::ToolUse { id, name, .. } = b {
                    Some((id.as_str(), name.as_str()))
                } else {
                    None
                }
            })
            .collect();

        messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    ChatRole::User => "user",
                    ChatRole::Assistant => "model",
                };

                let parts = match &msg.content {
                    MessageContent::Text(text) => vec![json!({"text": text})],
                    MessageContent::Blocks(blocks) => {
                        let mut parts = Vec::new();
                        for block in blocks {
                            match block {
                                ContentBlock::Text { text } => {
                                    parts.push(json!({"text": text}));
                                }
                                ContentBlock::ToolUse { id: _, name, input } => {
                                    parts.push(json!({
                                        "functionCall": {
                                            "name": name,
                                            "args": input
                                        }
                                    }));
                                }
                                ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                                    // Look up the function name from the cross-message map.
                                    let fn_name = tool_name_map
                                        .get(tool_use_id.as_str())
                                        .copied()
                                        .unwrap_or("function");
                                    parts.push(json!({
                                        "functionResponse": {
                                            "name": fn_name,
                                            "response": {
                                                "content": content,
                                                "is_error": is_error
                                            }
                                        }
                                    }));
                                }
                            }
                        }
                        parts
                    }
                };

                json!({
                    "role": role,
                    "parts": parts
                })
            })
            .collect()
    }

    /// Convert `ToolDef` to Gemini `functionDeclarations` format
    #[allow(clippy::unused_self)]
    fn convert_tools(&self, tools: &[ToolDef]) -> Value {
        let declarations: Vec<Value> = tools
            .iter()
            .map(|tool| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters
                })
            })
            .collect();

        json!([{
            "functionDeclarations": declarations
        }])
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    #[allow(clippy::unnecessary_literal_bound)]
    fn id(&self) -> &str {
        "gemini"
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "Google Gemini"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo::new("gemini-2.0-flash", "Gemini 2.0 Flash", 1_048_576).with_vision(true),
            ModelInfo::new("gemini-2.0-flash-lite", "Gemini 2.0 Flash Lite", 1_048_576)
                .with_vision(true),
            ModelInfo::new("gemini-1.5-pro", "Gemini 1.5 Pro", 2_097_152).with_vision(true),
            ModelInfo::new("gemini-1.5-flash", "Gemini 1.5 Flash", 1_048_576).with_vision(true),
        ]
    }

    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(name = "llm_call", skip_all, fields(provider = "gemini", model = %config.model))]
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: Vec<ToolDef>,
        config: &LlmConfig,
    ) -> Result<LlmEventStream> {
        let url = self.build_url(&config.model);

        // Build request body
        let mut body = json!({
            "contents": self.convert_messages(messages),
            "generationConfig": {
                "maxOutputTokens": config.max_tokens,
                "temperature": config.temperature
            }
        });

        // Add system instruction if provided
        if let Some(ref system) = config.system_prompt {
            body["systemInstruction"] = json!({
                "parts": [{"text": system}]
            });
        } else if let Some(ref blocks) = config.system_blocks {
            // Fall back to system_blocks (concatenated) when system_prompt is absent
            if !blocks.is_empty() {
                let combined: String =
                    blocks.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n\n");
                body["systemInstruction"] = json!({
                    "parts": [{"text": combined}]
                });
            }
        }

        // Add structured output via Gemini's native responseSchema
        if let Some(ref schema) = config.response_schema {
            body["generationConfig"]["responseMimeType"] = json!("application/json");
            body["generationConfig"]["responseSchema"] = schema.clone();
            tracing::debug!("Structured output enabled via Gemini responseSchema");
        }

        // Add tools if provided
        if !tools.is_empty() {
            body["tools"] = self.convert_tools(&tools);
        }

        tracing::debug!(
            model = %config.model,
            messages = messages.len(),
            tools = tools.len(),
            url = %self.build_url_redacted(&config.model),
            "Sending request to Gemini API"
        );

        // Send request
        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                // Redact API key from reqwest error messages (the key is in the URL)
                let msg = e.to_string().replace(&self.api_key, "REDACTED");
                LlmError::NetworkError(msg)
            })?;

        // Check for errors
        let status = response.status();
        if !status.is_success() {
            let error_text =
                response.text().await.unwrap_or_default().replace(&self.api_key, "REDACTED");
            return Err(LlmError::ApiError { status: status.as_u16(), message: error_text });
        }

        // Parse SSE stream
        let byte_stream = response.bytes_stream();
        let stream_timeout_secs = config.stream_timeout_secs;

        let stream = async_stream::stream! {
            let mut byte_stream = std::pin::pin!(byte_stream);
            let mut buffer = String::new();
            let mut total_input_tokens = 0usize;
            let mut total_output_tokens = 0usize;
            let mut tool_call_counter = 0u32;
            let mut message_end_emitted = false;
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

                            if line.is_empty() {
                                continue;
                            }

                            let data = if let Some(sse_data) = line.strip_prefix("data:") {
                                sse_data.trim_start()
                            } else {
                                continue;
                            };

                            if data == "[DONE]" {
                                continue;
                            }

                            match serde_json::from_str::<GeminiResponse>(data) {
                                Ok(resp) => {
                                    // Track usage
                                    if let Some(ref meta) = resp.usage_metadata {
                                        total_input_tokens = meta.prompt_token_count.unwrap_or(0);
                                        total_output_tokens = meta.candidates_token_count.unwrap_or(0);
                                    }

                                    // Process candidates
                                    for candidate in resp.candidates.iter().flatten() {
                                        if let Some(ref content) = candidate.content {
                                            for part in &content.parts {
                                                // Text content
                                                if let Some(ref text) = part.text {
                                                    if !text.is_empty() {
                                                        yield Ok(LlmEvent::TextDelta(text.clone()));
                                                    }
                                                }

                                                // Function call
                                                if let Some(ref fc) = part.function_call {
                                                    tool_call_counter += 1;
                                                    let id = format!("gemini-fc-{}-{}", fc.name, tool_call_counter);
                                                    yield Ok(LlmEvent::ToolUseStart {
                                                        id: id.clone(),
                                                        name: fc.name.clone(),
                                                    });
                                                    let args_str = serde_json::to_string(&fc.args)
                                                        .unwrap_or_else(|_| "{}".to_string());
                                                    yield Ok(LlmEvent::ToolUseInputDelta {
                                                        id: id.clone(),
                                                        delta: args_str,
                                                    });
                                                    yield Ok(LlmEvent::ToolUseEnd {
                                                        id,
                                                        name: fc.name.clone(),
                                                        input: fc.args.clone(),
                                                    });
                                                }
                                            }
                                        }

                                        // Check finish reason
                                        if let Some(ref reason) = candidate.finish_reason {
                                            if reason != "STOP" && reason != "MAX_TOKENS" {
                                                tracing::warn!(
                                                    finish_reason = %reason,
                                                    "Gemini response terminated with non-standard reason"
                                                );
                                            }
                                            if !message_end_emitted {
                                                message_end_emitted = true;
                                                yield Ok(LlmEvent::MessageEnd {
                                                    usage: Usage {
                                                        input_tokens: total_input_tokens,
                                                        output_tokens: total_output_tokens,
                                                        cache_creation_input_tokens: None,
                                                        cache_read_input_tokens: None,
                                                    },
                                                });
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to parse Gemini SSE data: {} - {}", e, data);
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        // Fallback: if stream ended without a finish_reason but we received data,
                        // emit MessageEnd so consumers don't hang waiting for it.
                        if !message_end_emitted && (total_output_tokens > 0 || total_input_tokens > 0) {
                            yield Ok(LlmEvent::MessageEnd {
                                usage: Usage {
                                    input_tokens: total_input_tokens,
                                    output_tokens: total_output_tokens,
                                    cache_creation_input_tokens: None,
                                    cache_read_input_tokens: None,
                                },
                            });
                        }
                        break;
                    }
                    Err(_) => {
                        tracing::warn!("Gemini stream read timeout after {stream_timeout_secs} seconds");
                        yield Err(LlmError::StreamInterrupted(format!(
                            "Stream read timeout after {stream_timeout_secs} seconds"
                        )));
                        break;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    fn estimate_tokens(&self, text: &str) -> usize {
        // Use character count (not byte length) for correct CJK/Unicode handling
        text.chars().count() / 4
    }
}

// ---------------------------------------------------------------------------
// Gemini response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    text: Option<String>,
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCall {
    name: String,
    args: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: Option<usize>,
    candidates_token_count: Option<usize>,
}
