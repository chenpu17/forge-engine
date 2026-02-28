//! Agent event types for Python bindings

use forge_sdk::AgentEvent;
use pyo3::prelude::*;

/// Agent event wrapper
#[pyclass(name = "AgentEvent")]
#[derive(Clone)]
pub struct PyAgentEvent {
    #[pyo3(get)]
    pub event_type: String,
    #[pyo3(get)]
    pub content: Option<String>,
    #[pyo3(get)]
    pub tool_name: Option<String>,
    #[pyo3(get)]
    pub tool_id: Option<String>,
    #[pyo3(get)]
    pub tool_input: Option<String>,
    #[pyo3(get)]
    pub is_error: Option<bool>,
    #[pyo3(get)]
    pub input_tokens: Option<usize>,
    #[pyo3(get)]
    pub output_tokens: Option<usize>,
    #[pyo3(get)]
    pub cache_read_tokens: Option<usize>,
    #[pyo3(get)]
    pub cache_creation_tokens: Option<usize>,
    #[pyo3(get)]
    pub confirmation_level: Option<String>,
    #[pyo3(get)]
    pub current_tokens: Option<usize>,
    #[pyo3(get)]
    pub limit: Option<usize>,
    #[pyo3(get)]
    pub messages_before: Option<usize>,
    #[pyo3(get)]
    pub messages_after: Option<usize>,
    #[pyo3(get)]
    pub tokens_saved: Option<usize>,
    #[pyo3(get)]
    pub task_type: Option<String>,
    #[pyo3(get)]
    pub description: Option<String>,
    #[pyo3(get)]
    pub exit_code: Option<i32>,
    #[pyo3(get)]
    pub success: Option<bool>,
    #[pyo3(get)]
    pub plan_file: Option<String>,
    #[pyo3(get)]
    pub saved: Option<bool>,
    #[pyo3(get)]
    pub task: Option<String>,
    #[pyo3(get)]
    pub todos_json: Option<String>,
    #[pyo3(get)]
    pub suggestion: Option<String>,
}

#[pymethods]
impl PyAgentEvent {
    fn __repr__(&self) -> String {
        format!("AgentEvent(type='{}')", self.event_type)
    }
}

impl Default for PyAgentEvent {
    fn default() -> Self {
        Self {
            event_type: String::new(),
            content: None,
            tool_name: None,
            tool_id: None,
            tool_input: None,
            is_error: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            cache_creation_tokens: None,
            confirmation_level: None,
            current_tokens: None,
            limit: None,
            messages_before: None,
            messages_after: None,
            tokens_saved: None,
            task_type: None,
            description: None,
            exit_code: None,
            success: None,
            plan_file: None,
            saved: None,
            task: None,
            todos_json: None,
            suggestion: None,
        }
    }
}

