//! Grep tool - Search file contents

use crate::description::ToolDescriptions;
use crate::security::validate_path_with_confirmed;
use crate::{ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use ignore::WalkBuilder;
use serde_json::{json, Value};
use std::sync::OnceLock;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r#"Search for a pattern in file contents.

Uses regex matching and respects .gitignore. Returns matching lines with file paths and line numbers.

Usage:
- Search all files: {"pattern": "TODO"}
- Search specific dir: {"pattern": "fn main", "path": "/project/src"}
- Filter by file type: {"pattern": "import", "glob": "*.py"}
- Case insensitive: {"pattern": "error", "case_insensitive": true}

Notes:
- Use 'glob' to find files by name pattern
- Use 'symbols' for finding code definitions
- Respects .gitignore by default
- Max 100 results by default (use max_results to change)"#;

/// Content search tool
pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("grep", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search in"
                },
                "glob": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., '*.rs')"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case insensitive search (default: false)"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results (default: 100)"
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

        // Validate and resolve search path with security checks
        let search_path = if let Some(p) = crate::optional_str(&params, "path") {
            // Validate the path for security (path traversal, sensitive files, working dir)
            validate_path_with_confirmed(p, ctx.working_dir(), ctx.confirmed_paths())?
        } else {
            ctx.working_dir().to_path_buf()
        };

        let glob_pattern = crate::optional_str(&params, "glob");
        let case_insensitive = crate::optional_bool(&params, "case_insensitive", false);
        let max_results = crate::optional_usize(&params, "max_results", 100);

        // Build regex
        let regex_pattern =
            if case_insensitive { format!("(?i){pattern}") } else { pattern.to_string() };

        let regex = regex::Regex::new(&regex_pattern)
            .map_err(|e| ToolError::InvalidParams(format!("Invalid regex: {e}")))?;

        // Build glob matcher if provided
        let glob_matcher = glob_pattern.and_then(|g| glob::Pattern::new(g).ok());

        let working_dir = ctx.working_dir().to_path_buf();
        let search_path_owned = search_path.clone();

        // Run the synchronous directory walk + file read in a blocking thread
        // to avoid blocking the tokio runtime
        let results = tokio::task::spawn_blocking(move || {
            let mut results: Vec<String> = Vec::new();
            let walker =
                WalkBuilder::new(&search_path_owned).hidden(false).git_ignore(true).build();

            for entry in walker.filter_map(std::result::Result::ok) {
                if results.len() >= max_results {
                    break;
                }

                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                // Check glob match
                if let Some(ref glob) = glob_matcher {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if !glob.matches(name) {
                            continue;
                        }
                    }
                }

                // Try to read file (skip binary files)
                let Ok(content) = std::fs::read_to_string(path) else { continue };

                // Search for matches
                for (line_num, line) in content.lines().enumerate() {
                    if results.len() >= max_results {
                        break;
                    }

                    if regex.is_match(line) {
                        let relative_path =
                            path.strip_prefix(&working_dir).unwrap_or(path).display();
                        results.push(format!("{}:{}:{}", relative_path, line_num + 1, line));
                    }
                }
            }
            results
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed(format!("Search task failed: {e}")))?;

        if results.is_empty() {
            Ok(ToolOutput::success("No matches found"))
        } else {
            let count = results.len();
            let output = results.join("\n");
            let mut result = ToolOutput::success(output);
            result.data = Some(json!({"match_count": count}));
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;
    use std::io::Write;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_grep_basic() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "hello world").unwrap();
        writeln!(file, "goodbye world").unwrap();
        writeln!(file, "hello again").unwrap();

        let tool = GrepTool;
        let ctx = ToolContext { working_dir: dir.path().to_path_buf(), ..Default::default() };

        let result = tool
            .execute(
                json!({
                    "pattern": "hello",
                    "path": "test.txt"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(!output.is_error);
        assert!(output.content.contains("hello world"));
        assert!(output.content.contains("hello again"));
    }

    #[tokio::test]
    async fn test_grep_case_insensitive() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "Hello World").unwrap();
        writeln!(file, "hello world").unwrap();

        let tool = GrepTool;
        let ctx = ToolContext { working_dir: dir.path().to_path_buf(), ..Default::default() };

        let result = tool
            .execute(
                json!({
                    "pattern": "HELLO",
                    "case_insensitive": true
                }),
                &ctx,
            )
            .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(!output.is_error);
        // Should match both lines
        assert!(output.content.contains("Hello World"));
        assert!(output.content.contains("hello world"));
    }

    #[tokio::test]
    async fn test_grep_no_matches() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, "foo bar").unwrap();

        let tool = GrepTool;
        let ctx = ToolContext { working_dir: dir.path().to_path_buf(), ..Default::default() };

        let result = tool
            .execute(
                json!({
                    "pattern": "xyz"
                }),
                &ctx,
            )
            .await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(!output.is_error);
        assert_eq!(output.content, "No matches found");
    }
}
