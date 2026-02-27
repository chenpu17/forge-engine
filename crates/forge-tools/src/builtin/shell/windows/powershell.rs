//! PowerShell tool - Execute shell commands on Windows systems

use super::WindowsDangerousCommands;
use crate::builtin::shell::common::encode_powershell_command;
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
const FALLBACK_DESCRIPTION: &str = r#"Execute a PowerShell command on Windows.

Uses: powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand

Usage:
- Run command: {"command": "Get-ChildItem"}
- With timeout: {"command": "npm install", "timeout_ms": 60000}

Notes:
- Dangerous commands (Remove-Item -Recurse, etc.) require confirmation
- Default timeout: 120 seconds (use timeout_ms to change)
- Working directory is the project root
- ExecutionPolicy Bypass is used to ensure script execution works
- In restricted enterprise environments, this may trigger security alerts
- For file operations, prefer read/write/edit tools"#;

/// PowerShell command execution tool
pub struct PowerShellTool;

impl Default for PowerShellTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerShellTool {
    /// Create a new PowerShellTool
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Encode command as UTF-16LE Base64 for -EncodedCommand
    ///
    /// Delegates to the common implementation to avoid code duplication.
    fn encode_command(command: &str) -> String {
        encode_powershell_command(command)
    }
}

#[async_trait]
impl Tool for PowerShellTool {
    fn name(&self) -> &str {
        "powershell"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("powershell", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The PowerShell command to execute"
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

        // Check plan mode - block all commands
        if ctx.plan_mode_active() {
            return Err(ToolError::PermissionDenied(
                "PowerShell is not available in plan mode. Use exit_plan_mode to enable all tools."
                    .to_string(),
            ));
        }

        // Check bash_readonly mode - block write operations
        if ctx.bash_readonly() && WindowsDangerousCommands::is_write_command(command) {
            return Err(ToolError::PermissionDenied(
                "PowerShell is in read-only mode. Write operations are blocked.".to_string(),
            ));
        }

        // Get timeout from params or context
        let timeout_ms = crate::optional_u64(&params, "timeout_ms", ctx.timeout_secs() * 1000);
        let timeout_secs = timeout_ms / 1000;

        tracing::debug!(command = %command, timeout = %timeout_secs, "Executing PowerShell command");

        // Encode command as UTF-16LE Base64
        let encoded = Self::encode_command(command);

        // Create the command
        let mut child = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
                &encoded,
            ])
            .current_dir(ctx.working_dir())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed(format!("Failed to spawn process: {}", e)))?;

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
                                return Err(ToolError::ExecutionFailed(format!("Read error: {}", e)));
                            }
                        }
                    }
                    line = stderr_reader.next_line() => {
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
            let _ = child.kill().await;
            return Err(ToolError::Timeout(timeout_secs));
        }

        result.map_err(|_| ToolError::Timeout(timeout_secs))??;

        // Wait for the process to complete
        let status = child
            .wait()
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("Wait error: {}", e)))?;

        let exit_code = status.code().unwrap_or(-1);

        // Combine output
        let final_output = if stderr_output.is_empty() {
            output
        } else {
            format!("{}\n[stderr]\n{}", output, stderr_output)
        };

        if exit_code == 0 {
            Ok(ToolOutput::success(final_output.trim_end()))
        } else {
            Ok(ToolOutput::error(format!("Exit code: {}\n{}", exit_code, final_output.trim_end())))
        }
    }

    fn confirmation_level(&self, params: &Value) -> ConfirmationLevel {
        if let Some(cmd) = params["command"].as_str() {
            if WindowsDangerousCommands::is_very_dangerous(cmd) {
                return ConfirmationLevel::Dangerous;
            }
            if WindowsDangerousCommands::is_dangerous(cmd) {
                return ConfirmationLevel::Once;
            }
        }
        ConfirmationLevel::Once
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_command_utf16le() {
        // Verify encoding correctness
        let cmd = "Write-Host 'Hello, World'";
        let encoded = PowerShellTool::encode_command(cmd);

        // Decode and verify
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&encoded).unwrap();

        // Each UTF-16 character is 2 bytes (LE)
        assert_eq!(bytes.len() % 2, 0);

        // Reconstruct string
        let utf16: Vec<u16> =
            bytes.chunks(2).map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]])).collect();
        let decoded = String::from_utf16(&utf16).unwrap();
        assert_eq!(decoded, cmd);
    }

    #[test]
    fn test_encode_command_unicode() {
        // Test with Unicode characters
        let cmd = "Write-Host '你好世界'";
        let encoded = PowerShellTool::encode_command(cmd);

        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&encoded).unwrap();
        let utf16: Vec<u16> =
            bytes.chunks(2).map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]])).collect();
        let decoded = String::from_utf16(&utf16).unwrap();
        assert_eq!(decoded, cmd);
    }

    #[test]
    fn test_confirmation_level() {
        let tool = PowerShellTool::new();

        // Very dangerous commands
        assert_eq!(
            tool.confirmation_level(&json!({"command": "Remove-Item -Recurse -Force C:\\"})),
            ConfirmationLevel::Dangerous
        );

        // Dangerous commands
        assert_eq!(
            tool.confirmation_level(&json!({"command": "Remove-Item file.txt"})),
            ConfirmationLevel::Once
        );

        // Normal commands
        assert_eq!(
            tool.confirmation_level(&json!({"command": "Get-ChildItem"})),
            ConfirmationLevel::Once
        );
    }
}
