//! Agent and LLM event types.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Agent events
// ---------------------------------------------------------------------------

/// Events emitted during agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Agent started thinking.
    ThinkingStart,
    /// Extended thinking content (for reasoning models).
    Thinking {
        /// Thinking content text.
        content: String,
    },
    /// Text delta from LLM.
    TextDelta {
        /// Text delta content.
        delta: String,
    },
    /// Tool call started.
    ToolCallStart {
        /// Tool call ID.
        id: String,
        /// Tool name.
        name: String,
        /// Tool input parameters.
        input: serde_json::Value,
    },
    /// Tool is executing.
    ToolExecuting {
        /// Tool call ID.
        id: String,
        /// Tool name.
        name: String,
        /// Tool input parameters.
        input: serde_json::Value,
    },
    /// Tool execution completed.
    ToolResult {
        /// Tool call ID.
        id: String,
        /// Output content.
        output: String,
        /// Whether the result is an error.
        is_error: bool,
    },
    /// Confirmation required for tool.
    ConfirmationRequired {
        /// Tool call ID.
        id: String,
        /// Tool name.
        tool: String,
        /// Tool parameters.
        params: serde_json::Value,
        /// Required confirmation level.
        level: super::tool::ConfirmationLevel,
    },
    /// Path confirmation required from user.
    PathConfirmationRequired {
        /// Tool call ID.
        id: String,
        /// Path requiring confirmation.
        path: String,
        /// Reason for confirmation.
        reason: String,
    },
    /// Todo item added.
    TodoAdded {
        /// Task description.
        task: String,
    },
    /// Todo item completed.
    TodoCompleted {
        /// Task description.
        task: String,
    },
    /// Todo list updated.
    TodoUpdate {
        /// Current todo items.
        todos: Vec<TodoItem>,
    },
    /// Agent completed successfully.
    Done {
        /// Optional summary of work done.
        summary: Option<String>,
    },
    /// Agent was cancelled.
    Cancelled,
    /// Error occurred.
    Error {
        /// Error message.
        message: String,
    },
    /// Recovery action taken after error.
    Recovery {
        /// Recovery action description.
        action: String,
        /// Optional suggestion for the user.
        suggestion: Option<String>,
    },
    /// Context usage warning (approaching limit).
    ContextWarning {
        /// Current token count.
        current_tokens: usize,
        /// Token limit.
        limit: usize,
    },
    /// Context was compressed to fit within limits.
    ContextCompressed {
        /// Message count before compression.
        messages_before: usize,
        /// Message count after compression.
        messages_after: usize,
        /// Tokens saved by compression.
        tokens_saved: usize,
    },
    /// Context overflow recovery attempt.
    ContextRecoveryAttempt {
        /// Recovery message.
        message: String,
    },
    /// LLM request retrying after error.
    Retrying {
        /// Current attempt number.
        attempt: u32,
        /// Maximum attempts allowed.
        max_attempts: u32,
        /// Error that triggered the retry.
        error: String,
    },
    /// Token usage update.
    TokenUsage {
        /// Input tokens consumed.
        input_tokens: usize,
        /// Output tokens generated.
        output_tokens: usize,
        /// Cache read tokens (if applicable).
        cache_read_tokens: Option<usize>,
        /// Cache creation tokens (if applicable).
        cache_creation_tokens: Option<usize>,
    },
    /// Background task started.
    BackgroundTaskStarted {
        /// Task ID.
        id: String,
        /// Task type identifier.
        task_type: String,
        /// Human-readable description.
        description: String,
    },
    /// Background task completed.
    BackgroundTaskCompleted {
        /// Task ID.
        id: String,
        /// Process exit code (if applicable).
        exit_code: Option<i32>,
        /// Whether the task succeeded.
        success: bool,
    },
    /// Entered plan mode.
    PlanModeEntered {
        /// Path to the plan file.
        plan_file: Option<String>,
    },
    /// Exited plan mode.
    PlanModeExited {
        /// Whether the plan was saved.
        saved: bool,
        /// Path to the plan file.
        plan_file: Option<String>,
    },
    /// Git checkpoint created before first write operation.
    CheckpointCreated {
        /// Git HEAD SHA at checkpoint.
        head_sha: String,
    },
    /// Working tree rolled back to checkpoint.
    RolledBack {
        /// Reason for rollback.
        reason: String,
        /// Number of files restored.
        files_restored: usize,
    },
    /// Per-agent cost update.
    CostUpdate {
        /// Agent identifier.
        agent_id: String,
        /// Input tokens in this update.
        input_tokens: usize,
        /// Output tokens in this update.
        output_tokens: usize,
        /// Cumulative estimated cost in USD for this agent.
        estimated_cost_usd: f64,
        /// Budget limit in USD, if configured.
        budget_limit_usd: Option<f64>,
    },
    /// Cost approaching budget limit (above warning threshold).
    CostWarning {
        /// Agent identifier.
        agent_id: String,
        /// Current cumulative cost in USD.
        current_usd: f64,
        /// Budget limit in USD.
        limit_usd: f64,
        /// Percentage of budget consumed.
        percentage: f64,
    },
    /// Budget exceeded — agent should gracefully shut down.
    BudgetExceeded {
        /// Agent identifier.
        agent_id: String,
        /// Current cumulative cost in USD.
        current_usd: f64,
        /// Budget limit in USD.
        limit_usd: f64,
    },
    /// Trace recording completed for this agent.
    TraceRecorded {
        /// Trace identifier for linking parent/child traces.
        trace_id: String,
    },
    /// Session started (tracing).
    SessionStart {
        /// Session ID.
        session_id: String,
        /// Timestamp (Unix milliseconds).
        timestamp: i64,
        /// Session context.
        context: SessionContext,
    },
    /// Session ended (tracing).
    SessionEnd {
        /// Session ID.
        session_id: String,
        /// Timestamp (Unix milliseconds).
        timestamp: i64,
        /// Duration in milliseconds.
        duration_ms: u64,
    },
    /// User message (tracing).
    UserMessage {
        /// Message content.
        content: String,
        /// Timestamp (Unix milliseconds).
        timestamp: i64,
    },
    /// Assistant message (tracing).
    AssistantMessage {
        /// Message content.
        content: String,
        /// Timestamp (Unix milliseconds).
        timestamp: i64,
    },
    /// API request started (tracing).
    ApiRequest {
        /// Request ID.
        request_id: String,
        /// Model name.
        model: String,
        /// Timestamp (Unix milliseconds).
        timestamp: i64,
    },
    /// API response received (tracing).
    ApiResponse {
        /// Request ID.
        request_id: String,
        /// Duration in milliseconds.
        duration_ms: u64,
        /// Input tokens.
        input_tokens: u32,
        /// Output tokens.
        output_tokens: u32,
        /// Cache read tokens.
        cache_read_tokens: Option<u32>,
        /// Cache write tokens.
        cache_write_tokens: Option<u32>,
        /// Timestamp (Unix milliseconds).
        timestamp: i64,
    },
    /// API error (tracing).
    ApiError {
        /// Request ID.
        request_id: String,
        /// Error message.
        error: String,
        /// Error details.
        details: Option<String>,
        /// Timestamp (Unix milliseconds).
        timestamp: i64,
    },
    /// Tool result with full details (tracing).
    ToolResultDetailed {
        /// Tool call ID.
        id: String,
        /// Output content.
        output: serde_json::Value,
        /// Error message if failed.
        error: Option<String>,
        /// Duration in milliseconds.
        duration_ms: u64,
        /// Timestamp (Unix milliseconds).
        timestamp: i64,
    },
}

