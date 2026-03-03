//! Trace recording types for deterministic replay.
//!
//! Captures the complete execution trace of an agent run — every LLM request,
//! response, tool call, and sub-agent invocation — enabling offline replay,
//! debugging, and regression testing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Top-level trace
// ---------------------------------------------------------------------------

/// A complete execution trace for one agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTrace {
    /// Unique trace identifier.
    pub trace_id: String,
    /// Session identifier (if running within a session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Agent type (e.g. "explore", "plan", "general-purpose", "main").
    pub agent_type: String,
    /// Model used for this agent.
    pub model: String,
    /// When the agent started executing.
    pub started_at: DateTime<Utc>,
    /// When the agent finished (None if still running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    /// Recorded rounds (one per agent loop iteration).
    pub rounds: Vec<TraceRound>,
    /// Aggregate token usage and cost.
    pub total_usage: TraceUsage,
    /// Nested sub-agent traces.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_traces: Vec<AgentTrace>,
}

// ---------------------------------------------------------------------------
// Per-round trace
// ---------------------------------------------------------------------------

/// A single round of the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceRound {
    /// Round number (0-indexed).
    pub round: usize,
    /// What was sent to the LLM.
    pub request: TraceRequest,
    /// What the LLM returned.
    pub response: TraceResponse,
    /// Tool calls executed in this round.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<TraceToolCall>,
}

// ---------------------------------------------------------------------------
// Request / Response recording
// ---------------------------------------------------------------------------

/// Recorded LLM request metadata.
///
/// We store a hash of the full message list rather than the messages themselves
/// to keep trace files manageable. The full messages can be reconstructed from
/// the round sequence if needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceRequest {
    /// SHA-256 hash of serialized messages (for integrity verification).
    pub messages_hash: String,
    /// Number of messages sent.
    pub messages_count: usize,
    /// Number of tool definitions sent.
    pub tools_count: usize,
    /// Model requested.
    pub model: String,
    /// Max tokens setting.
    pub max_tokens: usize,
    /// Temperature setting.
    pub temperature: f64,
}

/// Recorded LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceResponse {
    /// Assembled text output from the LLM.
    pub text: String,
    /// Tool use blocks emitted by the LLM.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_uses: Vec<TraceToolUse>,
    /// Token usage for this response.
    pub usage: TraceUsage,
    /// Raw LLM events in order (the replay core).
    ///
    /// Stored as serialized JSON values to avoid tight coupling to
    /// `LlmEvent` enum variants, which may evolve.
    pub raw_events: Vec<Value>,
}

/// A tool use block as returned by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceToolUse {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool input (JSON).
    pub input: Value,
}

// ---------------------------------------------------------------------------
// Tool call recording
// ---------------------------------------------------------------------------

/// Recorded tool call and its result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceToolCall {
    /// Tool call ID (matches the LLM's tool use ID).
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool input parameters.
    pub input: Value,
    /// Tool output text.
    pub output: String,
    /// Whether the tool returned an error.
    pub is_error: bool,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Usage / cost
// ---------------------------------------------------------------------------

/// Token usage and optional cost for a trace segment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraceUsage {
    /// Input tokens consumed.
    pub input_tokens: usize,
    /// Output tokens generated.
    pub output_tokens: usize,
    /// Cache-read tokens (if applicable).
    #[serde(default)]
    pub cache_read_tokens: usize,
    /// Cache-write tokens (if applicable).
    #[serde(default)]
    pub cache_write_tokens: usize,
    /// Estimated cost in USD (None if pricing info unavailable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

