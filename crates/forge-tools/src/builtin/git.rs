//! Git tool for version control operations
//!
//! This tool provides safe, structured access to common Git operations.
//! It enforces safety rules to prevent destructive operations.

use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::process::Command;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r#"Execute Git version control operations safely.

Supported operations:
- status: Show working tree status
- log: Show commit logs (use --oneline for compact view)
- diff: Show changes between commits, commit and working tree, etc.
- add: Add file contents to the index
- commit: Record changes to the repository (requires -m "message")
- show: Show various types of objects
- branch: List, create, or delete branches
- checkout: Switch branches or restore working tree files
- stash: Stash changes in a dirty working directory
- ls-remote: List remote references
- remote: Show remote repositories
- fetch: Fetch from remote
- blame: Show commit history for a file
- tag: List tags

Safety rules:
- Destructive operations (push --force, reset --hard) are blocked
- Use bash tool for operations not listed here
- Commit messages are required for commits

Examples:
- {"operation": "status"}
- {"operation": "log", "args": ["--oneline", "-10"]}
- {"operation": "diff", "args": ["HEAD~1"]}
- {"operation": "add", "args": ["."]}
- {"operation": "commit", "args": ["-m", "Fix bug in parser"]}"#;

/// Git operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitOperation {
    /// Show working tree status
    Status,
    /// Show commit logs
    Log,
    /// Show changes between commits
    Diff,
    /// Add file contents to the index
    Add,
    /// Record changes to the repository
    Commit,
    /// Show various types of objects
    Show,
    /// List, create, or delete branches
    Branch,
    /// Switch branches or restore working tree files
    Checkout,
    /// Stash changes in a dirty working directory
    Stash,
    /// List remote references
    LsRemote,
    /// Show remote repositories
    Remote,
    /// Fetch from remote (safe, read-only)
    Fetch,
    /// Show commit history for a file
    Blame,
    /// List tags
    Tag,
}

impl GitOperation {
    /// Parse operation from string
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "status" => Some(Self::Status),
            "log" => Some(Self::Log),
            "diff" => Some(Self::Diff),
            "add" => Some(Self::Add),
            "commit" => Some(Self::Commit),
            "show" => Some(Self::Show),
            "branch" => Some(Self::Branch),
            "checkout" => Some(Self::Checkout),
            "stash" => Some(Self::Stash),
            "ls-remote" | "ls_remote" => Some(Self::LsRemote),
            "remote" => Some(Self::Remote),
            "fetch" => Some(Self::Fetch),
            "blame" => Some(Self::Blame),
            "tag" => Some(Self::Tag),
            _ => None,
        }
    }

    /// Check if operation is read-only (safe)
    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        matches!(
            self,
            Self::Status
                | Self::Log
                | Self::Diff
                | Self::Show
                | Self::Branch
                | Self::LsRemote
                | Self::Remote
                | Self::Fetch
                | Self::Blame
                | Self::Tag
        )
    }

    /// Check if operation modifies the repository
    #[must_use]
    pub const fn is_write(&self) -> bool {
        matches!(self, Self::Add | Self::Commit | Self::Checkout | Self::Stash)
    }

    /// Get the git command name
    #[must_use]
    pub const fn command(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Log => "log",
            Self::Diff => "diff",
            Self::Add => "add",
            Self::Commit => "commit",
            Self::Show => "show",
            Self::Branch => "branch",
            Self::Checkout => "checkout",
            Self::Stash => "stash",
            Self::LsRemote => "ls-remote",
            Self::Remote => "remote",
            Self::Fetch => "fetch",
            Self::Blame => "blame",
            Self::Tag => "tag",
        }
    }
}

/// Git tool for version control operations
pub struct GitTool;

impl GitTool {
    /// Create a new `GitTool`
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Execute a git command
    async fn execute_git(
        &self,
        operation: GitOperation,
        args: &[String],
        working_dir: &std::path::Path,
    ) -> std::result::Result<String, ToolError> {
        // Build command
        let mut cmd = Command::new("git");
        cmd.arg(operation.command());
        cmd.args(args);
        cmd.current_dir(working_dir);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Execute
        let output = cmd
            .output()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to execute git: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if output.status.success() {
            if stdout.is_empty() && !stderr.is_empty() {
                // Some git commands output to stderr even on success
                Ok(stderr)
            } else {
                Ok(stdout)
            }
        } else {
            Err(ToolError::ExecutionFailed(format!(
                "Git {} failed: {}",
                operation.command(),
                if stderr.is_empty() { &stdout } else { &stderr }
            )))
        }
    }

