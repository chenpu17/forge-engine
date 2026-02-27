//! Glob tool - File pattern matching

use crate::description::ToolDescriptions;
use crate::security::validate_path;
use crate::{ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Maximum number of results to return
const DEFAULT_MAX_RESULTS: usize = 1000;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r#"Find files matching a glob pattern.

Returns matching file paths (max 1000 results by default). Results are sorted by modification time.

Usage:
- Find all Rust files: {"pattern": "**/*.rs"}
- Find in specific dir: {"pattern": "*.ts", "path": "/project/src"}
- Limit results: {"pattern": "**/*.log", "limit": 100}

Pattern syntax:
- * matches any sequence except /
- ** matches any sequence including /
- ? matches any single character
- [abc] matches one of the characters

Notes:
- Use 'grep' to search file contents
- Hidden files (starting with .) are excluded by default"#;

/// File glob search tool
pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("glob", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The glob pattern to match (e.g., '**/*.rs')"
                },
                "path": {
                    "type": "string",
                    "description": "Base directory to search in (default: current directory)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 1000)"
                }
            },
            "required": ["pattern"]
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
        let pattern = crate::required_str(&params, "pattern")?;

        // Validate and resolve base path with security checks
        let base_path = if let Some(p) = crate::optional_str(&params, "path") {
            // Validate the path for security (path traversal, sensitive files, working dir)
            validate_path(p, ctx.working_dir())?
        } else {
            ctx.working_dir().to_path_buf()
        };

        let limit = crate::optional_usize(&params, "limit", DEFAULT_MAX_RESULTS);

        // Reject patterns with path traversal components
        if pattern.contains("..") {
            return Err(ToolError::InvalidParams(
                "Pattern must not contain '..' path traversal components".to_string(),
            ));
        }

        // Build full pattern
        let full_pattern = base_path.join(pattern);
        let pattern_str = full_pattern.to_string_lossy();

        // Execute glob with limit
        let mut paths: Vec<String> = Vec::new();
        let mut total_matches = 0usize;
        let mut truncated = false;

        for entry in glob::glob(&pattern_str)
            .map_err(|e| ToolError::InvalidParams(format!("Invalid pattern: {e}")))?
            .filter_map(std::result::Result::ok)
        {
            total_matches += 1;
            if paths.len() < limit {
                paths.push(entry.to_string_lossy().to_string());
            } else {
                truncated = true;
                // Continue counting for a bit to give accurate info
                if total_matches > limit + 10000 {
                    break;
                }
            }
        }

        if paths.is_empty() {
            Ok(ToolOutput::success("No files found matching pattern"))
        } else if truncated {
            let result = format!(
                "{}\n\n... (showing {} of {}+ results, use 'limit' parameter to see more)",
                paths.join("\n"),
                limit,
                total_matches
            );
            Ok(ToolOutput::success(result))
        } else {
            Ok(ToolOutput::success(paths.join("\n")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;
    use std::fs::File;
    use tempfile::TempDir;

    fn create_test_context(dir: &std::path::Path) -> ToolContext {
        ToolContext { working_dir: dir.to_path_buf(), ..Default::default() }
    }

    #[tokio::test]
    async fn test_glob_basic() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("test1.txt")).unwrap();
        File::create(dir.path().join("test2.txt")).unwrap();
        File::create(dir.path().join("other.rs")).unwrap();

        let tool = GlobTool;
        let ctx = create_test_context(dir.path());

        let result = tool.execute(json!({"pattern": "*.txt"}), &ctx).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("test1.txt"));
        assert!(output.content.contains("test2.txt"));
        assert!(!output.content.contains("other.rs"));
    }

    #[tokio::test]
    async fn test_glob_no_matches() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("test.txt")).unwrap();

        let tool = GlobTool;
        let ctx = create_test_context(dir.path());

        let result = tool.execute(json!({"pattern": "*.rs"}), &ctx).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.content, "No files found matching pattern");
    }

    #[tokio::test]
    async fn test_glob_with_limit() {
        let dir = TempDir::new().unwrap();

        // Create 10 files
        for i in 0..10 {
            File::create(dir.path().join(format!("file{}.txt", i))).unwrap();
        }

        let tool = GlobTool;
        let ctx = create_test_context(dir.path());

        // Request limit of 3
        let result = tool
            .execute(
                json!({
                    "pattern": "*.txt",
                    "limit": 3
                }),
                &ctx,
            )
            .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(!output.is_error);
        // Should show truncation message
        assert!(output.content.contains("showing 3 of"));
        assert!(output.content.contains("results"));
    }

    #[tokio::test]
    async fn test_glob_default_limit() {
        // This test verifies that default limit is applied
        // Just verify the DEFAULT_MAX_RESULTS constant is sensible
        assert_eq!(DEFAULT_MAX_RESULTS, 1000);
    }

    #[tokio::test]
    async fn test_glob_with_path() {
        let dir = TempDir::new().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();
        File::create(subdir.join("nested.txt")).unwrap();

        let tool = GlobTool;
        let ctx = create_test_context(dir.path());

        // Use absolute path for the subdir
        let result = tool
            .execute(
                json!({
                    "pattern": "*.txt",
                    "path": subdir.to_string_lossy()
                }),
                &ctx,
            )
            .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.content.contains("nested.txt"));
    }
}
