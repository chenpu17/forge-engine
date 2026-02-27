//! Memory write tool - write/update memory files

use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_memory::{MemoryScope, MemoryWriter, WriteMode};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::OnceLock;

const FALLBACK_DESCRIPTION: &str = r#"Write or update memory files in the structured memory system.

Parameters:
- scope: "user" | "project" (default: "project")
- path: Relative file path (e.g. "preferences.md", "projects/forge.md")
- content: The markdown content to write
- mode: "replace" | "append" (default: "replace")
- allow_sensitive: boolean (default: false) - Allow sensitive content when user explicitly requests it
- sensitive_reason: string - Required when allow_sensitive=true, reason for storing sensitive content

Notes:
- Cannot write to index.md (auto-maintained)
- Files get YAML frontmatter automatically
- Index is updated automatically after write
- 2000 token limit per file
- High-risk content (private key blocks) is always rejected even with allow_sensitive=true
"#;

/// Memory write tool
pub struct MemoryWriteTool {
    user_dir: PathBuf,
}

impl Default for MemoryWriteTool {
    fn default() -> Self {
        Self::new(forge_infra::data_dir().join("memory"))
    }
}

impl MemoryWriteTool {
    /// Create a `memory_write` tool bound to a specific user memory directory.
    #[must_use]
    pub const fn new(user_dir: PathBuf) -> Self {
        Self { user_dir }
    }

    fn make_writer(&self) -> MemoryWriter {
        MemoryWriter::new(self.user_dir.clone())
    }

    fn parse_scope(params: &Value) -> MemoryScope {
        match params["scope"].as_str().unwrap_or("project") {
            "user" => MemoryScope::User,
            _ => MemoryScope::Project,
        }
    }

    fn parse_mode(params: &Value) -> WriteMode {
        match params["mode"].as_str().unwrap_or("replace") {
            "append" => WriteMode::Append,
            "merge" => WriteMode::Merge,
            _ => WriteMode::Replace,
        }
    }

    fn project_dir(ctx: &dyn ToolExecutionContext) -> std::path::PathBuf {
        ctx.working_dir().join(".forge").join("memory")
    }
}

#[async_trait]
impl Tool for MemoryWriteTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "memory_write"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("memory_write", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "scope": {
                    "type": "string",
                    "enum": ["user", "project"],
                    "description": "Memory scope (default: project)"
                },
                "path": {
                    "type": "string",
                    "description": "Relative file path (e.g. preferences.md)"
                },
                "content": {
                    "type": "string",
                    "description": "Markdown content to write"
                },
                "mode": {
                    "type": "string",
                    "enum": ["replace", "append", "merge"],
                    "description": "Write mode (default: replace). merge: smart section merge (Phase 2, currently falls back to replace)"
                },
                "allow_sensitive": {
                    "type": "boolean",
                    "description": "Allow writing content that contains sensitive patterns (default: false). Only set when user explicitly requests storing sensitive info."
                },
                "sensitive_reason": {
                    "type": "string",
                    "description": "Required when allow_sensitive=true. Reason for storing sensitive content (for audit)."
                }
            },
            "required": ["path", "content"]
        })
    }

    fn confirmation_level(&self, params: &Value) -> ConfirmationLevel {
        if params["allow_sensitive"].as_bool().unwrap_or(false) {
            return ConfirmationLevel::Dangerous;
        }
        match params["mode"].as_str().unwrap_or("replace") {
            "append" | "merge" => ConfirmationLevel::Once,
            _ => ConfirmationLevel::Always,
        }
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let path = crate::required_str(&params, "path")?;
        let content = crate::required_str(&params, "content")?;
        let scope = Self::parse_scope(&params);
        let mode = Self::parse_mode(&params);
        let allow_sensitive = params["allow_sensitive"].as_bool().unwrap_or(false);

        // Validate sensitive_reason is provided when allow_sensitive=true
        if allow_sensitive {
            let reason = params["sensitive_reason"].as_str().unwrap_or("").trim();
            if reason.is_empty() {
                return Err(ToolError::ExecutionFailed(
                    "sensitive_reason is required when allow_sensitive=true".into(),
                ));
            }
            tracing::warn!(
                scope = %scope,
                path = %path,
                reason = %reason,
                "Sensitive memory write authorized by user"
            );
        }

        // Reject writes to index.md
        if path == "index.md" || path.ends_with("/index.md") {
            return Err(ToolError::ExecutionFailed(
                "Cannot write to index.md directly. It is auto-maintained.".into(),
            ));
        }

        let writer = self.make_writer();
        let project_dir = Self::project_dir(ctx);

        writer
            .write_file(scope, Some(&project_dir), path, content, mode, allow_sensitive)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let mode_str = match mode {
            WriteMode::Replace => "written",
            WriteMode::Append => "appended to",
            WriteMode::Merge => "merged into",
        };

        Ok(ToolOutput::success(format!("Memory {mode_str}: {path} ({scope} scope)")))
    }
}
