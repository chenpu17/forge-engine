//! Common types and traits for shell tools
//!
//! Provides unified parameter structure and executor trait.

use serde::Deserialize;

/// Default timeout in milliseconds (2 minutes)
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// Unified shell parameters (ensures schema consistency across platforms)
#[derive(Debug, Deserialize)]
pub struct ShellParams {
    /// The command to execute
    pub command: String,

    /// Timeout in milliseconds (default: 120000)
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,

    /// Brief description (used for background tasks)
    #[serde(default)]
    pub description: Option<String>,

    /// Run command in background
    #[serde(default)]
    pub run_in_background: bool,
}

const fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

impl ShellParams {
    /// Get timeout in seconds
    #[must_use]
    pub const fn timeout_secs(&self) -> u64 {
        self.timeout_ms / 1000
    }
}

/// Shell executor trait for background task manager
pub trait ShellExecutor: Send + Sync {
    /// Get the shell program name (e.g., "bash", "powershell.exe")
    fn program(&self) -> &str;

    /// Get the command argument flag (e.g., "-c", "-Command")
    fn command_arg(&self) -> &str;

    /// Get extra arguments (e.g., `-NoProfile` for `PowerShell`)
    fn extra_args(&self) -> Vec<&str> {
        vec![]
    }

    /// Encode a command string for execution
    /// Default implementation returns the command as-is
    /// `PowerShell` overrides this to use `-EncodedCommand` with UTF-16LE Base64
    fn encode_command(&self, cmd: &str) -> String {
        cmd.to_string()
    }

    /// Whether to use encoded command mode
    /// If true, uses `encode_command()` and `-EncodedCommand` instead of -Command
    fn use_encoded_command(&self) -> bool {
        false
    }
}

/// Get the unified parameters schema for shell tools
#[allow(dead_code)]
#[must_use]
pub fn shell_parameters_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "The command to execute"
            },
            "timeout_ms": {
                "type": "integer",
                "description": "Timeout in milliseconds (default: 120000)"
            },
            "description": {
                "type": "string",
                "description": "Brief description (used for background tasks)"
            },
            "run_in_background": {
                "type": "boolean",
                "description": "Run command in background, returns task ID immediately"
            }
        },
        "required": ["command"]
    })
}

/// Encode a command as UTF-16LE Base64 for `PowerShell` `-EncodedCommand`
#[allow(dead_code)]
#[must_use]
pub fn encode_powershell_command(cmd: &str) -> String {
    use base64::Engine;
    let utf16le_bytes: Vec<u8> = cmd.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(&utf16le_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_params_default_timeout() {
        let params: ShellParams = serde_json::from_str(r#"{"command": "test"}"#).unwrap();
        assert_eq!(params.timeout_ms, 120_000);
        assert_eq!(params.timeout_secs(), 120);
    }

    #[test]
    fn test_shell_params_custom_timeout() {
        let params: ShellParams =
            serde_json::from_str(r#"{"command": "test", "timeout_ms": 60000}"#).unwrap();
        assert_eq!(params.timeout_ms, 60_000);
        assert_eq!(params.timeout_secs(), 60);
    }

    #[test]
    fn test_encode_powershell_command_ascii() {
        let cmd = "Write-Host 'Hello'";
        let encoded = encode_powershell_command(cmd);

        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&encoded).unwrap();
        assert_eq!(bytes.len() % 2, 0);

        let utf16: Vec<u16> =
            bytes.chunks(2).map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]])).collect();
        let decoded = String::from_utf16(&utf16).unwrap();
        assert_eq!(decoded, cmd);
    }

    #[test]
    fn test_encode_powershell_command_unicode() {
        let cmd = "Write-Host '你好世界'";
        let encoded = encode_powershell_command(cmd);

        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&encoded).unwrap();
        let utf16: Vec<u16> =
            bytes.chunks(2).map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]])).collect();
        let decoded = String::from_utf16(&utf16).unwrap();
        assert_eq!(decoded, cmd);
    }

    #[test]
    fn test_encode_powershell_command_special_chars() {
        let cmd =
            r#"Get-ChildItem -Path "C:\Program Files" | Where-Object { $_.Name -match '.*' }"#;
        let encoded = encode_powershell_command(cmd);

        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&encoded).unwrap();
        let utf16: Vec<u16> =
            bytes.chunks(2).map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]])).collect();
        let decoded = String::from_utf16(&utf16).unwrap();
        assert_eq!(decoded, cmd);
    }

    #[test]
    fn test_encode_powershell_command_empty() {
        let cmd = "";
        let encoded = encode_powershell_command(cmd);
        assert_eq!(encoded, "");
    }
}