    /// Validate commit message
    fn validate_commit_message(message: &str) -> std::result::Result<(), ToolError> {
        if message.trim().is_empty() {
            return Err(ToolError::InvalidParams("Commit message cannot be empty".to_string()));
        }
        if message.len() > 5000 {
            return Err(ToolError::InvalidParams(
                "Commit message too long (max 5000 characters)".to_string(),
            ));
        }
        Ok(())
    }

    /// Check for dangerous arguments
    fn check_dangerous_args(args: &[String]) -> std::result::Result<(), ToolError> {
        let dangerous_patterns =
            ["--force", "-f", "--hard", "--no-verify", "--skip-hooks", "--amend"];

        for arg in args {
            for pattern in &dangerous_patterns {
                if arg == *pattern || arg.starts_with(&format!("{pattern}=")) {
                    return Err(ToolError::PermissionDenied(format!(
                        "Dangerous argument '{arg}' is not allowed. Use bash tool if you need this.",
                    )));
                }
            }
        }
        Ok(())
    }
}

impl Default for GitTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for GitTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "git"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("git", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": [
                        "status", "log", "diff", "add", "commit", "show",
                        "branch", "checkout", "stash", "ls-remote", "remote",
                        "fetch", "blame", "tag"
                    ],
                    "description": "The git operation to perform"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Additional arguments for the git command"
                }
            },
            "required": ["operation"]
        })
    }

    fn confirmation_level(&self, params: &Value) -> ConfirmationLevel {
        let operation =
            params.get("operation").and_then(|v| v.as_str()).and_then(GitOperation::parse);

        match operation {
            Some(op) if op.is_read_only() => ConfirmationLevel::None,
            _ => ConfirmationLevel::Once,
        }
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let operation_str = crate::required_str(&params, "operation")?;

        let operation = GitOperation::parse(operation_str).ok_or_else(|| {
            ToolError::InvalidParams(format!(
                "Unknown git operation: '{operation_str}'. Supported: status, log, diff, add, commit, show, branch, checkout, stash, ls-remote, remote, fetch, blame, tag"
            ))
        })?;

        // Check plan mode - block write operations
        if ctx.plan_mode_active() && operation.is_write() {
            return Err(ToolError::PermissionDenied(format!(
                "Git {} is not available in plan mode. Use exit_plan_mode to enable all tools.",
                operation.command()
            )));
        }

        // Parse args using helper
        let args = crate::string_array(&params, "args");

        // Check for dangerous arguments
        Self::check_dangerous_args(&args)?;

        // Special validation for commit
        if operation == GitOperation::Commit {
            // Check for -m flag
            let has_message = args.iter().any(|a| a == "-m" || a.starts_with("-m"));
            if !has_message {
                return Err(ToolError::InvalidParams(
                    "Commit requires a message. Use: {\"operation\": \"commit\", \"args\": [\"-m\", \"Your message\"]}".to_string(),
                ));
            }

            // Find and validate the message
            let mut found_m = false;
            for arg in &args {
                if found_m {
                    Self::validate_commit_message(arg)?;
                    break;
                }
                if arg == "-m" {
                    found_m = true;
                }
            }
        }

        tracing::info!("Executing git {} {:?}", operation.command(), args);

        // Execute git command
        let output = self.execute_git(operation, &args, ctx.working_dir()).await?;

        // Format output
        let result = if output.trim().is_empty() {
            format!("git {} completed successfully (no output)", operation.command())
        } else {
            format!("```\n{}\n```", output.trim())
        };

        Ok(ToolOutput::success(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    #[test]
    fn test_git_operation_from_str() {
        assert_eq!(GitOperation::parse("status"), Some(GitOperation::Status));
        assert_eq!(GitOperation::parse("log"), Some(GitOperation::Log));
        assert_eq!(GitOperation::parse("diff"), Some(GitOperation::Diff));
        assert_eq!(GitOperation::parse("commit"), Some(GitOperation::Commit));
        assert_eq!(GitOperation::parse("ls-remote"), Some(GitOperation::LsRemote));
        assert_eq!(GitOperation::parse("invalid"), None);
    }

    #[test]
    fn test_git_operation_is_read_only() {
        assert!(GitOperation::Status.is_read_only());
        assert!(GitOperation::Log.is_read_only());
        assert!(GitOperation::Diff.is_read_only());
        assert!(!GitOperation::Add.is_read_only());
        assert!(!GitOperation::Commit.is_read_only());
    }

    #[test]
    fn test_git_operation_is_write() {
        assert!(!GitOperation::Status.is_write());
        assert!(GitOperation::Add.is_write());
        assert!(GitOperation::Commit.is_write());
        assert!(GitOperation::Checkout.is_write());
    }

    #[test]
    fn test_validate_commit_message() {
        assert!(GitTool::validate_commit_message("Valid message").is_ok());
        assert!(GitTool::validate_commit_message("").is_err());
        assert!(GitTool::validate_commit_message("   ").is_err());

        // Too long message
        let long_msg = "a".repeat(6000);
        assert!(GitTool::validate_commit_message(&long_msg).is_err());
    }

    #[test]
    fn test_check_dangerous_args() {
        assert!(GitTool::check_dangerous_args(&[]).is_ok());
        assert!(GitTool::check_dangerous_args(&["--oneline".to_string()]).is_ok());
        assert!(GitTool::check_dangerous_args(&["-m".to_string(), "message".to_string()]).is_ok());

        // Dangerous args
        assert!(GitTool::check_dangerous_args(&["--force".to_string()]).is_err());
        assert!(GitTool::check_dangerous_args(&["-f".to_string()]).is_err());
        assert!(GitTool::check_dangerous_args(&["--hard".to_string()]).is_err());
        assert!(GitTool::check_dangerous_args(&["--no-verify".to_string()]).is_err());
        assert!(GitTool::check_dangerous_args(&["--amend".to_string()]).is_err());
    }

    #[test]
    fn test_tool_name() {
        let tool = GitTool::new();
        assert_eq!(tool.name(), "git");
    }

    #[test]
    fn test_tool_schema() {
        let tool = GitTool::new();
        let schema = tool.parameters_schema();

        assert!(schema.get("properties").is_some());
        assert!(schema["properties"].get("operation").is_some());
        assert!(schema["properties"].get("args").is_some());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("operation")));
    }

    #[test]
    fn test_confirmation_level_read_only() {
        let tool = GitTool::new();

        let params = json!({"operation": "status"});
        assert_eq!(tool.confirmation_level(&params), ConfirmationLevel::None);

        let params = json!({"operation": "log"});
        assert_eq!(tool.confirmation_level(&params), ConfirmationLevel::None);

        let params = json!({"operation": "diff"});
        assert_eq!(tool.confirmation_level(&params), ConfirmationLevel::None);
    }

    #[test]
    fn test_confirmation_level_write() {
        let tool = GitTool::new();

        let params = json!({"operation": "add"});
        assert_eq!(tool.confirmation_level(&params), ConfirmationLevel::Once);

        let params = json!({"operation": "commit"});
        assert_eq!(tool.confirmation_level(&params), ConfirmationLevel::Once);
    }

    #[tokio::test]
    async fn test_missing_operation() {
        let tool = GitTool::new();
        let ctx = ToolContext::default();

        let params = json!({});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalid_operation() {
        let tool = GitTool::new();
        let ctx = ToolContext::default();

        let params = json!({"operation": "push"}); // push is not allowed
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_commit_requires_message() {
        let tool = GitTool::new();
        let ctx = ToolContext::default();

        let params = json!({"operation": "commit"});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("message"));
    }

    #[tokio::test]
    async fn test_dangerous_args_blocked() {
        let tool = GitTool::new();
        let ctx = ToolContext::default();

        let params = json!({"operation": "checkout", "args": ["--force", "main"]});
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Dangerous"));
    }

    // Integration test - requires git to be installed
    #[tokio::test]
    async fn test_git_status() {
        let tool = GitTool::new();
        let ctx = ToolContext::default();

        let params = json!({"operation": "status"});
        let result = tool.execute(params, &ctx).await;

        // This might fail if not in a git repo, which is fine
        // We're just testing that the command executes
        match result {
            Ok(output) => {
                assert!(!output.is_error);
            }
            Err(e) => {
                // Expected if not in a git repo
                assert!(e.to_string().contains("git") || e.to_string().contains("repository"));
            }
        }
    }
}
