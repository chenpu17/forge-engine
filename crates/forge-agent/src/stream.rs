//! LLM event stream processing.
//!
//! Collects text deltas and tool calls from the LLM response stream.

use crate::{AgentError, Result};
use forge_domain::{AgentEvent, ToolCall, Usage};
use forge_llm::LlmEvent;
use futures::StreamExt;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::trace_recorder::TraceRecorder;

// ---------------------------------------------------------------------------
// Streaming tool assembler
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PartialToolCall {
    name: String,
    input_delta: String,
}

/// Assembles tool calls from streaming input deltas.
///
/// When `streaming_tools` is enabled, the assembler accumulates
/// `ToolUseInputDelta` fragments and parses the final JSON on `ToolUseEnd`.
#[derive(Default)]
pub struct ToolStreamAssembler {
    partials: HashMap<String, PartialToolCall>,
}

impl ToolStreamAssembler {
    /// Record the start of a tool call.
    pub(crate) fn on_start(&mut self, id: &str, name: &str) {
        let entry = self.partials.entry(id.to_string()).or_default();
        if !name.is_empty() {
            entry.name = name.to_string();
        }
    }

    /// Append an input delta fragment.
    pub(crate) fn on_input_delta(&mut self, id: &str, delta: &str) {
        let entry = self.partials.entry(id.to_string()).or_default();
        entry.input_delta.push_str(delta);
    }

    /// Finalize a tool call, returning `Some(ToolCall)` on success.
    pub(crate) fn on_end(
        &mut self,
        id: String,
        name: String,
        input: serde_json::Value,
    ) -> Option<ToolCall> {
        let partial = self.partials.remove(&id).unwrap_or_default();
        let final_name = if name.is_empty() { partial.name } else { name };
        if final_name.is_empty() {
            tracing::warn!(id = %id, "LLM returned tool call with empty name, skipping");
            return None;
        }

        let final_input = if input.is_null() {
            parse_tool_input_delta(&partial.input_delta).unwrap_or(serde_json::Value::Null)
        } else {
            input
        };

        Some(ToolCall { id, name: final_name, input: final_input })
    }
}

// ---------------------------------------------------------------------------
// Stream processing
// ---------------------------------------------------------------------------

