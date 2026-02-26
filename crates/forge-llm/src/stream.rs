//! SSE (Server-Sent Events) parsing and LLM event types

use crate::{LlmError, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// SSE event parsed from stream
#[derive(Debug, Clone)]
pub struct SseEvent {
    /// Event type (e.g., "message_start", "content_block_delta")
    pub event_type: Option<String>,
    /// Event data (JSON string)
    pub data: String,
}

/// SSE stream processor
pub struct SseProcessor {
    buffer: String,
}

impl SseProcessor {
    /// Create a new SSE processor
    pub fn new() -> Self {
        Self { buffer: String::new() }
    }

    /// Process a chunk of data and return parsed events
    pub fn process_chunk(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(chunk);

        let mut events = Vec::new();

        // Split by double newline (event separator)
        while let Some(pos) = self.buffer.find("\n\n") {
            let event_str = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();

            if let Some(event) = self.parse_event(&event_str) {
                events.push(event);
            }
        }

        events
    }

    fn parse_event(&self, event_str: &str) -> Option<SseEvent> {
        let mut event_type = None;
        let mut data = String::new();

        for line in event_str.lines() {
            if let Some(value) = line.strip_prefix("event: ") {
                event_type = Some(value.to_string());
            } else if let Some(value) = line.strip_prefix("data: ") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(value);
            }
        }

        if data.is_empty() {
            return None;
        }

        Some(SseEvent { event_type, data })
    }
}

impl Default for SseProcessor {
    fn default() -> Self {
        Self::new()
    }
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens
    pub input_tokens: usize,
    /// Output tokens
    pub output_tokens: usize,
    /// Cache read tokens (Anthropic prompt caching)
    #[serde(default)]
    pub cache_read_input_tokens: Option<usize>,
    /// Cache creation tokens
    #[serde(default)]
    pub cache_creation_input_tokens: Option<usize>,
}

impl Usage {
    /// Total tokens used
    pub fn total(&self) -> usize {
        self.input_tokens + self.output_tokens
    }
}

/// Stop reason for message completion
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Normal end of turn
    EndTurn,
    /// Max tokens reached
    MaxTokens,
    /// Stop sequence encountered
    StopSequence,
    /// Tool use requested
    ToolUse,
}

/// Tool call parser for incremental JSON parsing
pub struct ToolCallParser {
    tool_id: String,
    tool_name: String,
    input_buffer: String,
}

impl ToolCallParser {
    /// Create a new tool call parser
    pub fn new(id: String, name: String) -> Self {
        Self { tool_id: id, tool_name: name, input_buffer: String::new() }
    }

    /// Get the tool ID
    pub fn id(&self) -> &str {
        &self.tool_id
    }

    /// Get the tool name
    pub fn name(&self) -> &str {
        &self.tool_name
    }

    /// Append input delta
    pub fn append(&mut self, delta: &str) {
        self.input_buffer.push_str(delta);
    }

    /// Try to parse the accumulated JSON
    pub fn try_parse(&self) -> Option<Value> {
        serde_json::from_str(&self.input_buffer).ok()
    }

    /// Finish parsing and return the complete tool call
    pub fn finish(self) -> Result<ToolCall> {
        let input = serde_json::from_str(&self.input_buffer)
            .map_err(|e| LlmError::ParseError(format!("Invalid tool input JSON: {e}")))?;

        Ok(ToolCall { id: self.tool_id, name: self.tool_name, input })
    }
}

/// Parsed tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Tool call ID
    pub id: String,
    /// Tool name
    pub name: String,
    /// Tool input parameters
    pub input: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sse_processor() {
        let mut processor = SseProcessor::new();

        let chunk1 = "event: message_start\ndata: {\"type\":\"message\"}\n\n";
        let events = processor.process_chunk(chunk1);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, Some("message_start".to_string()));

        let chunk2 = "event: content";
        let events = processor.process_chunk(chunk2);
        assert!(events.is_empty()); // Incomplete event

        let chunk3 = "_block_delta\ndata: {\"delta\":\"hello\"}\n\n";
        let events = processor.process_chunk(chunk3);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn test_tool_call_parser() {
        let mut parser = ToolCallParser::new("id_123".into(), "read".into());
        parser.append("{\"path\":");
        assert!(parser.try_parse().is_none());

        parser.append("\"/tmp/test.txt\"}");
        let parsed = parser.try_parse();
        assert!(parsed.is_some());

        let tool_call = parser.finish().expect("finish");
        assert_eq!(tool_call.name, "read");
        assert_eq!(tool_call.input["path"], "/tmp/test.txt");
    }
}