// ---------------------------------------------------------------------------
// Tool call / result
// ---------------------------------------------------------------------------

/// Session context information (tracing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    /// Engine version.
    pub engine_version: String,
    /// Working directory.
    pub working_dir: String,
    /// Git branch.
    pub git_branch: Option<String>,
    /// Git commit.
    pub git_commit: Option<String>,
    /// Model name.
    pub model: String,
    /// Config summary.
    pub config_summary: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Tool call / result
// ---------------------------------------------------------------------------

/// Tool call from LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this call.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool input parameters.
    pub input: serde_json::Value,
}

/// Result from tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// Tool call ID this result is for.
    pub tool_call_id: String,
    /// Output content.
    pub output: String,
    /// Whether this is an error.
    pub is_error: bool,
    /// Path confirmation needed (for paths outside working directory).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_confirmation: Option<PathConfirmation>,
}

impl ToolResult {
    /// Create a successful result
    #[must_use]
    pub fn success(tool_call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            output: output.into(),
            is_error: false,
            path_confirmation: None,
        }
    }

    /// Create an error result
    #[must_use]
    pub fn error(tool_call_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            output: message.into(),
            is_error: true,
            path_confirmation: None,
        }
    }
}

/// Path confirmation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathConfirmation {
    /// The path that needs confirmation.
    pub path: String,
    /// Human-readable reason.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Todo items (used by AgentEvent::TodoUpdate)
// ---------------------------------------------------------------------------

/// A todo item tracked during agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    /// Unique ID.
    pub id: String,
    /// Task description.
    pub content: String,
    /// Current status.
    pub status: TodoStatus,
}

/// Status of a todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoStatus {
    /// Task is pending.
    Pending,
    /// Task is in progress.
    InProgress,
    /// Task is completed.
    Completed,
}

// ---------------------------------------------------------------------------
// LLM events (generic subset — provider-specific events stay in forge-llm)
// ---------------------------------------------------------------------------

/// LLM streaming events.
#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// Text content delta.
    TextDelta(String),
    /// Tool use started.
    ToolUseStart {
        /// Tool call ID.
        id: String,
        /// Tool name.
        name: String,
    },
    /// Tool use input delta.
    ToolUseInputDelta {
        /// Tool call ID.
        id: String,
        /// Input delta.
        delta: String,
    },
    /// Tool use completed.
    ToolUseEnd {
        /// Tool call ID.
        id: String,
        /// Tool name.
        name: String,
        /// Parsed input.
        input: serde_json::Value,
    },
    /// Message completed.
    MessageEnd {
        /// Token usage.
        usage: Usage,
    },
    /// Error occurred.
    Error(String),
}

