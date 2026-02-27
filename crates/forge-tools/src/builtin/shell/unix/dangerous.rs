//! Unix dangerous command patterns

/// Unix dangerous command detection
pub struct UnixDangerousCommands;

impl UnixDangerousCommands {
    /// Patterns that indicate dangerous commands (require confirmation)
    #[must_use]
    pub const fn dangerous_patterns() -> &'static [&'static str] {
        &[
            "sudo ", "rm -r", "rm -f", "chmod ", "chown ", "kill ", "pkill ", "killall ", "mv /",
            "cp /", "> /", ">> /",
        ]
    }

    /// Patterns that indicate very dangerous commands (require extra warning)
    #[must_use]
    pub const fn very_dangerous_patterns() -> &'static [&'static str] {
        &[
            "rm -rf /",
            "rm -rf ~",
            "rm -rf /*",
            "sudo rm -rf /",
            "mkfs",
            "dd if=",
            "> /dev/",
            "chmod 777 /",
            ":(){:|:&};:", // Fork bomb
            "curl | sh",
            "curl | bash",
            "wget | sh",
            "wget | bash",
        ]
    }

    /// Patterns that indicate write operations (for read-only mode)
    #[must_use]
    pub const fn write_patterns() -> &'static [&'static str] {
        &[
            // File modification commands
            "rm ",
            "rm\t",
            "rmdir ",
            "mv ",
            "mv\t",
            "cp ",
            "cp\t",
            "touch ",
            "touch\t",
            "mkdir ",
            "mkdir\t",
            // Redirection (write to file)
            " > ",
            " >> ",
            ">|",
            // Text manipulation that writes
            "tee ",
            "tee\t",
            // Editors (could modify files)
            "nano ",
            "vim ",
            "vi ",
            "emacs ",
            // Package managers (install/remove)
            "apt ",
            "apt-get ",
            "yum ",
            "dnf ",
            "pacman ",
            "npm install",
            "npm uninstall",
            "npm remove",
            "pip install",
            "pip uninstall",
            "cargo install",
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
            // Dangerous operations
            "chmod ",
            "chown ",
            "chgrp ",
            "sudo ",
            "dd ",
            "mkfs",
        ]
    }

    /// Check if a command is potentially dangerous
    #[must_use]
    pub fn is_dangerous(command: &str) -> bool {
        Self::dangerous_patterns().iter().any(|p| command.contains(p))
    }

    /// Check if a command is very dangerous (needs extra warning)
    #[must_use]
    pub fn is_very_dangerous(command: &str) -> bool {
        // Special handling for "rm -rf /" - check it's root, not /tmp etc
        if command.contains("rm -rf /") || command.contains("rm -rf ~") {
            let cmd = command.trim();
            if cmd.ends_with("rm -rf /") || cmd.ends_with("rm -rf ~") {
                return true;
            }
            // Check for "rm -rf / " followed by flags, not paths
            if cmd.contains("rm -rf / ") || cmd.contains("rm -rf ~ ") {
                return true;
            }
            // Check for "rm -rf /*" (delete all in root)
            if cmd.contains("rm -rf /*") {
                return true;
            }
        }

        // Check other patterns
        Self::very_dangerous_patterns().iter().any(|p| {
            // Skip the rm patterns we handled above
            if p.starts_with("rm -rf") {
                return false;
            }
            command.contains(p)
        })
    }

    /// Check if a command performs write operations (for `bash_readonly` mode)
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
        assert!(UnixDangerousCommands::is_dangerous("sudo apt update"));
        assert!(UnixDangerousCommands::is_dangerous("rm -rf /tmp"));
        assert!(!UnixDangerousCommands::is_dangerous("ls -la"));
        assert!(!UnixDangerousCommands::is_dangerous("git status"));
    }

    #[test]
    fn test_very_dangerous_detection() {
        assert!(UnixDangerousCommands::is_very_dangerous("rm -rf /"));
        assert!(UnixDangerousCommands::is_very_dangerous("sudo rm -rf ~"));
        assert!(UnixDangerousCommands::is_very_dangerous("mkfs.ext4 /dev/sda"));
        assert!(!UnixDangerousCommands::is_very_dangerous("rm -rf /tmp/test"));
    }

    #[test]
    fn test_write_command_detection() {
        assert!(UnixDangerousCommands::is_write_command("rm file.txt"));
        assert!(UnixDangerousCommands::is_write_command("git push"));
        assert!(UnixDangerousCommands::is_write_command("npm install"));
        assert!(!UnixDangerousCommands::is_write_command("ls -la"));
        assert!(!UnixDangerousCommands::is_write_command("cat file.txt"));
    }
}
