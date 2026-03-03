//! Mock LLM Provider for testing
//!
//! Provides a configurable mock LLM that can simulate various response scenarios.

use crate::{ConfirmationHandler, ConfirmationLevel, Result as AgentResult};
use async_trait::async_trait;
use forge_llm::{
    ChatMessage, LlmConfig, LlmError, LlmEvent, LlmEventStream, LlmProvider, ModelInfo, Result,
    ToolDef, Usage,
};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Auto-approve confirmation handler for testing
///
/// This handler automatically approves all tool confirmation requests,
/// useful for E2E tests where user interaction is not possible.
pub struct AutoApproveHandler;

#[async_trait]
impl ConfirmationHandler for AutoApproveHandler {
    async fn request_confirmation(
        &self,
        _id: &str,
        _tool: &str,
        _params: &serde_json::Value,
        _level: ConfirmationLevel,
    ) -> AgentResult<bool> {
        // Always approve for testing
        Ok(true)
    }
}

/// A mock response for testing
#[derive(Debug, Clone)]
pub enum MockResponse {
    /// Simple text response
    Text(String),
    /// Tool call response
    ToolCall {
        /// Tool ID
        id: String,
        /// Tool name
        name: String,
        /// Tool input JSON
        input: String,
    },
    /// Multiple tool calls
    ToolCalls(Vec<(String, String, String)>), // (id, name, input)
    /// Error response
    Error(String),
    /// Delayed response (for timeout testing)
    Delayed {
        /// Response after delay
        response: Box<Self>,
        /// Delay in milliseconds
        delay_ms: u64,
    },
}

/// Mock LLM Provider for testing
pub struct MockLlmProvider {
    /// Queue of responses to return
    responses: Arc<Mutex<VecDeque<MockResponse>>>,
    /// Record of all messages received
    recorded_messages: Arc<Mutex<Vec<Vec<ChatMessage>>>>,
    /// Record of system prompts received (from `LlmConfig`)
    recorded_system_prompts: Arc<Mutex<Vec<Option<String>>>>,
    /// Whether to return empty response when queue is empty
    return_empty_on_exhausted: bool,
}

impl MockLlmProvider {
    /// Create a new mock provider
    #[must_use]
    pub fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::new())),
            recorded_messages: Arc::new(Mutex::new(Vec::new())),
            recorded_system_prompts: Arc::new(Mutex::new(Vec::new())),
            return_empty_on_exhausted: true,
        }
    }

    /// Add a response to the queue
    pub fn push_response(&self, response: MockResponse) {
        self.responses.lock().push_back(response);
    }

    /// Add multiple responses
    pub fn push_responses(&self, responses: impl IntoIterator<Item = MockResponse>) {
        let mut queue = self.responses.lock();
        for r in responses {
            queue.push_back(r);
        }
    }

    /// Get recorded messages
    #[must_use]
    pub fn get_recorded_messages(&self) -> Vec<Vec<ChatMessage>> {
        self.recorded_messages.lock().clone()
    }

    /// Get recorded system prompts
    #[must_use]
    pub fn get_recorded_system_prompts(&self) -> Vec<Option<String>> {
        self.recorded_system_prompts.lock().clone()
    }

    /// Clear recorded messages
    pub fn clear_recorded(&self) {
        self.recorded_messages.lock().clear();
        self.recorded_system_prompts.lock().clear();
    }

    /// Set behavior when responses are exhausted
    pub const fn set_return_empty_on_exhausted(&mut self, value: bool) {
        self.return_empty_on_exhausted = value;
    }

    /// Create a provider that returns a simple text response
    #[must_use]
    pub fn with_text_response(text: impl Into<String>) -> Self {
        let provider = Self::new();
        provider.push_response(MockResponse::Text(text.into()));
        provider
    }

    /// Create a provider that returns a tool call
    #[must_use]
    pub fn with_tool_call(id: &str, name: &str, input: &str) -> Self {
        let provider = Self::new();
        provider.push_response(MockResponse::ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input: input.to_string(),
        });
        provider
    }

    /// Create a provider that returns an error
    pub fn with_error(message: impl Into<String>) -> Self {
        let provider = Self::new();
        provider.push_response(MockResponse::Error(message.into()));
        provider
    }
}