/// Process LLM event stream, collecting text and tool calls.
///
/// Maps [`LlmEvent`] variants to [`AgentEvent`] variants and sends them
/// through the channel. Returns the accumulated full text and parsed tool
/// calls on success.
pub async fn process_llm_stream(
    mut stream: forge_llm::LlmEventStream,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    cancellation: &CancellationToken,
    enable_streaming_tool_assembly: bool,
    mut trace_recorder: Option<&mut TraceRecorder>,
) -> Result<(String, Vec<ToolCall>, Option<Usage>)> {
    let mut full_text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut tool_names: HashMap<String, String> = HashMap::new();
    let mut assembler = enable_streaming_tool_assembly.then(ToolStreamAssembler::default);
    let mut last_usage: Option<Usage> = None;

    while let Some(event_result) = stream.next().await {
        if cancellation.is_cancelled() {
            let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
            return Err(AgentError::Aborted);
        }

        match event_result {
            Ok(ref event) => {
                // Record to trace if enabled
                if let Some(ref mut rec) = trace_recorder {
                    rec.record_llm_event(event);
                }
                match event {
                    LlmEvent::TextDelta(delta) => {
                        full_text.push_str(delta);
                        let _ = tx.send(Ok(AgentEvent::TextDelta { delta: delta.clone() })).await;
                    }
                    LlmEvent::ToolUseStart { id, name } => {
                        tool_names.insert(id.clone(), name.clone());
                        if let Some(a) = assembler.as_mut() {
                            a.on_start(id, name);
                        }
                        let _ = tx
                            .send(Ok(AgentEvent::ToolCallStart {
                                id: id.clone(),
                                name: name.clone(),
                                input: serde_json::Value::Null,
                            }))
                            .await;
                    }
                    LlmEvent::ToolUseInputDelta { id, delta } => {
                        if let Some(a) = assembler.as_mut() {
                            a.on_input_delta(id, delta);
                        }
                    }
                    LlmEvent::ThinkingStart => {
                        let _ = tx.send(Ok(AgentEvent::ThinkingStart)).await;
                    }
                    LlmEvent::ThinkingDelta(content) => {
                        let _ = tx.send(Ok(AgentEvent::Thinking { content: content.clone() })).await;
                    }
                    LlmEvent::ThinkingEnd => {}
                    LlmEvent::ToolUseEnd { id, name, input } => {
                        if let Some(a) = assembler.as_mut() {
                            if let Some(call) = a.on_end(id.clone(), name.clone(), input.clone()) {
                                tool_names.remove(id);
                                tool_calls.push(call);
                            }
                        } else {
                            let tool_name = if name.is_empty() {
                                tool_names.remove(id).unwrap_or_default()
                            } else {
                                tool_names.remove(id);
                                name.clone()
                            };
                            if tool_name.is_empty() {
                                tracing::warn!(id = %id, "LLM returned tool call with empty name, skipping");
                                continue;
                            }
                            tool_calls.push(ToolCall { id: id.clone(), name: tool_name, input: input.clone() });
                        }
                    }
                    LlmEvent::MessageEnd { usage } => {
                        last_usage = Some(Usage {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                            cache_read_input_tokens: usage.cache_read_input_tokens,
                            cache_creation_input_tokens: usage.cache_creation_input_tokens,
                        });
                        let _ = tx
                            .send(Ok(AgentEvent::TokenUsage {
                                input_tokens: usage.input_tokens,
                                output_tokens: usage.output_tokens,
                                cache_read_tokens: usage.cache_read_input_tokens,
                                cache_creation_tokens: usage.cache_creation_input_tokens,
                            }))
                            .await;
                    }
                    LlmEvent::Error(e) => {
                        let _ = tx.send(Ok(AgentEvent::Error { message: e.clone() })).await;
                        return Err(AgentError::LlmError(e.clone()));
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(Ok(AgentEvent::Error { message: e.to_string() })).await;
                return Err(AgentError::LlmError(e.to_string()));
            }
        }
    }

    Ok((full_text, tool_calls, last_usage))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a raw JSON string accumulated from tool input deltas.
pub fn parse_tool_input_delta(raw: &str) -> Option<serde_json::Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(parsed) => Some(parsed),
        Err(e) => {
            tracing::debug!(error = %e, "Failed to parse tool input delta as JSON");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::mpsc;

    #[test]
    fn test_parse_tool_input_delta_json() {
        let parsed = parse_tool_input_delta(r#"{"path":"/tmp/a.txt"}"#).expect("should parse");
        assert_eq!(parsed["path"], "/tmp/a.txt");
    }

    #[test]
    fn test_parse_tool_input_delta_empty() {
        assert!(parse_tool_input_delta("").is_none());
        assert!(parse_tool_input_delta("   ").is_none());
    }

    #[test]
    fn test_parse_tool_input_delta_invalid() {
        assert!(parse_tool_input_delta("{not json").is_none());
    }

    #[tokio::test]
    async fn test_process_llm_stream_assembles_deltas_when_enabled() {
        let (tx, _rx) = mpsc::channel(8);
        let cancellation = CancellationToken::new();
        let events = vec![
            Ok(LlmEvent::ToolUseStart { id: "call_1".to_string(), name: "read".to_string() }),
            Ok(LlmEvent::ToolUseInputDelta {
                id: "call_1".to_string(),
                delta: r#"{"path":"/tmp/"#.to_string(),
            }),
            Ok(LlmEvent::ToolUseInputDelta {
                id: "call_1".to_string(),
                delta: r#"a.txt"}"#.to_string(),
            }),
            Ok(LlmEvent::ToolUseEnd {
                id: "call_1".to_string(),
                name: "".to_string(),
                input: serde_json::Value::Null,
            }),
        ];
        let stream: forge_llm::LlmEventStream = Box::pin(futures::stream::iter(events));
        let (_text, tool_calls, _usage) = process_llm_stream(stream, &tx, &cancellation, true, None)
            .await
            .expect("stream should be processed");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "read");
        assert_eq!(tool_calls[0].input["path"], "/tmp/a.txt");
    }

    #[tokio::test]
    async fn test_process_llm_stream_uses_end_input_when_disabled() {
        let (tx, _rx) = mpsc::channel(8);
        let cancellation = CancellationToken::new();
        let events = vec![
            Ok(LlmEvent::ToolUseStart { id: "call_2".to_string(), name: "read".to_string() }),
            Ok(LlmEvent::ToolUseInputDelta {
                id: "call_2".to_string(),
                delta: r#"{"path":"ignored"}"#.to_string(),
            }),
            Ok(LlmEvent::ToolUseEnd {
                id: "call_2".to_string(),
                name: "".to_string(),
                input: json!({"path": "/tmp/final.txt"}),
            }),
        ];
        let stream: forge_llm::LlmEventStream = Box::pin(futures::stream::iter(events));
        let (_text, tool_calls, _usage) = process_llm_stream(stream, &tx, &cancellation, false, None)
            .await
            .expect("stream should be processed");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].input["path"], "/tmp/final.txt");
    }
}
