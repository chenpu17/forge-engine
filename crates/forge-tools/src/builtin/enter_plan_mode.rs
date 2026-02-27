//! `EnterPlanMode` tool - Enter read-only exploration mode
//!
//! This tool switches the agent into plan mode where only read-only
//! tools are available, allowing safe exploration before implementation.

use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = "Enter plan mode for read-only exploration.\n\nIn plan mode:\n- You can only use read-only tools (read, glob, grep, web_fetch, web_search, task_output)\n- Write operations (write, edit, bash, git) are disabled\n- Use this to explore codebases and design implementation strategies safely\n\nParameters:\n- plan_file: Optional path for the plan file (default: auto-generated in ~/.forge/plans/)\n\nCall exit_plan_mode when ready to execute your plan.";

/// `EnterPlanMode` tool for switching to read-only mode
pub struct EnterPlanModeTool;

impl EnterPlanModeTool {
    /// Create a new `EnterPlanMode` tool
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for EnterPlanModeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "enter_plan_mode"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("enter_plan_mode", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "plan_file": {
                    "type": "string",
                    "description": "Path for plan file (optional, auto-generated if not provided)"
                }
            },
            "required": []
        })
    }

    fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
        ConfirmationLevel::None
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        // Check if already in plan mode
        if ctx.plan_mode_active() {
            return Ok(ToolOutput::error(
                "Already in plan mode. Use exit_plan_mode to exit first.",
            ));
        }

        // Get plan file path
        let plan_file = crate::optional_str(&params, "plan_file")
            .map_or_else(
                || {
                    // Generate default plan file path
                    let plans_dir = dirs::home_dir()
                        .map_or_else(
                            || std::path::PathBuf::from("/tmp/.forge/plans"),
                            |h| h.join(".forge/plans"),
                        );

                    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
                    let id = &uuid::Uuid::new_v4().to_string()[..8];
                    plans_dir.join(format!("plan-{timestamp}-{id}.md"))
                },
                std::path::PathBuf::from,
            );

        // Ensure plans directory exists
        if let Some(parent) = plan_file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ToolError::ExecutionFailed(format!("Failed to create plans directory: {e}"))
            })?;
        }

        Ok(ToolOutput::success(format!(
            "Entering plan mode.\n\n\
            Plan will be saved to: {path}\n\n\
            In this mode, you can only use read-only tools:\n\
            - read: Read file contents\n\
            - glob: Find files by pattern\n\
            - grep: Search file contents\n\
            - web_fetch, web_search: Research online\n\
            - task_output: Check background task status\n\n\
            Write operations (write, edit, bash, git) are DISABLED.\n\n\
            Call exit_plan_mode when ready to execute your plan.\n\n\
            __PLAN_MODE_ENTER__:{path}",
            path = plan_file.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    #[test]
    fn test_enter_plan_mode_tool_name() {
        let tool = EnterPlanModeTool::new();
        assert_eq!(tool.name(), "enter_plan_mode");
    }

    #[test]
    fn test_enter_plan_mode_tool_confirmation_level() {
        let tool = EnterPlanModeTool::new();
        assert_eq!(tool.confirmation_level(&json!({})), ConfirmationLevel::None);
    }

    #[test]
    fn test_enter_plan_mode_tool_schema() {
        let tool = EnterPlanModeTool::new();
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["plan_file"].is_object());
    }

    #[tokio::test]
    async fn test_enter_plan_mode_already_active() {
        let tool = EnterPlanModeTool::new();
        let ctx = ToolContext::default();
        ctx.set_plan_mode_active(true);

        let result = tool.execute(json!({}), &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Already in plan mode"));
    }
}
