//! `TodoWrite` tool - Task status management
//!
//! This tool allows the AI to create and manage a structured task list,
//! helping track progress and organize complex tasks.

use crate::description::ToolDescriptions;
use crate::{Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::sync::{Arc, OnceLock, RwLock};

/// Task status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    /// Task not yet started
    Pending,
    /// Currently working on
    InProgress,
    /// Task finished successfully
    Completed,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Completed => write!(f, "completed"),
        }
    }
}

/// A single todo item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    /// The task description (imperative form, e.g., "Fix bug")
    pub content: String,
    /// The active form shown during execution (e.g., "Fixing bug")
    #[serde(rename = "activeForm")]
    pub active_form: String,
    /// Current status
    pub status: TodoStatus,
}

/// Shared todo state
#[derive(Debug, Default)]
pub struct TodoState {
    todos: RwLock<Vec<TodoItem>>,
}

impl TodoState {
    /// Create a new empty todo state
    #[must_use]
    pub const fn new() -> Self {
        Self { todos: RwLock::new(Vec::new()) }
    }

    /// Get all todos
    #[must_use]
    pub fn get_todos(&self) -> Vec<TodoItem> {
        self.todos.read().map(|t| t.clone()).unwrap_or_default()
    }

    /// Set all todos (replaces existing)
    pub fn set_todos(&self, todos: Vec<TodoItem>) {
        if let Ok(mut guard) = self.todos.write() {
            *guard = todos;
        }
    }

    /// Get the current in-progress task
    #[must_use]
    pub fn current_task(&self) -> Option<TodoItem> {
        self.todos
            .read()
            .ok()
            .and_then(|t| t.iter().find(|item| item.status == TodoStatus::InProgress).cloned())
    }

    /// Get progress summary
    #[must_use]
    pub fn progress_summary(&self) -> (usize, usize, usize) {
        let todos = self.get_todos();
        let completed = todos.iter().filter(|t| t.status == TodoStatus::Completed).count();
        let in_progress = todos.iter().filter(|t| t.status == TodoStatus::InProgress).count();
        let pending = todos.iter().filter(|t| t.status == TodoStatus::Pending).count();
        (completed, in_progress, pending)
    }
}

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r"Create and manage a structured task list for tracking progress. Use this to plan complex tasks, track what you're working on, and show the user your progress. Each task has a content (imperative form), activeForm (present continuous), and status (pending/in_progress/completed). Only one task should be in_progress at a time.";

/// `TodoWrite` tool for task management
pub struct TodoWriteTool {
    state: Arc<TodoState>,
}

impl TodoWriteTool {
    /// Create a new `TodoWrite` tool with shared state
    #[must_use]
    pub const fn new(state: Arc<TodoState>) -> Self {
        Self { state }
    }

    /// Create a new `TodoWrite` tool with its own state
    #[must_use]
    pub fn with_new_state() -> Self {
        Self { state: Arc::new(TodoState::new()) }
    }

