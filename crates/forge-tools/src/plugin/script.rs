//! Script-based tool execution
//!
//! Implements the `Tool` trait for external script plugins.
//! Communication is JSON-in (stdin) / JSON-out (stdout).

use crate::{ConfirmationLevel, Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

/// Plugin manifest loaded from `tool.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Tool name (must be unique, alphanumeric + underscores)
    pub name: String,
    /// Human-readable description for the LLM
    pub description: String,
    /// JSON Schema for parameters
    #[serde(default = "default_params_schema")]
    pub parameters: Value,
    /// Confirmation level: "none", "once", "always", "dangerous"
    #[serde(default)]
    pub confirmation: ConfirmationStr,
    /// Execution timeout in seconds (default: 30)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Maximum output size in bytes before truncation (default: 100KB)
    #[serde(default = "default_max_output")]
    pub max_output_bytes: usize,
}

fn default_params_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {}
    })
}

const fn default_timeout() -> u64 {
    30
}

const fn default_max_output() -> usize {
    100 * 1024 // 100 KB
}

/// String representation of confirmation level for serde
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfirmationStr {
    /// No confirmation required
    #[default]
    None,
    /// One-time confirmation
    Once,
    /// Confirm every invocation
    Always,
    /// Dangerous operation warning
    Dangerous,
}

impl From<&ConfirmationStr> for ConfirmationLevel {
    fn from(s: &ConfirmationStr) -> Self {
        match s {
            ConfirmationStr::None => Self::None,
            ConfirmationStr::Once => Self::Once,
            ConfirmationStr::Always => Self::Always,
            ConfirmationStr::Dangerous => Self::Dangerous,
        }
    }
}

/// A tool backed by an external script
///
/// The script receives JSON parameters on stdin and writes
/// JSON output to stdout. Exit code 0 = success, non-zero = error.
#[derive(Debug, Clone)]
pub struct ScriptTool {
    /// Parsed manifest
    pub manifest: PluginManifest,
    /// Path to the executable script
    pub script_path: PathBuf,
    /// Plugin directory (for relative path resolution)
    pub plugin_dir: PathBuf,
}

impl ScriptTool {
    /// Create a new script tool from manifest and script path
    #[must_use]
    pub const fn new(manifest: PluginManifest, script_path: PathBuf, plugin_dir: PathBuf) -> Self {
        Self { manifest, script_path, plugin_dir }
    }
}

#[async_trait]
impl Tool for ScriptTool {
    fn name(&self) -> &str {
        &self.manifest.name
    }

    fn description(&self) -> &str {
        &self.manifest.description
    }

    fn parameters_schema(&self) -> Value {
        self.manifest.parameters.clone()
    }

    fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
        ConfirmationLevel::from(&self.manifest.confirmation)
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let input = serde_json::to_string(&params)
            .map_err(|e| ToolError::InvalidParams(format!("Failed to serialize params: {e}")))?;

        tracing::debug!(
            tool = %self.manifest.name,
            script = %self.script_path.display(),
            "Executing script plugin"
        );

        // Determine interpreter from file extension
        let (program, args) = detect_interpreter(&self.script_path);

        let mut cmd = Command::new(program);
        for arg in args {
            cmd.arg(arg);
        }

