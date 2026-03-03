//! Trace recording for deterministic replay.
//!
//! Records every LLM request, response, tool call, and sub-agent invocation
//! during an agent run. The recorded trace can be serialized to disk and
//! replayed later for debugging and regression testing.

use chrono::Utc;
use forge_domain::trace::{
    AgentTrace, TraceRequest, TraceResponse, TraceRound, TraceToolCall, TraceToolUse, TraceUsage,
};
use forge_domain::{ToolCall, ToolResult};
use forge_llm::LlmEvent;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Records the execution trace of an agent run.
///
/// Created at agent startup and finalized when the agent completes.
/// If the `trace_recording` experimental flag is disabled, all methods
/// are no-ops (the recorder is created but `enabled` is `false`).
pub struct TraceRecorder {
    /// The trace being recorded.
    trace: AgentTrace,
    /// Current round being recorded.
    current_round: Option<InProgressRound>,
    /// Whether recording is active.
    enabled: bool,
    /// Output directory for trace files.
    output_dir: PathBuf,
}

/// An in-progress round being assembled.
struct InProgressRound {
    round: usize,
    request: Option<TraceRequest>,
    text: String,
    tool_uses: Vec<TraceToolUse>,
    raw_events: Vec<Value>,
    usage: TraceUsage,
    tool_calls: Vec<TraceToolCall>,
}

impl TraceRecorder {
    /// Create a new recorder.
    ///
    /// If `enabled` is false, all recording methods are no-ops.
    #[must_use]
    pub fn new(
        session_id: Option<&str>,
        agent_type: &str,
        model: &str,
        enabled: bool,
    ) -> Self {
        let trace_id = uuid::Uuid::new_v4().to_string();
        let output_dir = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".forge")
            .join("traces");

