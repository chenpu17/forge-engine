//! `TaskOutput` tool - Retrieve output from background tasks
//!
//! This tool allows retrieving the status and output of tasks
//! started with `run_in_background=true`.

use crate::background::BackgroundTaskStatus;
use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r"Retrieve output from a background task.

Use this to check the status and output of tasks started with run_in_background=true.

Parameters:
- id: The task ID returned when starting the background task (required)
- wait: If true, blocks until task completes (default: false)
- timeout_ms: Maximum time to wait in milliseconds (default: 30000, max: 600000)

Returns task status, output so far, and whether task is still running.";

/// `TaskOutput` tool for retrieving background task output
pub struct TaskOutputTool {
    /// Background task manager reference
    background_manager: Option<Arc<crate::background::BackgroundTaskManager>>,
}

impl TaskOutputTool {
    /// Create a new `TaskOutput` tool
    #[must_use]
    pub const fn new() -> Self {
        Self { background_manager: None }
    }

    /// Create a new `TaskOutput` tool with a background manager
    #[must_use]
    pub const fn with_background_manager(
        manager: Arc<crate::background::BackgroundTaskManager>,
    ) -> Self {
        Self { background_manager: Some(manager) }
    }
}

impl Default for TaskOutputTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TaskOutputTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "task_output"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("task_output", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The task ID to get output from"
                },
                "wait": {
                    "type": "boolean",
                    "description": "Wait for task to complete (default: false)"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Max wait time in ms (default: 30000, max: 600000)"
                }
            },
            "required": ["id"]
        })
    }

    fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
        ConfirmationLevel::None
    }

    async fn execute(
        &self,
        params: Value,
        _ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let id = crate::required_str(&params, "id")?;
        let wait = crate::optional_bool(&params, "wait", false);
        let timeout_ms = crate::optional_u64(&params, "timeout_ms", 30_000).min(600_000);

        let manager = self.background_manager.as_ref().ok_or_else(|| {
            ToolError::ExecutionFailed("Background task manager not available".to_string())
        })?;

        let result = manager
            .get_output(id, wait, Some(timeout_ms))
            .await
            .map_err(ToolError::ExecutionFailed)?;

        let status_str = match &result.status {
            BackgroundTaskStatus::Running => "running",
            BackgroundTaskStatus::Completed { exit_code } => {
                if *exit_code == Some(0) || exit_code.is_none() {
                    "completed"
                } else {
                    "completed (non-zero exit)"
                }
            }
            BackgroundTaskStatus::Failed { .. } => "failed",
            BackgroundTaskStatus::Killed => "killed",
        };

        let output = format!(
            "Task ID: {}\nStatus: {}\nStill running: {}\n\n--- Output ---\n{}",
            result.id, status_str, result.is_running, result.output
        );

        if let BackgroundTaskStatus::Failed { error } = &result.status {
            Ok(ToolOutput::error(format!("{output}\n\nError: {error}")))
        } else {
            Ok(ToolOutput::success(output))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_output_tool_name() {
        let tool = TaskOutputTool::new();
        assert_eq!(tool.name(), "task_output");
    }

    #[test]
    fn test_task_output_tool_confirmation_level() {
        let tool = TaskOutputTool::new();
        assert_eq!(tool.confirmation_level(&json!({})), ConfirmationLevel::None);
    }

    #[test]
    fn test_task_output_tool_schema() {
        let tool = TaskOutputTool::new();
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["id"].is_object());
        assert!(schema["properties"]["wait"].is_object());
        assert!(schema["properties"]["timeout_ms"].is_object());
        assert!(schema["required"].as_array().expect("required array").contains(&json!("id")));
    }
}