        // Security: clear environment to prevent API key leakage to plugins.
        // Only pass through safe, minimal env vars needed for script execution.
        cmd.arg(&self.script_path)
            .current_dir(ctx.working_dir())
            .env_clear()
            .envs(safe_env_vars())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            ToolError::ExecutionFailed(format!(
                "Failed to spawn plugin '{}': {e}",
                self.manifest.name
            ))
        })?;

        // Write params to stdin then drop to signal EOF
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.as_bytes()).await.map_err(|e| {
                ToolError::ExecutionFailed(format!("Failed to write to plugin stdin: {e}"))
            })?;
        }

        // Take stdout/stderr handles before waiting
        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        // Concurrently read stdout/stderr and wait for the child to avoid pipe
        // buffer deadlock (the child may block writing to a full pipe if we
        // don't drain it while waiting).
        let timeout = std::time::Duration::from_secs(self.manifest.timeout_secs);

        let stdout_fut = read_handle(stdout_handle);
        let stderr_fut = read_stderr_handle(stderr_handle);
        let wait_fut = child.wait();

        let result = tokio::time::timeout(timeout, async {
            tokio::try_join!(
                async { Ok::<_, std::io::Error>(stdout_fut.await) },
                async { Ok::<_, std::io::Error>(stderr_fut.await) },
                wait_fut,
            )
        })
        .await;

        let (stdout_bytes, stderr_bytes, status) = match result {
            Ok(Ok((stdout_bytes, stderr_bytes, status))) => (stdout_bytes, stderr_bytes, status),
            Ok(Err(e)) => {
                kill_process_group(&child);
                return Err(ToolError::ExecutionFailed(format!(
                    "Plugin '{}' execution error: {e}",
                    self.manifest.name
                )));
            }
            Err(_) => {
                kill_process_group(&child);
                return Err(ToolError::Timeout(self.manifest.timeout_secs));
            }
        };

        let stdout = truncate_output(&stdout_bytes, self.manifest.max_output_bytes);
        let stderr = String::from_utf8_lossy(&stderr_bytes);

        let exit_code = status.code().unwrap_or(-1);

        if exit_code == 0 {
            // Try to parse as JSON with "content" field, fall back to raw text
            if let Ok(parsed) = serde_json::from_str::<Value>(&stdout) {
                if let Some(content) = parsed.get("content").and_then(|c| c.as_str()) {
                    return Ok(ToolOutput::success(content));
                }
                // Return pretty-printed JSON
                let pretty =
                    serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| stdout.clone());
                return Ok(ToolOutput::success(pretty));
            }
            Ok(ToolOutput::success(stdout.trim_end()))
        } else {
            let error_msg = if stderr.is_empty() {
                format!("Plugin '{}' exited with code {exit_code}\n{stdout}", self.manifest.name)
            } else {
                format!("Plugin '{}' exited with code {exit_code}\n{stderr}", self.manifest.name)
            };
            Ok(ToolOutput::error(error_msg.trim_end()))
        }
    }
}

/// Detect the interpreter for a script based on file extension
fn detect_interpreter(path: &Path) -> (&'static str, Vec<&'static str>) {
    match path.extension().and_then(|e| e.to_str()) {
        Some("py") => ("python3", vec![]),
        Some("rb") => ("ruby", vec![]),
        Some("js") => ("node", vec![]),
        Some("ts") => ("npx", vec!["tsx"]),
        Some("zsh") => ("zsh", vec![]),
        _ => ("bash", vec![]), // Default to bash (includes .sh, .bash, and unknown)
    }
}