/// Token usage statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens.
    pub input_tokens: usize,
    /// Output tokens.
    pub output_tokens: usize,
    /// Cache read tokens (Anthropic prompt caching).
    #[serde(default)]
    pub cache_read_input_tokens: Option<usize>,
    /// Cache creation tokens.
    #[serde(default)]
    pub cache_creation_input_tokens: Option<usize>,
}

// ---------------------------------------------------------------------------
// Project analysis types
// ---------------------------------------------------------------------------

/// Detected project type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectType {
    /// Rust project.
    Rust,
    /// Node.js project.
    Node,
    /// Python project.
    Python,
    /// Go project.
    Go,
    /// Java project.
    Java,
    /// Mixed-language project.
    Mixed,
    /// Unknown project type.
    Unknown,
}

impl std::fmt::Display for ProjectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rust => write!(f, "Rust"),
            Self::Node => write!(f, "Node.js"),
            Self::Python => write!(f, "Python"),
            Self::Go => write!(f, "Go"),
            Self::Java => write!(f, "Java"),
            Self::Mixed => write!(f, "Mixed"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Project analysis result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectAnalysis {
    /// Project name.
    pub name: String,
    /// Project description.
    pub description: String,
    /// Project type.
    pub project_type: ProjectType,
    /// Technology stack.
    pub tech_stack: Vec<String>,
    /// Directory structure description.
    pub structure: String,
    /// Architecture description.
    pub architecture: String,
    /// Development conventions.
    pub conventions: String,
    /// Common commands.
    pub commands: Vec<ProjectCommand>,
    /// Important notes.
    pub notes: Vec<String>,
}

/// A command commonly used in a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectCommand {
    /// Command name/description.
    pub name: String,
    /// The actual command.
    pub command: String,
    /// What this command does.
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_call_serde_roundtrip() {
        let tc = ToolCall {
            id: "tc_1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "ls"}),
        };
        let json = serde_json::to_string(&tc).expect("serialize");
        let parsed: ToolCall = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.id, "tc_1");
        assert_eq!(parsed.name, "bash");
    }

    #[test]
    fn test_tool_result_serde_roundtrip() {
        let tr = ToolResult {
            tool_call_id: "tc_1".to_string(),
            output: "file.txt".to_string(),
            is_error: false,
            path_confirmation: None,
        };
        let json = serde_json::to_string(&tr).expect("serialize");
        let parsed: ToolResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.tool_call_id, "tc_1");
        assert!(!parsed.is_error);
    }

    #[test]
    fn test_usage_default() {
        let u = Usage::default();
        assert_eq!(u.input_tokens, 0);
        assert_eq!(u.output_tokens, 0);
        assert!(u.cache_read_input_tokens.is_none());
    }

    #[test]
    fn test_project_type_display() {
        assert_eq!(ProjectType::Rust.to_string(), "Rust");
        assert_eq!(ProjectType::Node.to_string(), "Node.js");
        assert_eq!(ProjectType::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn test_todo_status_serde() {
        let json = serde_json::to_string(&TodoStatus::InProgress).expect("serialize");
        assert_eq!(json, "\"inprogress\"");
        let parsed: TodoStatus = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, TodoStatus::InProgress);
    }

    #[test]
    fn test_agent_event_serde_roundtrip() {
        let events = vec![
            AgentEvent::ThinkingStart,
            AgentEvent::TextDelta { delta: "hello".to_string() },
            AgentEvent::Done { summary: Some("done".to_string()) },
            AgentEvent::Cancelled,
            AgentEvent::Error { message: "oops".to_string() },
        ];
        for event in &events {
            let json = serde_json::to_string(event).expect("serialize");
            let parsed: AgentEvent = serde_json::from_str(&json).expect("deserialize");
            // Verify round-trip produces identical JSON
            let json2 = serde_json::to_string(&parsed).expect("re-serialize");
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn test_cost_update_event_serde() {
        let event = AgentEvent::CostUpdate {
            agent_id: "agent-1".to_string(),
            input_tokens: 500,
            output_tokens: 100,
            estimated_cost_usd: 0.002_25,
            budget_limit_usd: Some(1.0),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let parsed: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        let json2 = serde_json::to_string(&parsed).expect("re-serialize");
        assert_eq!(json, json2);
    }

    #[test]
    fn test_cost_warning_event_serde() {
        let event = AgentEvent::CostWarning {
            agent_id: "agent-2".to_string(),
            current_usd: 0.85,
            limit_usd: 1.0,
            percentage: 85.0,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("cost_warning"));
        let parsed: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        let json2 = serde_json::to_string(&parsed).expect("re-serialize");
        assert_eq!(json, json2);
    }

    #[test]
    fn test_budget_exceeded_event_serde() {
        let event = AgentEvent::BudgetExceeded {
            agent_id: "agent-3".to_string(),
            current_usd: 5.5,
            limit_usd: 5.0,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("budget_exceeded"));
        let parsed: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        let json2 = serde_json::to_string(&parsed).expect("re-serialize");
        assert_eq!(json, json2);
    }
}