impl From<AgentEvent> for PyAgentEvent {
    fn from(event: AgentEvent) -> Self {
        match event {
            AgentEvent::ThinkingStart => {
                Self { event_type: "thinking_start".into(), ..Default::default() }
            }
            AgentEvent::Thinking { content } => {
                Self { event_type: "thinking".into(), content: Some(content), ..Default::default() }
            }
            AgentEvent::TextDelta { delta } => {
                Self { event_type: "text_delta".into(), content: Some(delta), ..Default::default() }
            }
            AgentEvent::ToolCallStart { id, name, input } => Self {
                event_type: "tool_call_start".into(),
                tool_id: Some(id),
                tool_name: Some(name),
                tool_input: Some(input.to_string()),
                ..Default::default()
            },
            AgentEvent::ToolExecuting { id, name, input } => Self {
                event_type: "tool_executing".into(),
                tool_id: Some(id),
                tool_name: Some(name),
                tool_input: Some(input.to_string()),
                ..Default::default()
            },
            AgentEvent::ToolResult { id, output, is_error } => Self {
                event_type: "tool_result".into(),
                tool_id: Some(id),
                content: Some(output),
                is_error: Some(is_error),
                ..Default::default()
            },
            AgentEvent::TokenUsage { input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens } => Self {
                event_type: "token_usage".into(),
                input_tokens: Some(input_tokens),
                output_tokens: Some(output_tokens),
                cache_read_tokens,
                cache_creation_tokens,
                ..Default::default()
            },
            AgentEvent::ConfirmationRequired { id, tool, params, level } => Self {
                event_type: "confirmation_required".into(),
                tool_id: Some(id),
                tool_name: Some(tool),
                tool_input: Some(params.to_string()),
                confirmation_level: Some(format!("{level:?}")),
                ..Default::default()
            },
            AgentEvent::PathConfirmationRequired { id, path, reason } => Self {
                event_type: "path_confirmation_required".into(),
                tool_id: Some(id),
                content: Some(format!("Path: {path}, Reason: {reason}")),
                ..Default::default()
            },
            AgentEvent::TodoAdded { task } => {
                Self { event_type: "todo_added".into(), task: Some(task), ..Default::default() }
            }
            AgentEvent::TodoCompleted { task } => {
                Self { event_type: "todo_completed".into(), task: Some(task), ..Default::default() }
            }
            AgentEvent::TodoUpdate { todos } => Self {
                event_type: "todo_update".into(),
                todos_json: serde_json::to_string(&todos).ok(),
                ..Default::default()
            },
            AgentEvent::Done { summary } => {
                Self { event_type: "done".into(), content: summary, ..Default::default() }
            }
            AgentEvent::Cancelled => {
                Self { event_type: "cancelled".into(), ..Default::default() }
            }
            AgentEvent::Error { message } => Self {
                event_type: "error".into(),
                content: Some(message),
                is_error: Some(true),
                ..Default::default()
            },
            AgentEvent::Recovery { action, suggestion } => Self {
                event_type: "recovery".into(),
                content: Some(action),
                suggestion,
                ..Default::default()
            },
            AgentEvent::ContextWarning { current_tokens, limit } => Self {
                event_type: "context_warning".into(),
                current_tokens: Some(current_tokens),
                limit: Some(limit),
                ..Default::default()
            },
            AgentEvent::ContextCompressed { messages_before, messages_after, tokens_saved } => Self {
                event_type: "context_compressed".into(),
                messages_before: Some(messages_before),
                messages_after: Some(messages_after),
                tokens_saved: Some(tokens_saved),
                ..Default::default()
            },
            AgentEvent::ContextRecoveryAttempt { message } => Self {
                event_type: "context_recovery_attempt".into(),
                content: Some(message),
                ..Default::default()
            },
            AgentEvent::Retrying { attempt, max_attempts, error } => Self {
                event_type: "retrying".into(),
                content: Some(format!("Attempt {attempt}/{max_attempts}: {error}")),
                ..Default::default()
            },
            AgentEvent::BackgroundTaskStarted { id, task_type, description } => Self {
                event_type: "background_task_started".into(),
                tool_id: Some(id),
                task_type: Some(task_type),
                description: Some(description),
                ..Default::default()
            },
            AgentEvent::BackgroundTaskCompleted { id, exit_code, success } => Self {
                event_type: "background_task_completed".into(),
                tool_id: Some(id),
                exit_code,
                success: Some(success),
                ..Default::default()
            },
            AgentEvent::PlanModeEntered { plan_file } => Self {
                event_type: "plan_mode_entered".into(),
                plan_file,
                ..Default::default()
            },
            AgentEvent::PlanModeExited { saved, plan_file } => Self {
                event_type: "plan_mode_exited".into(),
                saved: Some(saved),
                plan_file,
                ..Default::default()
            },
            AgentEvent::CheckpointCreated { head_sha } => Self {
                event_type: "checkpoint_created".into(),
                content: Some(head_sha),
                ..Default::default()
            },
            AgentEvent::RolledBack { reason, files_restored } => Self {
                event_type: "rolled_back".into(),
                content: Some(reason),
                messages_after: Some(files_restored),
                ..Default::default()
            },
        }
    }
}
