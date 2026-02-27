//! `ExitPlanMode` tool - Exit read-only exploration mode
//!
//! This tool exits plan mode and optionally saves the plan to file.

use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = "Exit plan mode and optionally save the plan.\n\nParameters:\n- save: Whether to save the plan to file (default: true)\n\nReturns the plan summary and re-enables all tools.";

/// `ExitPlanMode` tool for switching back to normal mode
pub struct ExitPlanModeTool;

impl ExitPlanModeTool {
    /// Create a new `ExitPlanMode` tool
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for ExitPlanModeTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "exit_plan_mode"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("exit_plan_mode", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "save": {
                    "type": "boolean",
                    "description": "Save plan to file (default: true)"
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
        // Check if in plan mode
        if !ctx.plan_mode_active() {
            return Ok(ToolOutput::error("Not currently in plan mode."));
        }

        let save = crate::optional_bool(&params, "save", true);

        let output = if save {
            "Exiting plan mode.\n\n\
            Plan has been saved.\n\n\
            All tools are now available.\n\n\
            __PLAN_MODE_EXIT__:saved"
        } else {
            "Exiting plan mode.\n\n\
            Plan was not saved.\n\n\
            All tools are now available.\n\n\
            __PLAN_MODE_EXIT__:not_saved"
        };

        Ok(ToolOutput::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    #[test]
    fn test_exit_plan_mode_tool_name() {
        let tool = ExitPlanModeTool::new();
        assert_eq!(tool.name(), "exit_plan_mode");
    }

    #[test]
    fn test_exit_plan_mode_tool_confirmation_level() {
        let tool = ExitPlanModeTool::new();
        assert_eq!(tool.confirmation_level(&json!({})), ConfirmationLevel::None);
    }

    #[test]
    fn test_exit_plan_mode_tool_schema() {
        let tool = ExitPlanModeTool::new();
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["save"].is_object());
    }

    #[tokio::test]
    async fn test_exit_plan_mode_not_active() {
        let tool = ExitPlanModeTool::new();
        let ctx = ToolContext::default();

        let result = tool.execute(json!({}), &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Not currently in plan mode"));
    }

    #[tokio::test]
    async fn test_exit_plan_mode_with_save() {
        let tool = ExitPlanModeTool::new();
        let ctx = ToolContext::default();
        ctx.set_plan_mode_active(true);

        let result = tool.execute(json!({"save": true}), &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("__PLAN_MODE_EXIT__:saved"));
    }

    #[tokio::test]
    async fn test_exit_plan_mode_without_save() {
        let tool = ExitPlanModeTool::new();
        let ctx = ToolContext::default();
        ctx.set_plan_mode_active(true);

        let result = tool.execute(json!({"save": false}), &ctx).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("__PLAN_MODE_EXIT__:not_saved"));
    }
}
