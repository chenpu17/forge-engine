//! Bash tool - Execute shell commands on Unix systems

use super::UnixDangerousCommands;
use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use serde_json::{json, Value};
use std::process::Stdio;
use std::sync::OnceLock;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r#"Execute a bash command in the shell.

Runs commands with output capture. Use for system operations, builds, git, and other shell tasks.

Usage:
- Run command: {"command": "ls -la"}
- With timeout: {"command": "npm install", "timeout_ms": 60000}

Notes:
- Dangerous commands (rm -rf, sudo) require confirmation
- Default timeout: 120 seconds (use timeout_ms to change)
- Working directory is the project root
- Use for git, build tools, and system commands
- For file operations, prefer read/write/edit tools"#;

/// Bash command execution tool
pub struct BashTool;

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

impl BashTool {
    /// Create a new `BashTool`
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for BashTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("bash", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default: 120000)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let command = crate::required_str(&params, "command")?;

        // Check plan mode - block all bash commands
        if ctx.plan_mode_active() {
            return Err(ToolError::PermissionDenied(
                "Bash is not available in plan mode. Use exit_plan_mode to enable all tools."
                    .to_string(),
            ));
        }

        // Check bash_readonly mode - block write operations
        if ctx.bash_readonly() && UnixDangerousCommands::is_write_command(command) {
            return Err(ToolError::PermissionDenied(
                "Bash is in read-only mode. Write operations (rm, mv, cp, >, >>, tee, etc.) are blocked.".to_string()
            ));
        }

        // Get timeout from params or context
        let timeout_ms = crate::optional_u64(&params, "timeout_ms", ctx.timeout_secs() * 1000);
        let timeout_secs = timeout_ms / 1000;

        tracing::debug!(command = %command, timeout = %timeout_secs, "Executing bash command");

        // Create the command
        let mut child = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(ctx.working_dir())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to spawn process: {e}")))?;

        // Set up readers for stdout and stderr
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::ExecutionFailed("Failed to capture stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::ExecutionFailed("Failed to capture stderr".to_string()))?;

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut output = String::new();
        let mut stderr_output = String::new();

        // Read output with timeout
        let timeout_duration = std::time::Duration::from_secs(timeout_secs);

        let result = tokio::time::timeout(timeout_duration, async {
            loop {
                tokio::select! {
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                output.push_str(&line);
                                output.push('\n');
                            }
                            Ok(None) => break,
                            Err(e) => {
                                return Err(ToolError::ExecutionFailed(format!("Read error: {e}")));
                            }
                        }
                    }
                    line = stderr_reader.next_line() => {
                        #[allow(clippy::match_same_arms)]
                        match line {
                            Ok(Some(line)) => {
                                stderr_output.push_str(&line);
                                stderr_output.push('\n');
                            }
                            Ok(None) => {}
                            Err(_) => {}
                        }
                    }
                }
            }
            Ok(())
        })
        .await;

        // Handle timeout
        if result.is_err() {
            // Kill the process on timeout
            let _ = child.kill().await;
            return Err(ToolError::Timeout(timeout_secs));
        }

        result.map_err(|_| ToolError::Timeout(timeout_secs))??;

        // Wait for the process to complete
        let status = child
            .wait()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Wait error: {e}")))?;

        let exit_code = status.code().unwrap_or(-1);

        // Combine output
        let final_output = if stderr_output.is_empty() {
            output
        } else {
            format!("{output}\n[stderr]\n{stderr_output}")
        };

        if exit_code == 0 {
            Ok(ToolOutput::success(final_output.trim_end()))
        } else {
            Ok(ToolOutput::error(format!("Exit code: {exit_code}\n{}", final_output.trim_end())))
        }
    }

    fn confirmation_level(&self, params: &Value) -> ConfirmationLevel {
        if let Some(cmd) = params["command"].as_str() {
            if UnixDangerousCommands::is_very_dangerous(cmd) {
                return ConfirmationLevel::Dangerous;
            }
            if UnixDangerousCommands::is_dangerous(cmd) {
                return ConfirmationLevel::Once;
            }
        }
        // All bash commands require at least one-time confirmation
        ConfirmationLevel::Once
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    #[tokio::test]
    async fn test_bash_echo() {
        let tool = BashTool::new();
        let ctx = ToolContext::default();
        let result = tool.execute(json!({"command": "echo hello"}), &ctx).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(!output.is_error);
        assert_eq!(output.content.trim(), "hello");
    }

    #[tokio::test]
    async fn test_bash_pwd() {
        let tool = BashTool::new();
        let ctx = ToolContext::default();
        let result = tool.execute(json!({"command": "pwd"}), &ctx).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(!output.is_error);
        assert!(!output.content.is_empty());
    }

    #[tokio::test]
    async fn test_bash_exit_code() {
        let tool = BashTool::new();
        let ctx = ToolContext::default();
        let result = tool.execute(json!({"command": "exit 1"}), &ctx).await;

        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.is_error);
        assert!(output.content.contains("Exit code: 1"));
    }

    #[test]
    fn test_confirmation_level() {
        let tool = BashTool::new();

        // Very dangerous commands
        assert_eq!(
            tool.confirmation_level(&json!({"command": "rm -rf /"})),
            ConfirmationLevel::Dangerous
        );

        // Dangerous commands
        assert_eq!(
            tool.confirmation_level(&json!({"command": "sudo apt update"})),
            ConfirmationLevel::Once
        );

        // Normal commands still need confirmation (Once)
        assert_eq!(tool.confirmation_level(&json!({"command": "ls -la"})), ConfirmationLevel::Once);
    }
}
