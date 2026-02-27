//! Windows dangerous command patterns

/// Windows dangerous command detection
pub struct WindowsDangerousCommands;

impl WindowsDangerousCommands {
    /// Patterns that indicate dangerous commands (require confirmation)
    #[must_use]
    pub const fn dangerous_patterns() -> &'static [&'static str] {
        &[
            "Remove-Item",
            "del ",
            "rd ",
            "rmdir ",
            "Move-Item",
            "Copy-Item",
            "Stop-Process",
            "taskkill",
            "Set-ExecutionPolicy",
            "reg delete",
            "reg add",
        ]
    }

    /// Patterns that indicate very dangerous commands (require extra warning)
    #[must_use]
    pub const fn very_dangerous_patterns() -> &'static [&'static str] {
        &[
            "Remove-Item -Recurse -Force C:\\",
            "Remove-Item -Recurse -Force $env:",
            "Format-Volume",
            "Clear-Disk",
            "Initialize-Disk",
            "Invoke-Expression",
            "iex ",
            "Start-Process -Verb RunAs", // Privilege escalation
            "del /s /q C:\\",
            "rd /s /q C:\\",
            "format ",
        ]
    }

    /// Patterns that indicate write operations (for read-only mode)
    #[must_use]
    pub const fn write_patterns() -> &'static [&'static str] {
        &[
            // File modification
            "Remove-Item",
            "del ",
            "rd ",
            "rmdir ",
            "Move-Item",
            "Copy-Item",
            "New-Item",
            "mkdir ",
            "md ",
            // Content modification
            "Set-Content",
            "Add-Content",
            "Out-File",
            "> ",
            ">> ",
            // Registry
            "reg add",
            "reg delete",
            "Set-ItemProperty",
            "New-ItemProperty",
            "Remove-ItemProperty",
            // Git write operations
            "git push",
            "git commit",
            "git add",
            "git rm",
            "git mv",
            "git reset",
            "git revert",
            "git merge",
            "git rebase",
            // Package managers
            "choco install",
            "choco uninstall",
            "winget install",
            "winget uninstall",
            "npm install",
            "npm uninstall",
            "pip install",
            "pip uninstall",
            // System modification
            "Set-ExecutionPolicy",
            "Enable-WindowsOptionalFeature",
            "Disable-WindowsOptionalFeature",
        ]
    }

    /// Check if a command is potentially dangerous
    #[must_use]
    pub fn is_dangerous(command: &str) -> bool {
        let cmd_lower = command.to_lowercase();
        Self::dangerous_patterns().iter().any(|p| cmd_lower.contains(&p.to_lowercase()))
    }

    /// Check if a command is very dangerous (needs extra warning)
    #[must_use]
    pub fn is_very_dangerous(command: &str) -> bool {
        let cmd_lower = command.to_lowercase();
        Self::very_dangerous_patterns().iter().any(|p| cmd_lower.contains(&p.to_lowercase()))
    }

    /// Check if a command performs write operations (for read-only mode)
    #[must_use]
    pub fn is_write_command(command: &str) -> bool {
        let cmd_lower = command.to_lowercase();
        Self::write_patterns().iter().any(|p| cmd_lower.contains(&p.to_lowercase()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dangerous_detection() {
        assert!(WindowsDangerousCommands::is_dangerous("Remove-Item -Path file.txt"));
        assert!(WindowsDangerousCommands::is_dangerous("del file.txt"));
        assert!(WindowsDangerousCommands::is_dangerous("taskkill /F /PID 1234"));
        assert!(!WindowsDangerousCommands::is_dangerous("Get-ChildItem"));
        assert!(!WindowsDangerousCommands::is_dangerous("dir"));
    }

    #[test]
    fn test_very_dangerous_detection() {
        assert!(WindowsDangerousCommands::is_very_dangerous("Remove-Item -Recurse -Force C:\\"));
        assert!(WindowsDangerousCommands::is_very_dangerous("Invoke-Expression $malicious"));
        assert!(WindowsDangerousCommands::is_very_dangerous("Format-Volume"));
        assert!(!WindowsDangerousCommands::is_very_dangerous("Remove-Item file.txt"));
    }

    #[test]
    fn test_write_command_detection() {
        assert!(WindowsDangerousCommands::is_write_command("Remove-Item file.txt"));
        assert!(WindowsDangerousCommands::is_write_command("git push"));
        assert!(WindowsDangerousCommands::is_write_command("npm install"));
        assert!(!WindowsDangerousCommands::is_write_command("Get-ChildItem"));
        assert!(!WindowsDangerousCommands::is_write_command("Get-Content file.txt"));
    }
}
