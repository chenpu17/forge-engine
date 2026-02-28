//! Agent event bindings for NAPI

use napi_derive::napi;
use serde::{Deserialize, Serialize};

/// Todo item
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsTodoItem {
    pub task: String,
    pub status: String,
}

/// Agent event data (serializable to JSON for JS)
#[napi(object)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsAgentEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub session_id: Option<String>,
    pub request_id: Option<String>,
    pub content: Option<String>,
    pub delta: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub input: Option<String>,
    pub output: Option<String>,
    pub is_error: Option<bool>,
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub cache_read_tokens: Option<u32>,
    pub cache_creation_tokens: Option<u32>,
    pub action: Option<String>,
    pub suggestion: Option<String>,
    pub tool: Option<String>,
    pub params: Option<String>,
    pub level: Option<String>,
    pub task: Option<String>,
    pub todos: Option<Vec<JsTodoItem>>,
    pub summary: Option<String>,
    pub message: Option<String>,
    pub current_tokens: Option<u32>,
    pub limit: Option<u32>,
    pub messages_before: Option<u32>,
    pub messages_after: Option<u32>,
    pub tokens_saved: Option<u32>,
    pub task_type: Option<String>,
    pub description: Option<String>,
    pub exit_code: Option<i32>,
    pub success: Option<bool>,
    pub plan_file: Option<String>,
    pub saved: Option<bool>,
    pub attempt: Option<u32>,
    pub max_attempts: Option<u32>,
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
                e.input_tokens = Some(input_tokens as u32);
                e.output_tokens = Some(output_tokens as u32);
                e.cache_read_tokens = cache_read_tokens.map(|t| t as u32);
                e.cache_creation_tokens = cache_creation_tokens.map(|t| t as u32);
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
                e.current_tokens = Some(current_tokens as u32);
                e.limit = Some(limit as u32);
                e
            }
            forge_sdk::AgentEvent::ContextCompressed {
                messages_before, messages_after, tokens_saved,
            } => {
                let mut e = Self::new("ContextCompressed");
                e.messages_before = Some(messages_before as u32);
                e.messages_after = Some(messages_after as u32);
                e.tokens_saved = Some(tokens_saved as u32);
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
                e.messages_after = Some(files_restored as u32);
                e
            }
        }
    }
}
