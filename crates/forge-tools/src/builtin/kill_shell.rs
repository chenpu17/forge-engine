//! `KillShell` tool - Terminate background shell processes
//!
//! This tool allows terminating shell commands that were started
//! with `run_in_background=true`.

use crate::background::BackgroundTaskType;
use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};

use crate::BackgroundTaskManager;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r"Terminate a background shell process.

Use this to stop a shell command that was started with run_in_background=true.

Parameters:
- id: The task ID of the background shell to kill (required)
- force: If true, forcefully terminates the process immediately. If false (default), attempts graceful termination first.

Notes:
- Only shell tasks can be killed. Sub-agent tasks should be allowed to complete naturally.
- On Unix, this is a best-effort operation. Child processes may not be terminated if they were not spawned as a process group.";

/// `KillShell` tool for terminating background processes
pub struct KillShellTool {
    /// Background task manager reference
    manager: Arc<BackgroundTaskManager>,
}

impl KillShellTool {
    /// Create a new `KillShellTool` with a background task manager
    #[must_use]
    pub const fn new(manager: Arc<BackgroundTaskManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for KillShellTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "kill_shell"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("kill_shell", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The shell task ID to terminate"
                },
                "force": {
                    "type": "boolean",
                    "description": "Force immediate termination (default: false, attempts graceful termination first)"
                }
            },
            "required": ["id"]
        })
    }

    fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
        // Killing processes requires confirmation
        ConfirmationLevel::Once
    }

    async fn execute(
        &self,
        params: Value,
        _ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let id = crate::required_str(&params, "id")?;
        let force = crate::optional_bool(&params, "force", false);

        // Check if task exists and is a shell task
        let task_arc = self
            .manager
            .get_task(id)
            .await
            .ok_or_else(|| ToolError::ExecutionFailed(format!("Task not found: {id}")))?;

        {
            let task = task_arc.read().await;
            if task.task_type != BackgroundTaskType::Shell {
                return Err(ToolError::ExecutionFailed(
                    "Only shell tasks can be killed. Sub-agent tasks should complete naturally."
                        .to_string(),
                ));
            }
        }

        self.manager.kill(id, force).await.map_err(ToolError::ExecutionFailed)?;

        Ok(ToolOutput::success(format!(
            "Task {} has been {}.",
            id,
            if force { "forcefully terminated" } else { "terminated" }
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool() -> KillShellTool {
        KillShellTool::new(Arc::new(BackgroundTaskManager::new()))
    }

    #[test]
    fn test_kill_shell_tool_name() {
        let tool = make_tool();
        assert_eq!(tool.name(), "kill_shell");
    }

    #[test]
    fn test_kill_shell_tool_confirmation_level() {
        let tool = make_tool();
        assert_eq!(tool.confirmation_level(&json!({})), ConfirmationLevel::Once);
    }

    #[test]
    fn test_kill_shell_tool_schema() {
        let tool = make_tool();
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["id"].is_object());
        assert!(schema["properties"]["force"].is_object());
        assert!(schema["required"].as_array().unwrap().contains(&json!("id")));
    }
}
