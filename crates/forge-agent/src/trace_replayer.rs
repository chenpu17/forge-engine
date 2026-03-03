//! Trace replay for deterministic re-execution.
//!
//! Provides [`ReplayProvider`] (an `LlmProvider` implementation that returns
//! pre-recorded LLM events) and [`replay_trace`] (a standalone function that
//! replays a trace and emits events through a channel).

use async_trait::async_trait;
use forge_domain::trace::{AgentTrace, TraceToolCall};
use forge_llm::{
    ChatMessage, LlmConfig, LlmError, LlmEvent, LlmEventStream, LlmProvider, ModelInfo,
    Result as LlmResult, ToolDef, Usage,
};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::trace_recorder::TraceRecorder;

// ---------------------------------------------------------------------------
// ReplayProvider
// ---------------------------------------------------------------------------

/// An [`LlmProvider`] that replays pre-recorded LLM responses.
///
/// Each call to [`chat_stream`] returns the raw events from the next trace
/// round, advancing an internal round counter. This allows the replay provider
/// to be used as a drop-in replacement for a real LLM provider when running
/// through the agent loop.
pub struct ReplayProvider {
    /// The trace being replayed.
    trace: AgentTrace,
    /// Current round index (atomically incremented on each `chat_stream` call).
    round_index: AtomicUsize,
}

impl ReplayProvider {
    /// Create a replay provider from a loaded trace.
    #[must_use]
    pub fn new(trace: AgentTrace) -> Self {
        Self { trace, round_index: AtomicUsize::new(0) }
    }

    /// Load a trace from disk and create a replay provider.
    ///
    /// # Errors
    ///
    /// Returns an error if the trace file cannot be read or parsed.
    pub async fn from_file(path: &Path) -> std::result::Result<Self, String> {
        let trace = TraceRecorder::load(path).await?;
        Ok(Self::new(trace))
    }

    /// Get the trace being replayed.
    #[must_use]
    pub fn trace(&self) -> &AgentTrace {
        &self.trace
    }

    /// Get the current round index.
    #[must_use]
    pub fn current_round(&self) -> usize {
        self.round_index.load(Ordering::SeqCst)
    }

    /// Reset the round counter to replay from the beginning.
    pub fn reset(&self) {
        self.round_index.store(0, Ordering::SeqCst);
    }

    /// Total number of rounds in the trace.
    #[must_use]
    pub fn total_rounds(&self) -> usize {
        self.trace.rounds.len()
    }
}

#[async_trait]
impl LlmProvider for ReplayProvider {
    fn id(&self) -> &str {
        "replay"
    }

    fn name(&self) -> &str {
        "Replay Provider"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo::new(&self.trace.model, "Replay Model", 200_000)]
    }

    async fn chat_stream(
        &self,
        _messages: &[ChatMessage],
        _tools: Vec<ToolDef>,
        _config: &LlmConfig,
    ) -> LlmResult<LlmEventStream> {
        let idx = self.round_index.fetch_add(1, Ordering::SeqCst);
        let round = self.trace.rounds.get(idx).ok_or_else(|| {
            LlmError::ConfigError(format!(
                "Replay exhausted: requested round {idx} but trace only has {} rounds",
                self.trace.rounds.len()
            ))
        })?;

        let events = parse_raw_events(&round.response.raw_events);
        let stream = futures::stream::iter(events);
        Ok(Box::pin(stream))
    }
}

// ---------------------------------------------------------------------------
// Standalone replay
// ---------------------------------------------------------------------------

/// Replay a trace, emitting events through a channel.
///
/// This function does not use the full agent loop; it simply iterates through
/// the recorded rounds and re-emits the LLM events and tool call results.
/// Sub-agent traces (if any) are replayed recursively after each round's tool
/// results, wrapped in `SubTraceStart`/`SubTraceEnd` events.
///
/// Useful for offline analysis, UI previews, and trace validation.
pub async fn replay_trace(
    trace: &AgentTrace,
    tx: &tokio::sync::mpsc::Sender<ReplayEvent>,
) -> std::result::Result<(), String> {
    replay_trace_inner(trace, tx).await
}

