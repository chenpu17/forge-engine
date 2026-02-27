//! Shell tool module
//!
//! Provides platform-specific shell execution:
//! - Unix: `BashTool`
//! - Windows: `PowerShellTool`
//! - All platforms: `ShellAlias` (stable alias)

mod alias;
pub mod common;

#[cfg(unix)]
pub mod unix;

#[cfg(windows)]
pub mod windows;

pub use alias::ShellAlias;
pub use common::{ShellExecutor, ShellParams};

use forge_domain::Tool;
use std::sync::Arc;

/// Get the platform-specific shell tool
#[cfg(unix)]
#[must_use]
pub fn get_shell_tool() -> Arc<dyn Tool> {
    Arc::new(unix::BashTool::new())
}

/// Get the platform-specific shell tool (Windows)
#[cfg(windows)]
#[must_use]
pub fn get_shell_tool() -> Arc<dyn Tool> {
    Arc::new(windows::PowerShellTool::new())
}

/// Get the shell executor for background tasks
#[cfg(unix)]
#[must_use]
pub fn get_shell_executor() -> Box<dyn ShellExecutor> {
    Box::new(unix::BashExecutor)
}

/// Get the shell executor for background tasks (Windows)
#[cfg(windows)]
#[must_use]
pub fn get_shell_executor() -> Box<dyn ShellExecutor> {
    Box::new(windows::PowerShellExecutor)
}

/// Get all shell-related tools for this platform.
///
/// Returns `[ShellAlias, platform-specific tool]`.
#[cfg(unix)]
#[must_use]
pub fn get_shell_tools() -> Vec<Arc<dyn Tool>> {
    let bash = Arc::new(unix::BashTool::new());
    vec![
        Arc::new(ShellAlias::new(bash.clone())), // shell alias first
        bash,                                    // bash second
    ]
}

/// Get all shell-related tools for this platform (Windows).
///
/// Returns `[ShellAlias, platform-specific tool]`.
#[cfg(windows)]
#[must_use]
pub fn get_shell_tools() -> Vec<Arc<dyn Tool>> {
    let ps = Arc::new(windows::PowerShellTool::new());
    vec![
        Arc::new(ShellAlias::new(ps.clone())), // shell alias first
        ps,                                    // powershell second
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_shell_tools_list() {
        let tools = get_shell_tools();

        // Verify we have exactly 2 tools
        assert_eq!(tools.len(), 2, "Expected 2 shell tools (alias + platform-specific)");

        // Verify first tool is the shell alias
        assert_eq!(tools[0].name(), "shell", "First tool should be 'shell' alias");

        // Verify platform-specific tool
        #[cfg(unix)]
        assert_eq!(tools[1].name(), "bash", "Second tool should be 'bash' on Unix");
        #[cfg(windows)]
        assert_eq!(tools[1].name(), "powershell", "Second tool should be 'powershell' on Windows");
    }

    #[test]
    fn test_tool_names_unique() {
        let tools = get_shell_tools();
        let names: HashSet<_> = tools.iter().map(|t| t.name()).collect();

        // All tool names should be unique
        assert_eq!(names.len(), tools.len(), "Tool names must be unique");
    }

    #[test]
    fn test_shell_executor_matches_platform() {
        let executor = get_shell_executor();

        #[cfg(unix)]
        {
            assert_eq!(executor.program(), "bash");
            assert_eq!(executor.command_arg(), "-c");
        }
        #[cfg(windows)]
        {
            assert_eq!(executor.program(), "powershell.exe");
            assert_eq!(executor.command_arg(), "-EncodedCommand");
        }
    }

    #[test]
    fn test_get_shell_tool_returns_correct_type() {
        let tool = get_shell_tool();

        #[cfg(unix)]
        assert_eq!(tool.name(), "bash");
        #[cfg(windows)]
        assert_eq!(tool.name(), "powershell");
    }
}
