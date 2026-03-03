//! Memory manage tool - administrative operations on memory files

use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_memory::{MemoryLoader, MemoryScope, MemoryWriter};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::OnceLock;

const FALLBACK_DESCRIPTION: &str = r#"Manage memory files: delete, move, or export.

Actions:
- "delete": Delete a memory file
- "move": Move/rename a memory file
- "export": Export all memory as JSON

Parameters:
- action: "delete" | "move" | "export"
- scope: "user" | "project" (default: "project")
- path: File path (for delete/move)
- to: Destination path (for move)
"#;

/// Memory manage tool
pub struct MemoryManageTool {
    user_dir: PathBuf,
}

impl Default for MemoryManageTool {
    fn default() -> Self {
        Self::new(forge_infra::data_dir().join("memory"))
    }
}

impl MemoryManageTool {
    /// Create a `memory_manage` tool bound to a specific user memory directory.
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

    fn project_dir(ctx: &dyn ToolExecutionContext) -> std::path::PathBuf {
        ctx.working_dir().join(".forge").join("memory")
    }
}

#[async_trait]
impl Tool for MemoryManageTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "memory_manage"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("memory_manage", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["delete", "move", "export"],
                    "description": "The manage action to perform"
                },
                "scope": {
                    "type": "string",
                    "enum": ["user", "project"],
                    "description": "Memory scope (default: project)"
                },
                "path": {
                    "type": "string",
                    "description": "File path (for delete/move)"
                },
                "to": {
                    "type": "string",
                    "description": "Destination path (for move)"
                }
            },
            "required": ["action"]
        })
    }

    fn confirmation_level(&self, params: &Value) -> ConfirmationLevel {
        match params["action"].as_str().unwrap_or("") {
            "delete" | "export" => ConfirmationLevel::Always,
            _ => ConfirmationLevel::Once,
        }
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let action = crate::required_str(&params, "action")?;
        let scope = Self::parse_scope(&params);
        let project_dir = Self::project_dir(ctx);

        match action {
            "delete" => self.execute_delete(&params, scope, &project_dir).await,
            "move" => self.execute_move(&params, scope, &project_dir).await,
            "export" => self.execute_export(scope, &project_dir).await,
            other => Err(ToolError::InvalidParams(format!(
                "Unknown action: {other}. Use delete, move, or export."
            ))),
        }
    }
}

// Helper methods for manage actions
impl MemoryManageTool {
    async fn execute_delete(
        &self,
        params: &Value,
        scope: MemoryScope,
        project_dir: &std::path::Path,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let path = params["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidParams("path is required for delete".into()))?;

        // Reject deletion of index.md
        if path == "index.md" || path.ends_with("/index.md") {
            return Err(ToolError::ExecutionFailed(
                "Cannot delete index.md. It is auto-maintained.".into(),
            ));
        }

        let writer = self.make_writer();
        writer
            .delete_file(scope, Some(project_dir), path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolOutput::success(format!("Deleted memory file: {path} ({scope} scope)")))
    }

    async fn execute_move(
        &self,
        params: &Value,
        scope: MemoryScope,
        project_dir: &std::path::Path,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let from = params["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidParams("path is required for move".into()))?;
        let to = params["to"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidParams("to is required for move".into()))?;

        // Reject moving index.md or moving onto index.md
        if from == "index.md" || from.ends_with("/index.md") {
            return Err(ToolError::ExecutionFailed(
                "Cannot move index.md. It is auto-maintained.".into(),
            ));
        }
        if to == "index.md" || to.ends_with("/index.md") {
            return Err(ToolError::ExecutionFailed(
                "Cannot overwrite index.md. It is auto-maintained.".into(),
            ));
        }

        let writer = self.make_writer();
        let result = writer
            .move_file(scope, Some(project_dir), from, to)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let mut output = format!("Moved: {from} → {to} ({scope} scope)");
        if !result.dangling_refs.is_empty() {
            output.push_str("\nDangling references in: ");
            output.push_str(&result.dangling_refs.join(", "));
        }
        Ok(ToolOutput::success(output))
    }

    async fn execute_export(
        &self,
        scope: MemoryScope,
        project_dir: &std::path::Path,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let loader = MemoryLoader::new(self.user_dir.clone());

        let files = loader
            .list_files_recursive(scope, Some(project_dir))
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        if files.is_empty() {
            return Ok(ToolOutput::success(format!("No memory files to export ({scope} scope).")));
        }

        let mut entries = Vec::new();
        for (path, summary) in &files {
            let file = loader
                .read_file_raw(scope, Some(project_dir), path)
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            if let Some(f) = file {
                entries.push(json!({
                    "path": f.path,
                    "summary": summary,
                    "content": f.content,
                    "updated": f.meta.updated,
                    "tags": f.meta.tags,
                }));
            }
        }

        let export = json!({
            "scope": scope.to_string(),
            "file_count": entries.len(),
            "files": entries,
        });

        Ok(ToolOutput::success(
            serde_json::to_string_pretty(&export).unwrap_or_else(|_| "Export failed".into()),
        ))
    }
}