/// Recursive inner implementation for trace replay.
async fn replay_trace_inner(
    trace: &AgentTrace,
    tx: &tokio::sync::mpsc::Sender<ReplayEvent>,
) -> std::result::Result<(), String> {
    for round in &trace.rounds {
        // Emit LLM events
        for raw in &round.response.raw_events {
            if let Some(event) = parse_single_raw_event(raw) {
                tx.send(ReplayEvent::Llm(event))
                    .await
                    .map_err(|e| format!("Channel send error: {e}"))?;
            }
        }

        // Emit tool call results
        for tc in &round.tool_calls {
            tx.send(ReplayEvent::ToolResult(tc.clone()))
                .await
                .map_err(|e| format!("Channel send error: {e}"))?;
        }

        tx.send(ReplayEvent::RoundEnd { round: round.round })
            .await
            .map_err(|e| format!("Channel send error: {e}"))?;
    }

    // Recursively replay sub-agent traces
    for sub in &trace.sub_traces {
        tx.send(ReplayEvent::SubTraceStart {
            trace_id: sub.trace_id.clone(),
            agent_type: sub.agent_type.clone(),
        })
        .await
        .map_err(|e| format!("Channel send error: {e}"))?;

        Box::pin(replay_trace_inner(sub, tx)).await?;

        tx.send(ReplayEvent::SubTraceEnd { trace_id: sub.trace_id.clone() })
            .await
            .map_err(|e| format!("Channel send error: {e}"))?;
    }

    Ok(())
}

/// Events emitted during trace replay.
#[derive(Debug, Clone)]
pub enum ReplayEvent {
    /// An LLM event from the recorded stream.
    Llm(LlmEvent),
    /// A tool call result from the recorded execution.
    ToolResult(TraceToolCall),
    /// End of a replay round.
    RoundEnd {
        /// Round number.
        round: usize,
    },
    /// A sub-agent trace is starting.
    SubTraceStart {
        /// Trace ID of the sub-agent.
        trace_id: String,
        /// Agent type of the sub-agent (e.g. "explore", "plan").
        agent_type: String,
    },
    /// A sub-agent trace has ended.
    SubTraceEnd {
        /// Trace ID of the sub-agent.
        trace_id: String,
    },
}

// ---------------------------------------------------------------------------
// Trace comparison
// ---------------------------------------------------------------------------

/// Result of comparing two traces.
#[derive(Debug)]
pub struct TraceDiff {
    /// Number of rounds in trace A.
    pub rounds_a: usize,
    /// Number of rounds in trace B.
    pub rounds_b: usize,
    /// Per-round differences.
    pub round_diffs: Vec<RoundDiff>,
    /// Differences in sub-agent traces.
    pub sub_trace_diffs: Vec<SubTraceDiff>,
}

/// A difference in sub-agent traces between two parent traces.
#[derive(Debug)]
pub struct SubTraceDiff {
    /// Index of the sub-trace.
    pub index: usize,
    /// Agent type (from trace A if available, else trace B).
    pub agent_type: String,
    /// The recursive diff of the sub-trace.
    pub diff: TraceDiff,
}

/// Differences found in a single round.
#[derive(Debug)]
pub struct RoundDiff {
    /// Round number.
    pub round: usize,
    /// Text output differs.
    pub text_differs: bool,
    /// Tool use count differs.
    pub tool_use_count_differs: bool,
    /// Tool call results differ.
    pub tool_result_diffs: Vec<ToolResultDiff>,
}

/// A difference in a tool call result.
#[derive(Debug)]
pub struct ToolResultDiff {
    /// Tool call ID.
    pub tool_id: String,
    /// Tool name.
    pub tool_name: String,
    /// What kind of difference was found.
    pub kind: ToolDiffKind,
}

