//! Edit tool - Edit file with precise replacements

use crate::description::ToolDescriptions;
use crate::platform::PlatformPaths;
use crate::security::validate_write_path;
use crate::{ConfirmationLevel, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::OnceLock;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r#"Edit a file by replacing exact string matches.

Performs precise string replacement. old_string must appear exactly once (or use replace_all for multiple).

Usage:
- Single replacement: {"file_path": "/path/to/file.rs", "old_string": "old code", "new_string": "new code"}
- Replace all occurrences: {"file_path": "/path/to/file.rs", "old_string": "foo", "new_string": "bar", "replace_all": true}

Notes:
- old_string must match EXACTLY (including whitespace and indentation)
- Fails if old_string is not unique (use larger context to make it unique)
- For creating new files, use 'write' tool instead
- Read the file first to ensure correct old_string"#;

/// File editing tool
pub struct EditTool;

impl EditTool {
    /// Check if a path is sensitive using platform-specific detection
    fn is_sensitive_path(path: &str) -> bool {
        // Use platform-specific sensitive path detection
        if PlatformPaths::is_sensitive_path(Path::new(path)) {
            return true;
        }

        // Also check project-level sensitive patterns
        if let Some(filename) = Path::new(path).file_name() {
            if PlatformPaths::needs_confirmation(&filename.to_string_lossy()) {
                return true;
            }
        }

        false
    }
}

#[async_trait]
impl Tool for EditTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("edit", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact string to replace"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement string"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)"
                }
            },
            "required": ["file_path", "old_string", "new_string"]
        })
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let file_path = crate::required_str(&params, "file_path")?;
        let old_string = crate::required_str(&params, "old_string")?;
        let new_string = crate::required_str(&params, "new_string")?;
        let replace_all = crate::optional_bool(&params, "replace_all", false);

        // Check plan mode - block edit operations
        if ctx.plan_mode_active() {
            return Err(ToolError::PermissionDenied(
                "Edit is not available in plan mode. Use exit_plan_mode to enable all tools."
                    .to_string(),
            ));
        }

        // Validate path for security (path traversal, sensitive files, symlink protection)
        let path = validate_write_path(file_path, ctx.working_dir())?;

        // Read file
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read file: {e}")))?;

        // Check if old_string exists
        let count = content.matches(old_string).count();
        if count == 0 {
            return Err(ToolError::ExecutionFailed("old_string not found in file".to_string()));
        }

        if count > 1 && !replace_all {
            return Err(ToolError::ExecutionFailed(format!(
                "old_string found {count} times. Use replace_all=true or provide more context.",
            )));
        }

        // Replace
        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        // Write back
        tokio::fs::write(&path, &new_content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to write file: {e}")))?;

        // Generate diff output
        let diff = generate_diff(old_string, new_string);

        Ok(ToolOutput::success(format!(
            "Successfully replaced {} occurrence(s) in {}\n\n{}",
            if replace_all { count } else { 1 },
            path.display(),
            diff
        )))
    }

    fn confirmation_level(&self, params: &Value) -> ConfirmationLevel {
        if let Some(path) = params["file_path"].as_str() {
            if Self::is_sensitive_path(path) {
                return ConfirmationLevel::Dangerous;
            }
        }
        // All edits need at least one-time confirmation
        ConfirmationLevel::Once
    }
}

/// Generate a unified diff format for the replacement
fn generate_diff(old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut output = String::new();
    output.push_str("```diff\n");

    // Show removed lines
    for line in &old_lines {
        let _ = writeln!(output, "- {line}");
    }

    // Show added lines
    for line in &new_lines {
        let _ = writeln!(output, "+ {line}");
    }

    output.push_str("```");
    output
}
