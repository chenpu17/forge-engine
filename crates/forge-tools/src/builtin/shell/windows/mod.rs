//! Windows shell tools

mod dangerous;
mod powershell;

pub use dangerous::WindowsDangerousCommands;
pub use powershell::PowerShellTool;

use super::ShellExecutor;

/// Windows shell executor (PowerShell)
pub struct PowerShellExecutor;

impl ShellExecutor for PowerShellExecutor {
    fn program(&self) -> &str {
        "powershell.exe"
    }

    fn command_arg(&self) -> &str {
        "-EncodedCommand"
    }

    fn extra_args(&self) -> Vec<&str> {
        vec!["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass"]
    }

    fn use_encoded_command(&self) -> bool {
        true
    }

    /// Encode command as UTF-16LE Base64 for PowerShell -EncodedCommand
    fn encode_command(&self, cmd: &str) -> String {
        super::common::encode_powershell_command(cmd)
    }
}