/// Kind of tool result difference.
#[derive(Debug)]
pub enum ToolDiffKind {
    /// Tool call only exists in trace A.
    OnlyInA,
    /// Tool call only exists in trace B.
    OnlyInB,
    /// Output text differs.
    OutputDiffers {
        /// Output in trace A.
        a: String,
        /// Output in trace B.
        b: String,
    },
    /// Error status differs.
    ErrorStatusDiffers {
        /// Error in trace A.
        a: bool,
        /// Error in trace B.
        b: bool,
    },
}

/// Compare two traces and return a summary of differences.
///
/// This is useful for regression testing: record a trace, make changes to
/// tools or prompts, record again, and diff the results.
#[must_use]
pub fn diff_traces(a: &AgentTrace, b: &AgentTrace) -> TraceDiff {
    let max_rounds = a.rounds.len().max(b.rounds.len());
    let mut round_diffs = Vec::new();

    for i in 0..max_rounds {
        let round_a = a.rounds.get(i);
        let round_b = b.rounds.get(i);

        match (round_a, round_b) {
            (Some(ra), Some(rb)) => {
                let text_differs = ra.response.text != rb.response.text;
                let tool_use_count_differs =
                    ra.response.tool_uses.len() != rb.response.tool_uses.len();

                let tool_result_diffs = diff_tool_calls(&ra.tool_calls, &rb.tool_calls);

                if text_differs || tool_use_count_differs || !tool_result_diffs.is_empty() {
                    round_diffs.push(RoundDiff {
                        round: i,
                        text_differs,
                        tool_use_count_differs,
                        tool_result_diffs,
                    });
                }
            }
            (Some(_), None) => {
                round_diffs.push(RoundDiff {
                    round: i,
                    text_differs: true,
                    tool_use_count_differs: true,
                    tool_result_diffs: vec![],
                });
            }
            (None, Some(_)) => {
                round_diffs.push(RoundDiff {
                    round: i,
                    text_differs: true,
                    tool_use_count_differs: true,
                    tool_result_diffs: vec![],
                });
            }
            (None, None) => break,
        }
    }

    TraceDiff {
        rounds_a: a.rounds.len(),
        rounds_b: b.rounds.len(),
        round_diffs,
        sub_trace_diffs: diff_sub_traces(&a.sub_traces, &b.sub_traces),
    }
}

/// Compare sub-traces from two parent traces.
fn diff_sub_traces(a: &[AgentTrace], b: &[AgentTrace]) -> Vec<SubTraceDiff> {
    let max = a.len().max(b.len());
    let mut diffs = Vec::new();

    for i in 0..max {
        let sub_a = a.get(i);
        let sub_b = b.get(i);

        match (sub_a, sub_b) {
            (Some(sa), Some(sb)) => {
                let sub_diff = diff_traces(sa, sb);
                if !sub_diff.is_identical() {
                    diffs.push(SubTraceDiff {
                        index: i,
                        agent_type: sa.agent_type.clone(),
                        diff: sub_diff,
                    });
                }
            }
            (Some(sa), None) | (None, Some(sa)) => {
                // One side has a sub-trace, the other doesn't — always a diff
                diffs.push(SubTraceDiff {
                    index: i,
                    agent_type: sa.agent_type.clone(),
                    diff: TraceDiff {
                        rounds_a: if sub_a.is_some() { sa.rounds.len() } else { 0 },
                        rounds_b: if sub_b.is_some() { sa.rounds.len() } else { 0 },
                        round_diffs: vec![],
                        sub_trace_diffs: vec![],
                    },
                });
            }
            (None, None) => break,
        }
    }

    diffs
}

impl TraceDiff {
    /// Whether the traces are considered identical (no differences found).
    #[must_use]
    pub fn is_identical(&self) -> bool {
        self.rounds_a == self.rounds_b
            && self.round_diffs.is_empty()
            && self.sub_trace_diffs.is_empty()
    }