impl TraceUsage {
    /// Add another usage record into this one.
    pub fn add(&mut self, other: &Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
        match (self.cost_usd, other.cost_usd) {
            (Some(a), Some(b)) => self.cost_usd = Some(a + b),
            (None, Some(b)) => self.cost_usd = Some(b),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_usage_add() {
        let mut a = TraceUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: 10,
            cache_write_tokens: 0,
            cost_usd: Some(0.01),
        };
        let b = TraceUsage {
            input_tokens: 200,
            output_tokens: 100,
            cache_read_tokens: 20,
            cache_write_tokens: 5,
            cost_usd: Some(0.02),
        };
        a.add(&b);
        assert_eq!(a.input_tokens, 300);
        assert_eq!(a.output_tokens, 150);
        assert_eq!(a.cache_read_tokens, 30);
        assert_eq!(a.cache_write_tokens, 5);
        assert!((a.cost_usd.unwrap() - 0.03).abs() < f64::EPSILON);
    }

    #[test]
    fn test_trace_usage_add_none_cost() {
        let mut a = TraceUsage { cost_usd: None, ..Default::default() };
        let b = TraceUsage { cost_usd: Some(0.01), ..Default::default() };
        a.add(&b);
        assert_eq!(a.cost_usd, Some(0.01));
    }

    #[test]
    fn test_agent_trace_serde_roundtrip() {
        let trace = AgentTrace {
            trace_id: "t-001".to_string(),
            session_id: Some("sess-1".to_string()),
            agent_type: "explore".to_string(),
            model: "claude-sonnet-4".to_string(),
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            rounds: vec![TraceRound {
                round: 0,
                request: TraceRequest {
                    messages_hash: "abc123".to_string(),
                    messages_count: 3,
                    tools_count: 5,
                    model: "claude-sonnet-4".to_string(),
                    max_tokens: 8192,
                    temperature: 0.3,
                },
                response: TraceResponse {
                    text: "Found 5 files.".to_string(),
                    tool_uses: vec![TraceToolUse {
                        id: "tc_1".to_string(),
                        name: "glob".to_string(),
                        input: serde_json::json!({"pattern": "*.rs"}),
                    }],
                    usage: TraceUsage {
                        input_tokens: 500,
                        output_tokens: 100,
                        cost_usd: Some(0.002),
                        ..Default::default()
                    },
                    raw_events: vec![
                        serde_json::json!({"type": "text_delta", "delta": "Found"}),
                    ],
                },
                tool_calls: vec![TraceToolCall {
                    id: "tc_1".to_string(),
                    name: "glob".to_string(),
                    input: serde_json::json!({"pattern": "*.rs"}),
                    output: "src/main.rs\nsrc/lib.rs".to_string(),
                    is_error: false,
                    duration_ms: 15,
                }],
            }],
            total_usage: TraceUsage {
                input_tokens: 500,
                output_tokens: 100,
                cost_usd: Some(0.002),
                ..Default::default()
            },
            sub_traces: vec![],
        };

        let json = serde_json::to_string_pretty(&trace).expect("serialize");
        let parsed: AgentTrace = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.trace_id, "t-001");
        assert_eq!(parsed.rounds.len(), 1);
        assert_eq!(parsed.rounds[0].tool_calls.len(), 1);
        assert_eq!(parsed.rounds[0].response.raw_events.len(), 1);
    }

    #[test]
    fn test_trace_tool_call_serde() {
        let tc = TraceToolCall {
            id: "tc_1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "ls"}),
            output: "file.txt".to_string(),
            is_error: false,
            duration_ms: 42,
        };
        let json = serde_json::to_string(&tc).expect("serialize");
        let parsed: TraceToolCall = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, "bash");
        assert_eq!(parsed.duration_ms, 42);
    }

    #[test]
    fn test_empty_sub_traces_omitted() {
        let trace = AgentTrace {
            trace_id: "t-002".to_string(),
            session_id: None,
            agent_type: "main".to_string(),
            model: "test".to_string(),
            started_at: Utc::now(),
            completed_at: None,
            rounds: vec![],
            total_usage: TraceUsage::default(),
            sub_traces: vec![],
        };
        let json = serde_json::to_string(&trace).expect("serialize");
        assert!(!json.contains("sub_traces"));
        assert!(!json.contains("session_id"));
    }
}