impl Default for MockLlmProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn name(&self) -> &'static str {
        "Mock Provider"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        // Support common models for testing
        vec![
            // Wildcard model - accepts ANY model name in tests
            // This allows tests to work regardless of FORGE_LLM_MODEL env var
            ModelInfo::new("*", "Any Model (Mock Wildcard)", 200_000),
            ModelInfo::new("mock-model", "Mock Model", 200_000),
            // Support Anthropic models
            ModelInfo::new("claude-sonnet-4-20250514", "Claude Sonnet 4", 200_000),
            ModelInfo::new("claude-3-opus-20240229", "Claude 3 Opus", 200_000),
            ModelInfo::new("claude-3-sonnet-20240229", "Claude 3 Sonnet", 200_000),
            ModelInfo::new("claude-3-haiku-20240307", "Claude 3 Haiku", 200_000),
            // Support test models
            ModelInfo::new("test-model", "Test Model", 200_000),
        ]
    }

    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        _tools: Vec<ToolDef>,
        config: &LlmConfig,
    ) -> Result<LlmEventStream> {
        // Record the messages
        self.recorded_messages.lock().push(messages.to_vec());
        self.recorded_system_prompts.lock().push(config.system_prompt.clone());

        // Get next response
        let response = self.responses.lock().pop_front();

        let (tx, rx) = mpsc::channel::<std::result::Result<LlmEvent, LlmError>>(10);

        // Spawn task to send events
        let response = response.unwrap_or_else(|| {
            if self.return_empty_on_exhausted {
                MockResponse::Text(String::new())
            } else {
                MockResponse::Error("No more responses in queue".to_string())
            }
        });

        tokio::spawn(async move {
            send_mock_response(&tx, response).await;
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }
}

async fn send_mock_response(
    tx: &mpsc::Sender<std::result::Result<LlmEvent, LlmError>>,
    response: MockResponse,
) {
    match response {
        MockResponse::Text(text) => {
            // Send text in chunks to simulate streaming
            for chunk in text.chars().collect::<Vec<_>>().chunks(10) {
                let delta: String = chunk.iter().collect();
                let _ = tx.send(Ok(LlmEvent::TextDelta(delta))).await;
            }
            let _ = tx
                .send(Ok(LlmEvent::MessageEnd {
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: text.len() / 4,
                        cache_read_input_tokens: None,
                        cache_creation_input_tokens: None,
                    },
                }))
                .await;
        }
        MockResponse::ToolCall { id, name, input } => {
            let _ =
                tx.send(Ok(LlmEvent::ToolUseStart { id: id.clone(), name: name.clone() })).await;
            let _ = tx
                .send(Ok(LlmEvent::ToolUseInputDelta { id: id.clone(), delta: input.clone() }))
                .await;
            // Parse input as JSON Value — panic on invalid JSON so test failures are obvious
            #[allow(clippy::expect_used)]
            let input_value: serde_json::Value = serde_json::from_str(&input)
                .expect("MockResponse::ToolCall input must be valid JSON");
            let _ = tx.send(Ok(LlmEvent::ToolUseEnd { id, name, input: input_value })).await;
            let _ = tx
                .send(Ok(LlmEvent::MessageEnd {
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 50,
                        cache_read_input_tokens: None,
                        cache_creation_input_tokens: None,
                    },
                }))
                .await;
        }
        MockResponse::ToolCalls(calls) => {
            for (id, name, input) in calls {
                let _ = tx
                    .send(Ok(LlmEvent::ToolUseStart { id: id.clone(), name: name.clone() }))
                    .await;
                let _ = tx
                    .send(Ok(LlmEvent::ToolUseInputDelta { id: id.clone(), delta: input.clone() }))
                    .await;
                // Parse input as JSON Value — panic on invalid JSON so test failures are obvious
                #[allow(clippy::expect_used)]
                let input_value: serde_json::Value = serde_json::from_str(&input)
                    .expect("MockResponse::ToolCalls input must be valid JSON");
                let _ = tx.send(Ok(LlmEvent::ToolUseEnd { id, name, input: input_value })).await;
            }
            let _ = tx
                .send(Ok(LlmEvent::MessageEnd {
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 100,
                        cache_read_input_tokens: None,
                        cache_creation_input_tokens: None,
                    },
                }))
                .await;
        }
        MockResponse::Error(msg) => {
            let _ = tx.send(Ok(LlmEvent::Error(msg))).await;
        }
        MockResponse::Delayed { response, delay_ms } => {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            Box::pin(send_mock_response(tx, *response)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_mock_text_response() {
        let provider = MockLlmProvider::with_text_response("Hello, world!");
        let config = LlmConfig::default();

        let mut stream = provider.chat_stream(&[], vec![], &config).await.unwrap();

        let mut text = String::new();
        while let Some(Ok(event)) = stream.next().await {
            if let LlmEvent::TextDelta(delta) = event {
                text.push_str(&delta);
            }
        }

        assert_eq!(text, "Hello, world!");
    }

    #[tokio::test]
    async fn test_mock_tool_call() {
        let provider =
            MockLlmProvider::with_tool_call("call_1", "read", r#"{"path": "/test.txt"}"#);
        let config = LlmConfig::default();

        let mut stream = provider.chat_stream(&[], vec![], &config).await.unwrap();

        let mut tool_name = None;
        while let Some(Ok(event)) = stream.next().await {
            if let LlmEvent::ToolUseStart { id: _, name } = event {
                tool_name = Some(name);
            }
        }

        assert_eq!(tool_name, Some("read".to_string()));
    }

    #[tokio::test]
    async fn test_mock_error() {
        let provider = MockLlmProvider::with_error("Test error");
        let config = LlmConfig::default();

        let mut stream = provider.chat_stream(&[], vec![], &config).await.unwrap();

        let mut error_msg = None;
        while let Some(Ok(event)) = stream.next().await {
            if let LlmEvent::Error(msg) = event {
                error_msg = Some(msg);
            }
        }

        assert_eq!(error_msg, Some("Test error".to_string()));
    }

    #[tokio::test]
    async fn test_multiple_responses() {
        let provider = MockLlmProvider::new();
        provider.push_responses([
            MockResponse::Text("First".to_string()),
            MockResponse::Text("Second".to_string()),
        ]);
        let config = LlmConfig::default();

        // First call
        let mut stream = provider.chat_stream(&[], vec![], &config).await.unwrap();
        let mut text = String::new();
        while let Some(Ok(event)) = stream.next().await {
            if let LlmEvent::TextDelta(delta) = event {
                text.push_str(&delta);
            }
        }
        assert_eq!(text, "First");

        // Second call
        let mut stream = provider.chat_stream(&[], vec![], &config).await.unwrap();
        let mut text = String::new();
        while let Some(Ok(event)) = stream.next().await {
            if let LlmEvent::TextDelta(delta) = event {
                text.push_str(&delta);
            }
        }
        assert_eq!(text, "Second");
    }

    #[tokio::test]
    async fn test_recorded_messages() {
        let provider = MockLlmProvider::with_text_response("Response");
        let config = LlmConfig::default();

        let messages = vec![ChatMessage {
            role: forge_llm::ChatRole::User,
            content: forge_llm::MessageContent::Text("Hello".to_string()),
        }];

        let mut stream = provider.chat_stream(&messages, vec![], &config).await.unwrap();

        // Consume stream
        while stream.next().await.is_some() {}

        let recorded = provider.get_recorded_messages();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].len(), 1);
    }
}