    /// Get the shared state
    #[must_use]
    pub fn state(&self) -> Arc<TodoState> {
        Arc::clone(&self.state)
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "todo_write"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("todo_write", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The updated todo list",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "minLength": 1,
                                "description": "Task description in imperative form (e.g., 'Fix bug', 'Run tests')"
                            },
                            "activeForm": {
                                "type": "string",
                                "minLength": 1,
                                "description": "Task description in present continuous form (e.g., 'Fixing bug', 'Running tests')"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Current task status"
                            }
                        },
                        "required": ["content", "status", "activeForm"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    async fn execute(
        &self,
        params: Value,
        _ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        // Parse todos from params
        let todos_value = params.get("todos").cloned().unwrap_or(Value::Array(vec![]));

        let todos: Vec<TodoItem> = serde_json::from_value(todos_value).map_err(|e| {
            ToolError::InvalidParams(format!("Failed to parse todos: {e}"))
        })?;

        // Validate: only one task should be in_progress
        let in_progress_count = todos.iter().filter(|t| t.status == TodoStatus::InProgress).count();
        if in_progress_count > 1 {
            return Ok(ToolOutput::error(format!(
                "Only one task should be in_progress at a time, but found {in_progress_count}"
            )));
        }

        // Update state
        self.state.set_todos(todos.clone());

        // Build output
        let (completed, in_progress, pending) = self.state.progress_summary();
        let total = completed + in_progress + pending;

        let mut output = String::new();
        let _ = write!(output, "Todo list updated: {completed}/{total} completed");

        if let Some(current) = self.state.current_task() {
            let _ = write!(output, "\nCurrently: {}", current.active_form);
        }

        output.push_str("\n\n");

        // Format todo list
        for (i, todo) in todos.iter().enumerate() {
            let status_icon = match todo.status {
                TodoStatus::Completed => "\u{2713}",
                TodoStatus::InProgress => "\u{2192}",
                TodoStatus::Pending => "\u{25CB}",
            };
            let _ = writeln!(output, "{}. [{}] {}", i + 1, status_icon, todo.content);
        }

        Ok(ToolOutput::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    #[test]
    fn test_todo_status_display() {
        assert_eq!(TodoStatus::Pending.to_string(), "pending");
        assert_eq!(TodoStatus::InProgress.to_string(), "in_progress");
        assert_eq!(TodoStatus::Completed.to_string(), "completed");
    }

    #[test]
    fn test_todo_state_new() {
        let state = TodoState::new();
        assert!(state.get_todos().is_empty());
    }

    #[test]
    fn test_todo_state_set_get() {
        let state = TodoState::new();
        let todos = vec![
            TodoItem {
                content: "Fix bug".to_string(),
                active_form: "Fixing bug".to_string(),
                status: TodoStatus::InProgress,
            },
            TodoItem {
                content: "Write tests".to_string(),
                active_form: "Writing tests".to_string(),
                status: TodoStatus::Pending,
            },
        ];

        state.set_todos(todos.clone());
        let retrieved = state.get_todos();

        assert_eq!(retrieved.len(), 2);
        assert_eq!(retrieved[0].content, "Fix bug");
        assert_eq!(retrieved[1].content, "Write tests");
    }

    #[test]
    fn test_todo_state_current_task() {
        let state = TodoState::new();
        let todos = vec![
            TodoItem {
                content: "Completed task".to_string(),
                active_form: "Completing task".to_string(),
                status: TodoStatus::Completed,
            },
            TodoItem {
                content: "Current task".to_string(),
                active_form: "Working on current task".to_string(),
                status: TodoStatus::InProgress,
            },
            TodoItem {
                content: "Future task".to_string(),
                active_form: "Working on future task".to_string(),
                status: TodoStatus::Pending,
            },
        ];

        state.set_todos(todos);
        let current = state.current_task();

        assert!(current.is_some());
        assert_eq!(current.expect("should have current task").content, "Current task");
    }

    #[test]
    fn test_todo_state_progress_summary() {
        let state = TodoState::new();
        let todos = vec![
            TodoItem {
                content: "Task 1".to_string(),
                active_form: "Task 1".to_string(),
                status: TodoStatus::Completed,
            },
            TodoItem {
                content: "Task 2".to_string(),
                active_form: "Task 2".to_string(),
                status: TodoStatus::Completed,
            },
            TodoItem {
                content: "Task 3".to_string(),
                active_form: "Task 3".to_string(),
                status: TodoStatus::InProgress,
            },
            TodoItem {
                content: "Task 4".to_string(),
                active_form: "Task 4".to_string(),
                status: TodoStatus::Pending,
            },
        ];

        state.set_todos(todos);
        let (completed, in_progress, pending) = state.progress_summary();

        assert_eq!(completed, 2);
        assert_eq!(in_progress, 1);
        assert_eq!(pending, 1);
    }

    #[tokio::test]
    async fn test_todo_write_tool_execute() {
        let tool = TodoWriteTool::with_new_state();
        let ctx = ToolContext::default();

        let params = json!({
            "todos": [
                {
                    "content": "Fix bug",
                    "activeForm": "Fixing bug",
                    "status": "in_progress"
                },
                {
                    "content": "Write tests",
                    "activeForm": "Writing tests",
                    "status": "pending"
                }
            ]
        });

        let result = tool.execute(params, &ctx).await.expect("execute should succeed");
        assert!(!result.is_error);
        assert!(
            result.content.contains("0/2 completed"),
            "Expected '0/2 completed' in: {}",
            result.content
        );
        assert!(
            result.content.contains("Fixing bug"),
            "Expected 'Fixing bug' in: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_todo_write_tool_multiple_in_progress_error() {
        let tool = TodoWriteTool::with_new_state();
        let ctx = ToolContext::default();

        let params = json!({
            "todos": [
                {
                    "content": "Task 1",
                    "activeForm": "Task 1",
                    "status": "in_progress"
                },
                {
                    "content": "Task 2",
                    "activeForm": "Task 2",
                    "status": "in_progress"
                }
            ]
        });

        let result = tool.execute(params, &ctx).await.expect("execute should succeed");
        assert!(result.is_error);
        assert!(result.content.contains("Only one task"));
    }

    #[test]
    fn test_todo_item_serialization() {
        let item = TodoItem {
            content: "Test task".to_string(),
            active_form: "Testing task".to_string(),
            status: TodoStatus::InProgress,
        };

        let json = serde_json::to_string(&item).expect("serialize");
        assert!(json.contains("\"activeForm\""));
        assert!(json.contains("\"in_progress\""));

        let parsed: TodoItem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.content, "Test task");
        assert_eq!(parsed.status, TodoStatus::InProgress);
    }
}