    /// Total number of rounds with differences.
    #[must_use]
    pub fn changed_rounds(&self) -> usize {
        self.round_diffs.len()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse raw JSON events back into [`LlmEvent`] variants.
fn parse_raw_events(raw: &[serde_json::Value]) -> Vec<LlmResult<LlmEvent>> {
    raw.iter().filter_map(|v| parse_single_raw_event(v).map(Ok)).collect()
}

/// Parse a single raw JSON event into an [`LlmEvent`].
fn parse_single_raw_event(v: &serde_json::Value) -> Option<LlmEvent> {
    let event_type = v.get("type")?.as_str()?;
    match event_type {
        "text_delta" => {
            let delta = v.get("delta")?.as_str()?.to_string();
            Some(LlmEvent::TextDelta(delta))
        }
        "thinking_start" => Some(LlmEvent::ThinkingStart),
        "thinking_delta" => {
            let content = v.get("content")?.as_str()?.to_string();
            Some(LlmEvent::ThinkingDelta(content))
        }
        "thinking_end" => Some(LlmEvent::ThinkingEnd),
        "tool_use_start" => {
            let id = v.get("id")?.as_str()?.to_string();
            let name = v.get("name")?.as_str()?.to_string();
            Some(LlmEvent::ToolUseStart { id, name })
        }
        "tool_use_input_delta" => {
            let id = v.get("id")?.as_str()?.to_string();
            let delta = v.get("delta")?.as_str()?.to_string();
            Some(LlmEvent::ToolUseInputDelta { id, delta })
        }
        "tool_use_end" => {
            let id = v.get("id")?.as_str()?.to_string();
            let name = v.get("name")?.as_str()?.to_string();
            let input = v.get("input").cloned().unwrap_or(serde_json::Value::Null);
            Some(LlmEvent::ToolUseEnd { id, name, input })
        }
        "message_end" => {
            let input_tokens = v.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let output_tokens =
                v.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            Some(LlmEvent::MessageEnd {
                usage: Usage {
                    input_tokens,
                    output_tokens,
                    cache_read_input_tokens: v
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize),
                    cache_creation_input_tokens: v
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize),
                },
            })
        }
        "error" => {
            let message = v.get("message")?.as_str()?.to_string();
            Some(LlmEvent::Error(message))
        }
        _ => {
            tracing::debug!(event_type, "Unknown event type in trace replay, skipping");
            None
        }
    }
}

