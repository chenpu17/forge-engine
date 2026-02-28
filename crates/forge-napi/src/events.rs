//! Agent event bindings for NAPI

use napi_derive::napi;
use serde::{Deserialize, Serialize};

/// Todo item exposed to JavaScript.
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsTodoItem {
    /// Task description.
    pub task: String,
    /// Task status (e.g. "pending", "done").
    pub status: String,
}

/// Agent event data (serializable to JSON for JS).
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsAgentEvent {
    /// Event type discriminator (e.g. "TextDelta", "ToolResult", "Done").
    #[serde(rename = "type")]
    pub event_type: String,
    /// Session identifier.
    pub session_id: Option<String>,
    /// Request identifier.
    pub request_id: Option<String>,
    /// Text content (thinking, path info, checkpoint SHA).
    pub content: Option<String>,
    /// Incremental text delta.
    pub delta: Option<String>,
    /// Tool call or confirmation identifier.
    pub id: Option<String>,
    /// Tool name.
    pub name: Option<String>,
    /// Tool input (JSON string).
    pub input: Option<String>,
    /// Tool output text.
    pub output: Option<String>,
    /// Whether a tool result is an error.
    pub is_error: Option<bool>,
    /// Input tokens consumed.
    pub input_tokens: Option<u32>,
    /// Output tokens generated.
    pub output_tokens: Option<u32>,
    /// Cache read tokens.
    pub cache_read_tokens: Option<u32>,
    /// Cache creation tokens.
    pub cache_creation_tokens: Option<u32>,
    /// Recovery action taken.
    pub action: Option<String>,
    /// Recovery suggestion.
    pub suggestion: Option<String>,
    /// Tool name for confirmation events.
    pub tool: Option<String>,
    /// Tool parameters (JSON string) for confirmation.
    pub params: Option<String>,
    /// Confirmation level.
    pub level: Option<String>,
    /// Todo task description.
    pub task: Option<String>,
    /// List of todo items.
    pub todos: Option<Vec<JsTodoItem>>,
    /// Done summary.
    pub summary: Option<String>,
    /// Error or recovery message.
    pub message: Option<String>,
    /// Current token count for context warning.
    pub current_tokens: Option<u32>,
    /// Token limit for context warning.
    pub limit: Option<u32>,
    /// Message count before compression.
    pub messages_before: Option<u32>,
    /// Message count after compression.
    pub messages_after: Option<u32>,
    /// Tokens saved by compression.
    pub tokens_saved: Option<u32>,
    /// Background task type.
    pub task_type: Option<String>,
    /// Background task description.
    pub description: Option<String>,
    /// Process exit code.
    pub exit_code: Option<i32>,
    /// Whether the operation succeeded.
    pub success: Option<bool>,
    /// Plan mode file path.
    pub plan_file: Option<String>,
    /// Whether plan was saved on exit.
    pub saved: Option<bool>,
    /// Retry attempt number.
    pub attempt: Option<u32>,
    /// Maximum retry attempts.
    pub max_attempts: Option<u32>,
    /// OpenTelemetry trace identifier.
    pub trace_id: Option<String>,
}

impl JsAgentEvent {
    fn new(event_type: &str) -> Self {
        Self {
            event_type: event_type.to_string(),
            session_id: None, request_id: None, content: None, delta: None,
            id: None, name: None, input: None, output: None, is_error: None,
            input_tokens: None, output_tokens: None,
            cache_read_tokens: None, cache_creation_tokens: None,
            action: None, suggestion: None, tool: None, params: None, level: None,
            task: None, todos: None, summary: None, message: None,
            current_tokens: None, limit: None,
            messages_before: None, messages_after: None, tokens_saved: None,
            task_type: None, description: None, exit_code: None, success: None,
            plan_file: None, saved: None, attempt: None, max_attempts: None,
            trace_id: None,
        }
    }

    /// Check if this is a terminal event (Done, Cancelled, or Error).
    pub fn is_terminal(&self) -> bool {
        matches!(self.event_type.as_str(), "Done" | "Cancelled" | "Error")
    }
}