/// Kill a child process and its descendants (best-effort).
///
/// On Unix, first tries `killpg` (process group kill), then falls back to
/// `kill` on the individual PID. The `killpg` may fail if the child didn't
/// call `setpgid` (which we can't do without `unsafe`), so the fallback
/// ensures the child itself is always killed.
fn kill_process_group(child: &tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            use nix::sys::signal::{self, Signal};
            use nix::unistd::Pid;
            let nix_pid = Pid::from_raw(pid.cast_signed());
            // Try process group kill first (works if child has its own PGID)
            if signal::killpg(nix_pid, Signal::SIGKILL).is_err() {
                // Fallback: kill just the child process
                let _ = signal::kill(nix_pid, Signal::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child;
    }
}

/// Return a minimal set of safe environment variables for plugin execution.
///
/// We clear the environment to prevent leaking secrets (API keys, tokens)
/// and only pass through variables needed for basic script operation.
fn safe_env_vars() -> Vec<(&'static str, String)> {
    let mut vars = Vec::new();
    for key in
        &["PATH", "HOME", "USER", "LANG", "LC_ALL", "LC_CTYPE", "SHELL", "TERM", "TMPDIR", "TMP"]
    {
        if let Ok(val) = std::env::var(key) {
            vars.push((*key, val));
        }
    }
    vars
}

/// Read all bytes from an optional child process handle
async fn read_handle(handle: Option<tokio::process::ChildStdout>) -> Vec<u8> {
    if let Some(mut h) = handle {
        let mut buf = Vec::new();
        let _ = h.read_to_end(&mut buf).await;
        buf
    } else {
        Vec::new()
    }
}

/// Read all bytes from an optional stderr handle
async fn read_stderr_handle(handle: Option<tokio::process::ChildStderr>) -> Vec<u8> {
    if let Some(mut h) = handle {
        let mut buf = Vec::new();
        let _ = h.read_to_end(&mut buf).await;
        buf
    } else {
        Vec::new()
    }
}

/// Truncate output to max bytes, preserving UTF-8 boundaries.
///
/// Operates on raw bytes first to avoid allocating a potentially huge
/// `String` before truncation.
fn truncate_output(bytes: &[u8], max_bytes: usize) -> String {
    if bytes.len() <= max_bytes {
        return String::from_utf8_lossy(bytes).into_owned();
    }

    // Truncate raw bytes, then find a valid UTF-8 boundary
    let mut end = max_bytes;
    while end > 0 && !is_utf8_char_boundary(bytes[end]) {
        end -= 1;
    }

    let truncated = String::from_utf8_lossy(&bytes[..end]);
    format!("{truncated}...\n[output truncated at {max_bytes} bytes]")
}

/// Check if a byte is a valid UTF-8 character boundary.
const fn is_utf8_char_boundary(b: u8) -> bool {
    // In UTF-8, continuation bytes have the pattern 10xxxxxx (0x80..0xBF).
    // A char boundary is any byte that is NOT a continuation byte.
    (b.cast_signed()) >= -0x40 // equivalent to b < 128 || b >= 192
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_deserialization() {
        let json = r#"{
            "name": "my_tool",
            "description": "A test tool",
            "parameters": {
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                }
            },
            "confirmation": "once",
            "timeout_secs": 15
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).expect("parse manifest");
        assert_eq!(manifest.name, "my_tool");
        assert_eq!(manifest.timeout_secs, 15);
        assert!(matches!(manifest.confirmation, ConfirmationStr::Once));
    }

    #[test]
    fn test_manifest_defaults() {
        let json = r#"{
            "name": "minimal",
            "description": "Minimal tool"
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).expect("parse manifest");
        assert_eq!(manifest.timeout_secs, 30);
        assert_eq!(manifest.max_output_bytes, 100 * 1024);
        assert!(matches!(manifest.confirmation, ConfirmationStr::None));
    }

    #[test]
    fn test_confirmation_conversion() {
        assert_eq!(ConfirmationLevel::from(&ConfirmationStr::None), ConfirmationLevel::None);
        assert_eq!(ConfirmationLevel::from(&ConfirmationStr::Once), ConfirmationLevel::Once);
        assert_eq!(
            ConfirmationLevel::from(&ConfirmationStr::Dangerous),
            ConfirmationLevel::Dangerous
        );
    }

    #[test]
    fn test_detect_interpreter() {
        assert_eq!(detect_interpreter(&PathBuf::from("tool.py")).0, "python3");
        assert_eq!(detect_interpreter(&PathBuf::from("tool.sh")).0, "bash");
        assert_eq!(detect_interpreter(&PathBuf::from("tool.js")).0, "node");
        assert_eq!(detect_interpreter(&PathBuf::from("tool.rb")).0, "ruby");
        assert_eq!(detect_interpreter(&PathBuf::from("tool")).0, "bash");
    }

    #[test]
    fn test_truncate_output() {
        let short = b"hello";
        assert_eq!(truncate_output(short, 100), "hello");

        let long = b"hello world this is a long string";
        let result = truncate_output(long, 10);
        assert!(result.contains("..."));
        assert!(result.contains("truncated"));
    }

    #[test]
    fn test_script_tool_properties() {
        let manifest = PluginManifest {
            name: "test_plugin".to_string(),
            description: "A test plugin".to_string(),
            parameters: serde_json::json!({"type": "object"}),
            confirmation: ConfirmationStr::Once,
            timeout_secs: 10,
            max_output_bytes: 1024,
        };

        let tool = ScriptTool::new(manifest, PathBuf::from("/tmp/tool.sh"), PathBuf::from("/tmp"));

        assert_eq!(tool.name(), "test_plugin");
        assert_eq!(tool.description(), "A test plugin");
        assert_eq!(tool.confirmation_level(&serde_json::json!({})), ConfirmationLevel::Once);
    }

    #[tokio::test]
    async fn test_script_tool_execute_echo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script_path = dir.path().join("tool.sh");
        std::fs::write(&script_path, "#!/bin/bash\ncat\n").expect("write script");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }

        let manifest = PluginManifest {
            name: "echo_tool".to_string(),
            description: "Echoes input".to_string(),
            parameters: default_params_schema(),
            confirmation: ConfirmationStr::None,
            timeout_secs: 5,
            max_output_bytes: 1024,
        };

        let tool = ScriptTool::new(manifest, script_path, dir.path().to_path_buf());
        let ctx = crate::ToolContext::default();
        let result = tool.execute(serde_json::json!({"hello": "world"}), &ctx).await;

        assert!(result.is_ok());
        let output = result.expect("execute");
        assert!(!output.is_error);
        // The script cats stdin, so output should contain the JSON
        assert!(output.content.contains("hello"));
    }

    #[tokio::test]
    async fn test_script_tool_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script_path = dir.path().join("slow.sh");
        std::fs::write(&script_path, "#!/bin/bash\nsleep 60\n").expect("write script");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }

        let manifest = PluginManifest {
            name: "slow_tool".to_string(),
            description: "Slow tool".to_string(),
            parameters: default_params_schema(),
            confirmation: ConfirmationStr::None,
            timeout_secs: 1,
            max_output_bytes: 1024,
        };

        let tool = ScriptTool::new(manifest, script_path, dir.path().to_path_buf());
        let ctx = crate::ToolContext::default();
        let result = tool.execute(serde_json::json!({}), &ctx).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ToolError::Timeout(1)));
    }

    #[tokio::test]
    async fn test_script_tool_nonzero_exit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script_path = dir.path().join("fail.sh");
        std::fs::write(&script_path, "#!/bin/bash\necho 'bad input' >&2\nexit 1\n")
            .expect("write script");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }

        let manifest = PluginManifest {
            name: "fail_tool".to_string(),
            description: "Failing tool".to_string(),
            parameters: default_params_schema(),
            confirmation: ConfirmationStr::None,
            timeout_secs: 5,
            max_output_bytes: 1024,
        };

        let tool = ScriptTool::new(manifest, script_path, dir.path().to_path_buf());
        let ctx = crate::ToolContext::default();
        let result = tool.execute(serde_json::json!({}), &ctx).await;

        assert!(result.is_ok());
        let output = result.expect("execute");
        assert!(output.is_error);
        assert!(output.content.contains("bad input"));
    }

    #[tokio::test]
    async fn test_script_tool_json_content_field() {
        let dir = tempfile::tempdir().expect("tempdir");
        let script_path = dir.path().join("json_tool.sh");
        std::fs::write(&script_path, "#!/bin/bash\necho '{\"content\": \"extracted value\"}'\n")
            .expect("write script");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }

        let manifest = PluginManifest {
            name: "json_tool".to_string(),
            description: "JSON output tool".to_string(),
            parameters: default_params_schema(),
            confirmation: ConfirmationStr::None,
            timeout_secs: 5,
            max_output_bytes: 1024,
        };

        let tool = ScriptTool::new(manifest, script_path, dir.path().to_path_buf());
        let ctx = crate::ToolContext::default();
        let result = tool.execute(serde_json::json!({}), &ctx).await;

        assert!(result.is_ok());
        let output = result.expect("execute");
        assert!(!output.is_error);
        assert_eq!(output.content, "extracted value");
    }
}