/// Compare two lists of tool calls and return differences.
fn diff_tool_calls(a: &[TraceToolCall], b: &[TraceToolCall]) -> Vec<ToolResultDiff> {
    let mut diffs = Vec::new();
    let max = a.len().max(b.len());

    for i in 0..max {
        match (a.get(i), b.get(i)) {
            (Some(ta), Some(tb)) => {
                if ta.output != tb.output {
                    diffs.push(ToolResultDiff {
                        tool_id: ta.id.clone(),
                        tool_name: ta.name.clone(),
                        kind: ToolDiffKind::OutputDiffers {
                            a: ta.output.clone(),
                            b: tb.output.clone(),
                        },
                    });
                } else if ta.is_error != tb.is_error {
                    diffs.push(ToolResultDiff {
                        tool_id: ta.id.clone(),
                        tool_name: ta.name.clone(),
                        kind: ToolDiffKind::ErrorStatusDiffers { a: ta.is_error, b: tb.is_error },
                    });
                }
            }
            (Some(ta), None) => {
                diffs.push(ToolResultDiff {
                    tool_id: ta.id.clone(),
                    tool_name: ta.name.clone(),
                    kind: ToolDiffKind::OnlyInA,
                });
            }
            (None, Some(tb)) => {
                diffs.push(ToolResultDiff {
                    tool_id: tb.id.clone(),
                    tool_name: tb.name.clone(),
                    kind: ToolDiffKind::OnlyInB,
                });
            }
            (None, None) => break,
        }
    }

    diffs
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use forge_domain::trace::{
        TraceRequest, TraceResponse, TraceRound, TraceToolUse, TraceUsage,
    };
    use serde_json::json;

    fn make_test_trace(rounds: Vec<TraceRound>) -> AgentTrace {
        AgentTrace {
            trace_id: "test-trace".to_string(),
            session_id: None,
            agent_type: "test".to_string(),
            model: "test-model".to_string(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            rounds,
            total_usage: TraceUsage::default(),
            sub_traces: vec![],
        }
    }

    fn make_round(round_num: usize, text: &str, tool_calls: Vec<TraceToolCall>) -> TraceRound {
        let mut raw_events = vec![
            json!({"type": "text_delta", "delta": text}),
            json!({"type": "message_end", "input_tokens": 100, "output_tokens": 50}),
        ];
        // Add tool use events if there are tool calls
        for tc in &tool_calls {
            raw_events.insert(
                1,
                json!({
                    "type": "tool_use_end",
                    "id": tc.id,
                    "name": tc.name,
                    "input": tc.input
                }),
            );
        }

        TraceRound {
            round: round_num,
            request: TraceRequest {
                messages_hash: "abc".to_string(),
                messages_count: 1,
                tools_count: 0,
                model: "test-model".to_string(),
                max_tokens: 4096,
                temperature: 0.0,
            },
            response: TraceResponse {
                text: text.to_string(),
                tool_uses: tool_calls
                    .iter()
                    .map(|tc| TraceToolUse {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input: tc.input.clone(),
                    })
                    .collect(),
                usage: TraceUsage::default(),
                raw_events,
            },
            tool_calls,
        }
    }

    fn make_tool_call(id: &str, name: &str, output: &str) -> TraceToolCall {
        TraceToolCall {
            id: id.to_string(),
            name: name.to_string(),
            input: json!({}),
            output: output.to_string(),
            is_error: false,
            duration_ms: 10,
        }
    }

    #[test]
    fn test_parse_raw_events_text_delta() {
        let raw = vec![json!({"type": "text_delta", "delta": "hello"})];
        let events = parse_raw_events(&raw);
        assert_eq!(events.len(), 1);
        match events[0].as_ref().expect("ok") {
            LlmEvent::TextDelta(d) => assert_eq!(d, "hello"),
            other => panic!("Expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_raw_events_tool_use() {
        let raw = vec![
            json!({"type": "tool_use_start", "id": "tc_1", "name": "glob"}),
            json!({"type": "tool_use_input_delta", "id": "tc_1", "delta": "{\"pat\":"}),
            json!({"type": "tool_use_end", "id": "tc_1", "name": "glob", "input": {"pattern": "*.rs"}}),
        ];
        let events = parse_raw_events(&raw);
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn test_parse_raw_events_thinking() {
        let raw = vec![
            json!({"type": "thinking_start"}),
            json!({"type": "thinking_delta", "content": "Let me think..."}),
            json!({"type": "thinking_end"}),
        ];
        let events = parse_raw_events(&raw);
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0].as_ref().expect("ok"), LlmEvent::ThinkingStart));
    }

    #[test]
    fn test_parse_raw_events_message_end() {
        let raw = vec![json!({"type": "message_end", "input_tokens": 500, "output_tokens": 100})];
        let events = parse_raw_events(&raw);
        assert_eq!(events.len(), 1);
        match events[0].as_ref().expect("ok") {
            LlmEvent::MessageEnd { usage } => {
                assert_eq!(usage.input_tokens, 500);
                assert_eq!(usage.output_tokens, 100);
            }
            other => panic!("Expected MessageEnd, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_raw_events_unknown_type_skipped() {
        let raw = vec![
            json!({"type": "text_delta", "delta": "hi"}),
            json!({"type": "future_event_type", "data": 42}),
        ];
        let events = parse_raw_events(&raw);
        assert_eq!(events.len(), 1); // Unknown type is skipped
    }

    #[tokio::test]
    async fn test_replay_provider_basic() {
        let trace = make_test_trace(vec![
            make_round(0, "Hello", vec![]),
            make_round(1, "World", vec![]),
        ]);
        let provider = ReplayProvider::new(trace);

        assert_eq!(provider.id(), "replay");
        assert_eq!(provider.total_rounds(), 2);
        assert_eq!(provider.current_round(), 0);

        // First call returns round 0 events
        let stream = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(stream.is_ok());
        assert_eq!(provider.current_round(), 1);

        // Second call returns round 1 events
        let stream = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(stream.is_ok());
        assert_eq!(provider.current_round(), 2);

        // Third call should fail (exhausted)
        let stream = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(stream.is_err());
    }

    #[tokio::test]
    async fn test_replay_provider_reset() {
        let trace = make_test_trace(vec![make_round(0, "Hello", vec![])]);
        let provider = ReplayProvider::new(trace);

        let _ = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert_eq!(provider.current_round(), 1);

        provider.reset();
        assert_eq!(provider.current_round(), 0);

        // Can replay again
        let stream = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(stream.is_ok());
    }

    #[tokio::test]
    async fn test_replay_trace_emits_events() {
        let trace = make_test_trace(vec![
            make_round(0, "Hello", vec![make_tool_call("tc_1", "glob", "*.rs")]),
        ]);

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        replay_trace(&trace, &tx).await.expect("replay should succeed");
        drop(tx);

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        // Should have: text_delta + tool_use_end + message_end + tool_result + round_end
        assert!(events.len() >= 3);
        assert!(matches!(events.last(), Some(ReplayEvent::RoundEnd { round: 0 })));
    }

    #[test]
    fn test_diff_identical_traces() {
        let trace = make_test_trace(vec![
            make_round(0, "Hello", vec![make_tool_call("tc_1", "glob", "*.rs")]),
        ]);
        let diff = diff_traces(&trace, &trace);
        assert!(diff.is_identical());
        assert_eq!(diff.changed_rounds(), 0);
    }

    #[test]
    fn test_diff_different_text() {
        let a = make_test_trace(vec![make_round(0, "Hello", vec![])]);
        let b = make_test_trace(vec![make_round(0, "World", vec![])]);
        let diff = diff_traces(&a, &b);
        assert!(!diff.is_identical());
        assert_eq!(diff.changed_rounds(), 1);
        assert!(diff.round_diffs[0].text_differs);
    }

    #[test]
    fn test_diff_different_round_count() {
        let a = make_test_trace(vec![make_round(0, "A", vec![]), make_round(1, "B", vec![])]);
        let b = make_test_trace(vec![make_round(0, "A", vec![])]);
        let diff = diff_traces(&a, &b);
        assert!(!diff.is_identical());
        assert_eq!(diff.rounds_a, 2);
        assert_eq!(diff.rounds_b, 1);
    }

    #[test]
    fn test_diff_tool_output_differs() {
        let a = make_test_trace(vec![make_round(
            0,
            "test",
            vec![make_tool_call("tc_1", "glob", "file_a.rs")],
        )]);
        let b = make_test_trace(vec![make_round(
            0,
            "test",
            vec![make_tool_call("tc_1", "glob", "file_b.rs")],
        )]);
        let diff = diff_traces(&a, &b);
        assert!(!diff.is_identical());
        assert_eq!(diff.round_diffs.len(), 1);
        assert_eq!(diff.round_diffs[0].tool_result_diffs.len(), 1);
        assert!(matches!(
            diff.round_diffs[0].tool_result_diffs[0].kind,
            ToolDiffKind::OutputDiffers { .. }
        ));
    }

    #[test]
    fn test_diff_tool_count_mismatch() {
        let a = make_test_trace(vec![make_round(
            0,
            "test",
            vec![
                make_tool_call("tc_1", "glob", "*.rs"),
                make_tool_call("tc_2", "read", "content"),
            ],
        )]);
        let b = make_test_trace(vec![make_round(
            0,
            "test",
            vec![make_tool_call("tc_1", "glob", "*.rs")],
        )]);
        let diff = diff_traces(&a, &b);
        assert!(!diff.is_identical());
        // The extra tool call in a should be flagged
        assert!(!diff.round_diffs[0].tool_result_diffs.is_empty());
        assert!(matches!(
            diff.round_diffs[0].tool_result_diffs.last().expect("has diff").kind,
            ToolDiffKind::OnlyInA
        ));
    }

    #[test]
    fn test_diff_with_sub_traces() {
        let sub_a = make_test_trace(vec![make_round(0, "sub hello", vec![])]);
        let sub_b = make_test_trace(vec![make_round(0, "sub world", vec![])]);

        let mut a = make_test_trace(vec![make_round(0, "main", vec![])]);
        a.sub_traces.push(sub_a);

        let mut b = make_test_trace(vec![make_round(0, "main", vec![])]);
        b.sub_traces.push(sub_b);

        let diff = diff_traces(&a, &b);
        assert!(!diff.is_identical());
        // Main rounds are identical
        assert!(diff.round_diffs.is_empty());
        // Sub-traces differ
        assert_eq!(diff.sub_trace_diffs.len(), 1);
        assert!(diff.sub_trace_diffs[0].diff.round_diffs[0].text_differs);
    }

    #[test]
    fn test_diff_sub_trace_count_mismatch() {
        let sub = make_test_trace(vec![make_round(0, "sub", vec![])]);

        let mut a = make_test_trace(vec![make_round(0, "main", vec![])]);
        a.sub_traces.push(sub);

        let b = make_test_trace(vec![make_round(0, "main", vec![])]);

        let diff = diff_traces(&a, &b);
        assert!(!diff.is_identical());
        assert_eq!(diff.sub_trace_diffs.len(), 1);
    }

    #[tokio::test]
    async fn test_replay_with_sub_traces() {
        let sub = make_test_trace(vec![make_round(0, "sub output", vec![])]);

        let mut trace = make_test_trace(vec![make_round(0, "main output", vec![])]);
        trace.sub_traces.push(sub);

        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        replay_trace(&trace, &tx).await.expect("replay should succeed");
        drop(tx);

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }

        // Should have: main events + round_end + sub_trace_start + sub events + round_end + sub_trace_end
        let has_sub_start = events.iter().any(|e| matches!(e, ReplayEvent::SubTraceStart { .. }));
        let has_sub_end = events.iter().any(|e| matches!(e, ReplayEvent::SubTraceEnd { .. }));
        assert!(has_sub_start, "Should have SubTraceStart event");
        assert!(has_sub_end, "Should have SubTraceEnd event");
    }

    #[test]
    fn test_parse_message_end_with_cache_tokens() {
        let raw = serde_json::json!({
            "type": "message_end",
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_read_input_tokens": 30,
            "cache_creation_input_tokens": 10
        });
        let event = parse_single_raw_event(&raw).expect("should parse");
        if let LlmEvent::MessageEnd { usage } = event {
            assert_eq!(usage.input_tokens, 100);
            assert_eq!(usage.output_tokens, 50);
            assert_eq!(usage.cache_read_input_tokens, Some(30));
            assert_eq!(usage.cache_creation_input_tokens, Some(10));
        } else {
            panic!("Expected MessageEnd event");
        }
    }

    #[test]
    fn test_parse_message_end_without_cache_tokens() {
        let raw = serde_json::json!({
            "type": "message_end",
            "input_tokens": 100,
            "output_tokens": 50
        });
        let event = parse_single_raw_event(&raw).expect("should parse");
        if let LlmEvent::MessageEnd { usage } = event {
            assert_eq!(usage.cache_read_input_tokens, None);
            assert_eq!(usage.cache_creation_input_tokens, None);
        } else {
            panic!("Expected MessageEnd event");
        }
    }
}
