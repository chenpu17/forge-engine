//! Write tool - Write file contents

use crate::description::ToolDescriptions;
use crate::platform::PlatformPaths;
use crate::security::validate_write_path_with_confirmed;
use crate::{ConfirmationLevel, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::OnceLock;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r#"Write content to a file.

Creates the file if it doesn't exist. Overwrites if it does. Creates parent directories automatically.

Usage:
- Create new file: {"file_path": "/path/to/file.rs", "content": "fn main() {}"}

Notes:
- Use absolute paths
- For small edits to existing files, prefer the 'edit' tool instead
- Sensitive paths (system files, credentials) require confirmation
- Always verify content before writing to avoid data loss"#;

/// File writing tool
pub struct WriteTool;

impl WriteTool {
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
impl Tool for WriteTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("write", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let file_path = crate::required_str(&params, "file_path")?;
        let content = crate::required_str(&params, "content")?;

        // Check plan mode - block write operations
        if ctx.plan_mode_active() {
            return Err(ToolError::PermissionDenied(
                "Write is not available in plan mode. Use exit_plan_mode to enable all tools."
                    .to_string(),
            ));
        }

        // Validate path for security (path traversal, sensitive files, working dir)
        let path = validate_write_path_with_confirmed(
            file_path,
            ctx.working_dir(),
            ctx.confirmed_paths(),
        )?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolError::ExecutionFailed(format!("Failed to create directory: {e}"))
            })?;
        }

        // Write file
        tokio::fs::write(&path, content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to write file: {e}")))?;

        Ok(ToolOutput::success(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            path.display()
        )))
    }

    fn confirmation_level(&self, params: &Value) -> ConfirmationLevel {
        if let Some(path) = params["file_path"].as_str() {
            if Self::is_sensitive_path(path) {
                return ConfirmationLevel::Dangerous;
            }
        }
        // All writes need at least one-time confirmation
        ConfirmationLevel::Once
    }
}