        Self {
            trace: AgentTrace {
                trace_id,
                session_id: session_id.map(String::from),
                agent_type: agent_type.to_string(),
                model: model.to_string(),
                started_at: Utc::now(),
                completed_at: None,
                rounds: Vec::new(),
                total_usage: TraceUsage::default(),
                sub_traces: Vec::new(),
            },
            current_round: None,
            enabled,
            output_dir,
        }
    }

    /// Get the trace ID.
    #[must_use]
    pub fn trace_id(&self) -> &str {
        &self.trace.trace_id
    }

    /// Whether recording is enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Start recording a new round.
    pub fn begin_round(
        &mut self,
        round: usize,
        messages_json: &str,
        messages_count: usize,
        tools_count: usize,
        model: &str,
        max_tokens: usize,
        temperature: f64,
    ) {
        if !self.enabled {
            return;
        }

        // Compute messages hash
        let mut hasher = Sha256::new();
        hasher.update(messages_json.as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        let request = TraceRequest {
            messages_hash: hash,
            messages_count,
            tools_count,
            model: model.to_string(),
            max_tokens,
            temperature,
        };

        self.current_round = Some(InProgressRound {
            round,
            request: Some(request),
            text: String::new(),
            tool_uses: Vec::new(),
            raw_events: Vec::new(),
            usage: TraceUsage::default(),
            tool_calls: Vec::new(),
        });
    }

    /// Record an LLM event within the current round.
    pub fn record_llm_event(&mut self, event: &LlmEvent) {
        if !self.enabled {
            return;
        }

        let Some(ref mut round) = self.current_round else {
            return;
        };

        // Serialize to JSON for storage
        let event_json = match event {
            LlmEvent::TextDelta(delta) => {
                round.text.push_str(delta);
                serde_json::json!({"type": "text_delta", "delta": delta})
            }
            LlmEvent::ToolUseStart { id, name } => {
                serde_json::json!({"type": "tool_use_start", "id": id, "name": name})
            }
            LlmEvent::ToolUseInputDelta { id, delta } => {
                serde_json::json!({"type": "tool_use_input_delta", "id": id, "delta": delta})
            }
            LlmEvent::ToolUseEnd { id, name, input } => {
                round.tool_uses.push(TraceToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
                serde_json::json!({"type": "tool_use_end", "id": id, "name": name, "input": input})
            }
            LlmEvent::MessageEnd { usage } => {
                round.usage.input_tokens += usage.input_tokens;
                round.usage.output_tokens += usage.output_tokens;
                if let Some(cr) = usage.cache_read_input_tokens {
                    round.usage.cache_read_tokens += cr;
                }
                if let Some(cw) = usage.cache_creation_input_tokens {
                    round.usage.cache_write_tokens += cw;
                }
                serde_json::json!({
                    "type": "message_end",
                    "input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                    "cache_read_input_tokens": usage.cache_read_input_tokens,
                    "cache_creation_input_tokens": usage.cache_creation_input_tokens
                })
            }
            LlmEvent::ThinkingStart => {
                serde_json::json!({"type": "thinking_start"})
            }
            LlmEvent::ThinkingDelta(content) => {
                serde_json::json!({"type": "thinking_delta", "content": content})
            }
            LlmEvent::ThinkingEnd => {
                serde_json::json!({"type": "thinking_end"})
            }
            LlmEvent::Error(msg) => {
                serde_json::json!({"type": "error", "message": msg})
            }
        };

        round.raw_events.push(event_json);
    }

    /// Record a tool call result within the current round.
    pub fn record_tool_call(
        &mut self,
        call: &ToolCall,
        result: &ToolResult,
        duration_ms: u64,
    ) {
        if !self.enabled {
            return;
        }

        let Some(ref mut round) = self.current_round else {
            return;
        };

        round.tool_calls.push(TraceToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
            output: result.output.clone(),
            is_error: result.is_error,
            duration_ms,
        });
    }

    /// End the current round and add it to the trace.
    pub fn end_round(&mut self) {
        if !self.enabled {
            return;
        }

        let Some(round) = self.current_round.take() else {
            return;
        };

        let trace_round = TraceRound {
            round: round.round,
            request: round.request.unwrap_or(TraceRequest {
                messages_hash: String::new(),
                messages_count: 0,
                tools_count: 0,
                model: String::new(),
                max_tokens: 0,
                temperature: 0.0,
            }),
            response: TraceResponse {
                text: round.text,
                tool_uses: round.tool_uses,
                usage: round.usage.clone(),
                raw_events: round.raw_events,
            },
            tool_calls: round.tool_calls,
        };

        // Accumulate into total usage
        self.trace.total_usage.add(&round.usage);
        self.trace.rounds.push(trace_round);
    }

    /// Add a sub-agent trace.
    pub fn add_sub_trace(&mut self, sub_trace: AgentTrace) {
        if !self.enabled {
            return;
        }
        self.trace.sub_traces.push(sub_trace);
    }

    /// Finalize the trace and write it to disk.
    ///
    /// Returns the path to the trace file on success.
    ///
    /// # Errors
    ///
    /// Returns an error if the trace directory cannot be created or
    /// the trace file cannot be written.
    pub async fn finalize(&mut self) -> std::result::Result<PathBuf, String> {
        if !self.enabled {
            return Err("Trace recording is disabled".to_string());
        }

        // End any in-progress round
        if self.current_round.is_some() {
            self.end_round();
        }

        self.trace.completed_at = Some(Utc::now());

        // Write to disk
        tokio::fs::create_dir_all(&self.output_dir)
            .await
            .map_err(|e| format!("Failed to create trace directory: {e}"))?;

        let filename = format!("{}.json", self.trace.trace_id);
        let path = self.output_dir.join(&filename);
        let json = serde_json::to_vec_pretty(&self.trace)
            .map_err(|e| format!("Failed to serialize trace: {e}"))?;

        tokio::fs::write(&path, json)
            .await
            .map_err(|e| format!("Failed to write trace file: {e}"))?;

        tracing::info!(
            trace_id = %self.trace.trace_id,
            rounds = self.trace.rounds.len(),
            path = %path.display(),
            "Trace finalized"
        );

        Ok(path)
    }

    /// Consume the recorder and return the assembled trace without writing to disk.
    ///
    /// Useful for sub-agent traces that will be embedded in a parent trace.
    #[must_use]
    pub fn into_trace(mut self) -> AgentTrace {
        // End any in-progress round
        if self.current_round.is_some() {
            self.end_round();
        }
        self.trace.completed_at = Some(Utc::now());
        self.trace
    }

    /// Load a trace from a file on disk.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub async fn load(path: &Path) -> std::result::Result<AgentTrace, String> {
        let data = tokio::fs::read(path)
            .await
            .map_err(|e| format!("Failed to read trace file: {e}"))?;
        serde_json::from_slice(&data).map_err(|e| format!("Failed to parse trace: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use forge_llm::Usage;

    #[test]
    fn test_new_recorder() {
        let rec = TraceRecorder::new(Some("sess-1"), "explore", "claude-sonnet-4", true);
        assert!(rec.is_enabled());
        assert!(!rec.trace_id().is_empty());
        assert_eq!(rec.trace.agent_type, "explore");
    }

    #[test]
    fn test_disabled_recorder_noop() {
        let mut rec = TraceRecorder::new(None, "main", "test", false);
        rec.begin_round(0, "{}", 1, 0, "test", 8192, 0.7);
        rec.record_llm_event(&LlmEvent::TextDelta("hello".to_string()));
        rec.end_round();
        // No rounds recorded
        assert!(rec.trace.rounds.is_empty());
    }

    #[test]
    fn test_record_full_round() {
        let mut rec = TraceRecorder::new(Some("s1"), "explore", "claude-sonnet-4", true);

        rec.begin_round(
            0,
            r#"[{"role":"user","content":"hello"}]"#,
            1,
            3,
            "claude-sonnet-4",
            8192,
            0.3,
        );

        rec.record_llm_event(&LlmEvent::TextDelta("Found ".to_string()));
        rec.record_llm_event(&LlmEvent::TextDelta("5 files.".to_string()));
        rec.record_llm_event(&LlmEvent::ToolUseStart {
            id: "tc_1".to_string(),
            name: "glob".to_string(),
        });
        rec.record_llm_event(&LlmEvent::ToolUseEnd {
            id: "tc_1".to_string(),
            name: "glob".to_string(),
            input: serde_json::json!({"pattern": "*.rs"}),
        });
        rec.record_llm_event(&LlmEvent::MessageEnd {
            usage: Usage {
                input_tokens: 500,
                output_tokens: 100,
                ..Default::default()
            },
        });

        let call = ToolCall {
            id: "tc_1".to_string(),
            name: "glob".to_string(),
            input: serde_json::json!({"pattern": "*.rs"}),
        };
        let result = ToolResult::success("tc_1", "src/main.rs\nsrc/lib.rs");
        rec.record_tool_call(&call, &result, 15);

        rec.end_round();

        assert_eq!(rec.trace.rounds.len(), 1);
        let round = &rec.trace.rounds[0];
        assert_eq!(round.round, 0);
        assert_eq!(round.response.text, "Found 5 files.");
        assert_eq!(round.response.tool_uses.len(), 1);
        assert_eq!(round.response.raw_events.len(), 5);
        assert_eq!(round.tool_calls.len(), 1);
        assert_eq!(round.tool_calls[0].duration_ms, 15);
        assert_eq!(round.request.messages_count, 1);
        assert_eq!(round.request.tools_count, 3);

        // Total usage accumulated
        assert_eq!(rec.trace.total_usage.input_tokens, 500);
        assert_eq!(rec.trace.total_usage.output_tokens, 100);
    }

    #[test]
    fn test_into_trace() {
        let mut rec = TraceRecorder::new(None, "plan", "test", true);
        rec.begin_round(0, "{}", 1, 0, "test", 4096, 0.5);
        rec.record_llm_event(&LlmEvent::TextDelta("plan step 1".to_string()));
        // Don't explicitly end_round — into_trace should handle it
        let trace = rec.into_trace();
        assert_eq!(trace.rounds.len(), 1);
        assert!(trace.completed_at.is_some());
    }

    #[test]
    fn test_add_sub_trace() {
        let mut parent = TraceRecorder::new(None, "main", "test", true);
        let child = TraceRecorder::new(None, "explore", "test", true);
        parent.add_sub_trace(child.into_trace());
        assert_eq!(parent.trace.sub_traces.len(), 1);
        assert_eq!(parent.trace.sub_traces[0].agent_type, "explore");
    }

    #[tokio::test]
    async fn test_finalize_writes_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut rec = TraceRecorder::new(None, "explore", "test-model", true);
        rec.output_dir = tmp.path().to_path_buf();

        rec.begin_round(0, "{}", 1, 0, "test-model", 8192, 0.7);
        rec.record_llm_event(&LlmEvent::TextDelta("hello".to_string()));
        rec.end_round();

        let path = rec.finalize().await.expect("finalize should succeed");
        assert!(path.exists());

        // Load back
        let loaded = TraceRecorder::load(&path).await.expect("load");
        assert_eq!(loaded.rounds.len(), 1);
        assert_eq!(loaded.rounds[0].response.text, "hello");
    }
}