impl From<forge_sdk::AgentEvent> for JsAgentEvent {
    fn from(event: forge_sdk::AgentEvent) -> Self {
        match event {
            forge_sdk::AgentEvent::ThinkingStart => Self::new("ThinkingStart"),
            forge_sdk::AgentEvent::Thinking { content } => {
                let mut e = Self::new("Thinking");
                e.content = Some(content);
                e
            }
            forge_sdk::AgentEvent::TextDelta { delta } => {
                let mut e = Self::new("TextDelta");
                e.delta = Some(delta);
                e
            }
            forge_sdk::AgentEvent::ToolCallStart { id, name, input } => {
                let mut e = Self::new("ToolCallStart");
                e.id = Some(id);
                e.name = Some(name);
                e.input = Some(serde_json::to_string(&input).unwrap_or_default());
                e
            }
            forge_sdk::AgentEvent::ToolExecuting { id, name, input } => {
                let mut e = Self::new("ToolExecuting");
                e.id = Some(id);
                e.name = Some(name);
                e.input = Some(serde_json::to_string(&input).unwrap_or_default());
                e
            }
            forge_sdk::AgentEvent::ToolResult { id, output, is_error } => {
                let mut e = Self::new("ToolResult");
                e.id = Some(id);
                e.output = Some(output);
                e.is_error = Some(is_error);
                e
            }
            forge_sdk::AgentEvent::TokenUsage {
                input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens,
            } => {
                let mut e = Self::new("TokenUsage");
                e.input_tokens = Some(u32::try_from(input_tokens).unwrap_or(u32::MAX));
                e.output_tokens = Some(u32::try_from(output_tokens).unwrap_or(u32::MAX));
                e.cache_read_tokens = cache_read_tokens.map(|t| u32::try_from(t).unwrap_or(u32::MAX));
                e.cache_creation_tokens = cache_creation_tokens.map(|t| u32::try_from(t).unwrap_or(u32::MAX));
                e
            }
            forge_sdk::AgentEvent::Recovery { action, suggestion } => {
                let mut e = Self::new("Recovery");
                e.action = Some(action);
                e.suggestion = suggestion;
                e
            }
            forge_sdk::AgentEvent::ConfirmationRequired { id, tool, params, level } => {
                let mut e = Self::new("ConfirmationRequired");
                e.id = Some(id);
                e.tool = Some(tool);
                e.params = Some(serde_json::to_string(&params).unwrap_or_default());
                e.level = Some(format!("{:?}", level));
                e
            }
            forge_sdk::AgentEvent::PathConfirmationRequired { id, path, reason } => {
                let mut e = Self::new("PathConfirmationRequired");
                e.id = Some(id);
                e.content = Some(format!("Path: {path}, Reason: {reason}"));
                e
            }
            forge_sdk::AgentEvent::TodoAdded { task } => {
                let mut e = Self::new("TodoAdded");
                e.task = Some(task);
                e
            }
            forge_sdk::AgentEvent::TodoCompleted { task } => {
                let mut e = Self::new("TodoCompleted");
                e.task = Some(task);
                e
            }
            forge_sdk::AgentEvent::TodoUpdate { todos } => {
                let mut e = Self::new("TodoUpdate");
                e.todos = Some(todos.into_iter().map(|t| JsTodoItem {
                    task: t.content,
                    status: format!("{:?}", t.status),
                }).collect());
                e
            }
            forge_sdk::AgentEvent::ContextWarning { current_tokens, limit } => {
                let mut e = Self::new("ContextWarning");
                e.current_tokens = Some(u32::try_from(current_tokens).unwrap_or(u32::MAX));
                e.limit = Some(u32::try_from(limit).unwrap_or(u32::MAX));
                e
            }
            forge_sdk::AgentEvent::ContextCompressed {
                messages_before, messages_after, tokens_saved,
            } => {
                let mut e = Self::new("ContextCompressed");
                e.messages_before = Some(u32::try_from(messages_before).unwrap_or(u32::MAX));
                e.messages_after = Some(u32::try_from(messages_after).unwrap_or(u32::MAX));
                e.tokens_saved = Some(u32::try_from(tokens_saved).unwrap_or(u32::MAX));
                e
            }
            forge_sdk::AgentEvent::ContextRecoveryAttempt { message } => {
                let mut e = Self::new("ContextRecoveryAttempt");
                e.message = Some(message);
                e
            }
            forge_sdk::AgentEvent::Retrying { attempt, max_attempts, error } => {
                let mut e = Self::new("Retrying");
                e.attempt = Some(attempt);
                e.max_attempts = Some(max_attempts);
                e.message = Some(error);
                e
            }
            forge_sdk::AgentEvent::Done { summary } => {
                let mut e = Self::new("Done");
                e.summary = summary;
                e
            }
            forge_sdk::AgentEvent::Cancelled => Self::new("Cancelled"),
            forge_sdk::AgentEvent::Error { message } => {
                let mut e = Self::new("Error");
                e.message = Some(message);
                e
            }
            forge_sdk::AgentEvent::BackgroundTaskStarted { id, task_type, description } => {
                let mut e = Self::new("BackgroundTaskStarted");
                e.id = Some(id);
                e.task_type = Some(task_type);
                e.description = Some(description);
                e
            }
            forge_sdk::AgentEvent::BackgroundTaskCompleted { id, exit_code, success } => {
                let mut e = Self::new("BackgroundTaskCompleted");
                e.id = Some(id);
                e.exit_code = exit_code;
                e.success = Some(success);
                e
            }
            forge_sdk::AgentEvent::PlanModeEntered { plan_file } => {
                let mut e = Self::new("PlanModeEntered");
                e.plan_file = plan_file;
                e
            }
            forge_sdk::AgentEvent::PlanModeExited { saved, plan_file } => {
                let mut e = Self::new("PlanModeExited");
                e.saved = Some(saved);
                e.plan_file = plan_file;
                e
            }
            forge_sdk::AgentEvent::CheckpointCreated { head_sha } => {
                let mut e = Self::new("CheckpointCreated");
                e.content = Some(head_sha);
                e
            }
            forge_sdk::AgentEvent::RolledBack { reason, files_restored } => {
                let mut e = Self::new("RolledBack");
                e.message = Some(reason);
                e.messages_after = Some(u32::try_from(files_restored).unwrap_or(u32::MAX));
                e
            }
        }
    }
}
