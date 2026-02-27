//! Skill tool - Invoke skills during conversation
//!
//! This tool allows the AI model to invoke skills based on context matching.
//! It's part of the Model-Invoked mechanism for automatic skill discovery.

use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r#"Invoke a skill to handle specialized tasks.

Skills provide domain-specific capabilities and optimized prompts for common workflows.
When you recognize a task that matches an available skill, use this tool to invoke it.

Parameters:
- skill: The skill name (required, e.g., "commit", "review-pr")
- args: Optional arguments to pass to the skill (e.g., "fix typo in README")

The skill will be activated and its specialized instructions will guide you through the task.

Note: Only invoke skills that are listed in the <available_skills> section of your system prompt."#;

/// Skill tool for invoking model-discovered skills
pub struct SkillTool;

impl SkillTool {
    /// Create a new Skill tool
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Parse the skill invocation marker from tool output
    ///
    /// Returns `Some((skill_name, args))` if the output contains a valid marker
    #[must_use]
    pub fn parse_skill_marker(output: &str) -> Option<(String, String)> {
        for line in output.lines() {
            if let Some(rest) = line.strip_prefix("__SKILL_INVOKE__:") {
                let mut parts = rest.splitn(2, ':');
                let name = parts.next()?.trim().to_string();
                let args = parts.next().unwrap_or("").trim().to_string();
                if !name.is_empty() {
                    return Some((name, args));
                }
            }
        }
        None
    }
}

impl Default for SkillTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for SkillTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("skill", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The skill name to invoke (e.g., 'commit', 'review-pr')"
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments for the skill"
                }
            },
            "required": ["skill"]
        })
    }

    fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
        // Skill invocation doesn't require confirmation
        // The skill itself may have confirmation requirements
        ConfirmationLevel::None
    }

    async fn execute(
        &self,
        params: Value,
        _ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let skill_name = crate::required_str(&params, "skill")?;
        let args = crate::optional_str(&params, "args").unwrap_or_default();

        // Validate skill name format (basic check)
        if skill_name.is_empty() {
            return Ok(ToolOutput::error("Skill name cannot be empty"));
        }

        // Return marker for Agent to process
        // The Agent will:
        // 1. Look up the skill in SkillRegistry
        // 2. Load full skill content if needed
        // 3. Create SkillExecutionContext with allowed-tools
        // 4. Inject skill prompt into conversation
        let marker = if args.is_empty() {
            format!("__SKILL_INVOKE__:{skill_name}:")
        } else {
            format!("__SKILL_INVOKE__:{skill_name}:{args}")
        };

        Ok(ToolOutput::success(format!(
            "Invoking skill '{skill_name}'{}\n\n{marker}",
            if args.is_empty() { String::new() } else { format!(" with args: {args}") },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    #[test]
    fn test_skill_tool_name() {
        let tool = SkillTool::new();
        assert_eq!(tool.name(), "skill");
    }

    #[test]
    fn test_skill_tool_confirmation_level() {
        let tool = SkillTool::new();
        assert_eq!(tool.confirmation_level(&json!({})), ConfirmationLevel::None);
    }

    #[test]
    fn test_skill_tool_schema() {
        let tool = SkillTool::new();
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["skill"].is_object());
        assert!(schema["properties"]["args"].is_object());
        assert_eq!(schema["required"], json!(["skill"]));
    }

    #[tokio::test]
    async fn test_skill_invoke_with_args() {
        let tool = SkillTool::new();
        let ctx = ToolContext::default();

        let result = tool
            .execute(
                json!({
                    "skill": "commit",
                    "args": "fix typo"
                }),
                &ctx,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("Invoking skill 'commit'"));
        assert!(result.content.contains("__SKILL_INVOKE__:commit:fix typo"));
    }

    #[tokio::test]
    async fn test_skill_invoke_without_args() {
        let tool = SkillTool::new();
        let ctx = ToolContext::default();

        let result = tool.execute(json!({"skill": "commit"}), &ctx).await.unwrap();

        assert!(!result.is_error);
        assert!(result.content.contains("__SKILL_INVOKE__:commit:"));
    }

    #[tokio::test]
    async fn test_skill_invoke_empty_name() {
        let tool = SkillTool::new();
        let ctx = ToolContext::default();

        let result = tool.execute(json!({"skill": ""}), &ctx).await.unwrap();

        assert!(result.is_error);
        assert!(result.content.contains("cannot be empty"));
    }

    #[test]
    fn test_parse_skill_marker() {
        let output = "Invoking skill 'commit'\n\n__SKILL_INVOKE__:commit:fix typo";
        let result = SkillTool::parse_skill_marker(output);
        assert_eq!(result, Some(("commit".to_string(), "fix typo".to_string())));

        let output_no_args = "__SKILL_INVOKE__:review:";
        let result = SkillTool::parse_skill_marker(output_no_args);
        assert_eq!(result, Some(("review".to_string(), "".to_string())));

        let no_marker = "Some regular output";
        assert!(SkillTool::parse_skill_marker(no_marker).is_none());
    }
}
