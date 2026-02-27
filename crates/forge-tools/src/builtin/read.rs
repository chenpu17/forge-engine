//! Read tool - Read file contents

use crate::description::ToolDescriptions;
use crate::security::validate_path;
use crate::{ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r#"Read the contents of a file.

Returns file content with line numbers (format: "   123\tcontent"). Use offset/limit for large files.

Usage:
- Read entire file: {"file_path": "/path/to/file.rs"}
- Read lines 50-100: {"file_path": "/path/to/file.rs", "offset": 50, "limit": 50}

Notes:
- Always use absolute paths
- For large files, use offset/limit to read specific sections
- Returns error if file doesn't exist or is not readable"#;

/// File reading tool
pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("read", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-indexed)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read"
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_readonly(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let file_path = crate::required_str(&params, "file_path")?;
        let offset = crate::optional_usize(&params, "offset", 1);
        #[allow(clippy::cast_possible_truncation)]
        let limit = params["limit"].as_u64().map(|l| l as usize);

        // Validate path for security (path traversal, sensitive files, working dir)
        let path = validate_path(file_path, ctx.working_dir())?;

        // Read file
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to read file: {e}")))?;

        // Apply offset and limit
        let lines: Vec<&str> = content.lines().collect();
        let start = offset.saturating_sub(1);
        let end = limit.map_or(lines.len(), |l| (start + l).min(lines.len()));

        let result: String = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}\t{}", start + i + 1, line))
            .collect::<Vec<_>>()
            .join("\n");

        Ok(ToolOutput::success(result))
    }
}
