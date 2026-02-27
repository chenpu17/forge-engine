//! Memory read tool - read memory files and index

use crate::description::ToolDescriptions;
use crate::{Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_memory::{MemoryLoader, MemoryScope, MemoryWriter};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;

const FALLBACK_DESCRIPTION: &str = r#"Read memory files from the structured memory system.

Actions:
- "read_index": Load the memory index for a scope (user/project)
- "read_file": Read a specific memory file by path
- "list": List files in a memory directory

Parameters:
- action: "read_index" | "read_file" | "list"
- scope: "user" | "project" (default: "project")
- path: File path for read_file, directory path for list (relative to memory root)

Examples:
- Read project index: {"action": "read_index", "scope": "project"}
- Read a file: {"action": "read_file", "scope": "user", "path": "preferences.md"}
- List files: {"action": "list", "scope": "project", "path": ""}
"#;

/// Memory read tool
pub struct MemoryReadTool {
    user_dir: PathBuf,
}

impl Default for MemoryReadTool {
    fn default() -> Self {
        Self::new(forge_infra::data_dir().join("memory"))
    }
}

impl MemoryReadTool {
    /// Create a `memory_read` tool bound to a specific user memory directory.
    #[must_use]
    pub const fn new(user_dir: PathBuf) -> Self {
        Self { user_dir }
    }

    fn make_loader(&self) -> MemoryLoader {
        MemoryLoader::new(self.user_dir.clone())
    }

    fn parse_scope(params: &Value) -> MemoryScope {
        match params["scope"].as_str().unwrap_or("project") {
            "user" => MemoryScope::User,
            _ => MemoryScope::Project,
        }
    }

    fn project_dir(ctx: &dyn ToolExecutionContext) -> std::path::PathBuf {
        ctx.working_dir().join(".forge").join("memory")
    }
}

#[async_trait]
impl Tool for MemoryReadTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "memory_read"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("memory_read", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read_index", "read_file", "list"],
                    "description": "The read action to perform"
                },
                "scope": {
                    "type": "string",
                    "enum": ["user", "project"],
                    "description": "Memory scope (default: project)"
                },
                "path": {
                    "type": "string",
                    "description": "File path (for read_file) or directory path (for list)"
                }
            },
            "required": ["action"]
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
        let action = crate::required_str(&params, "action")?;
        let scope = Self::parse_scope(&params);
        let loader = self.make_loader();
        let project_dir = Self::project_dir(ctx);

        match action {
            "read_index" => {
                let index = loader
                    .load_index(scope, Some(&project_dir))
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

                index.map_or_else(
                    || Ok(ToolOutput::success(format!("No memory index found for {scope} scope."))),
                    |idx| Ok(ToolOutput::success(idx.to_prompt_string())),
                )
            }
            "read_file" => {
                let result = Self::execute_read_file(&loader, &params, scope, &project_dir).await;
                // Async update last_used_at on successful read (non-blocking)
                if result.is_ok() {
                    if let Some(path) = params["path"].as_str() {
                        let writer = MemoryWriter::new(self.user_dir.clone());
                        let path = path.to_string();
                        let project_dir = project_dir.clone();
                        tokio::spawn(async move {
                            let _ =
                                writer.update_last_used_at(scope, Some(&project_dir), &path).await;
                        });
                    }
                }
                result
            }
            "list" => Self::execute_list(&loader, &params, scope, &project_dir).await,
            other => Err(ToolError::InvalidParams(format!(
                "Unknown action: {other}. Use read_index, read_file, or list."
            ))),
        }
    }
}

// Helper methods for execute actions
impl MemoryReadTool {
    async fn execute_read_file(
        loader: &MemoryLoader,
        params: &Value,
        scope: MemoryScope,
        project_dir: &std::path::Path,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidParams("path is required for read_file".into()))?;

        let file = loader
            .read_file(scope, Some(project_dir), path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        match file {
            Some(f) => {
                let mut output = String::new();
                let _ = writeln!(output, "# {}", f.path);
                if !f.meta.tags.is_empty() {
                    let _ = writeln!(output, "Tags: {}", f.meta.tags.join(", "));
                }
                if !f.meta.updated.is_empty() {
                    let _ = writeln!(output, "Updated: {}", f.meta.updated);
                }
                let _ = write!(output, "\n{}", f.content);
                Ok(ToolOutput::success(output))
            }
            None => Ok(ToolOutput::success(format!("Memory file not found: {path}"))),
        }
    }

    async fn execute_list(
        loader: &MemoryLoader,
        params: &Value,
        scope: MemoryScope,
        project_dir: &std::path::Path,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let dir = params["path"].as_str().unwrap_or("");

        let files = loader
            .list_files(scope, Some(project_dir), dir)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        if files.is_empty() {
            return Ok(ToolOutput::success(format!(
                "No files found in {scope} scope{}",
                if dir.is_empty() { String::new() } else { format!(" under {dir}") }
            )));
        }

        let mut output = format!("Memory files ({scope}):\n");
        for (path, summary) in &files {
            let _ = writeln!(output, "  {path} — {summary}");
        }
        Ok(ToolOutput::success(output))
    }
}
