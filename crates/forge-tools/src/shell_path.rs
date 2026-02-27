//! Shell command path extraction
//!
//! This module provides utilities to extract file paths from shell commands
//! for security checking.

use std::sync::LazyLock;
use regex::Regex;
use std::path::PathBuf;

/// Regex for extracting redirect targets
#[allow(clippy::expect_used)]
static REDIRECT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r">{1,2}\s*([^\s|&;]+)").expect("Invalid redirect regex"));

/// Extract file paths from a shell command
///
/// Parses the command and extracts paths that may be affected by file operations.
#[must_use]
pub fn extract_paths_from_command(command: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Parse command using shell-words for proper quote handling
    let Ok(tokens) = shell_words::split(command) else { return paths };

    if tokens.is_empty() {
        return paths;
    }

    let cmd = tokens[0].as_str();
    match cmd {
        "rm" | "mv" | "cp" | "cat" | "head" | "tail" | "mkdir" | "touch" => {
            // Skip command and options, collect path arguments
            for token in &tokens[1..] {
                if !token.starts_with('-') {
                    let expanded = shellexpand::tilde(token);
                    paths.push(PathBuf::from(expanded.as_ref()));
                }
            }
        }
        "chmod" | "chown" => {
            // Skip command, options, and first non-option (mode/owner)
            let mut found_mode_or_owner = false;
            for token in &tokens[1..] {
                if token.starts_with('-') {
                    continue;
                }
                if !found_mode_or_owner {
                    found_mode_or_owner = true;
                    continue;
                }
                let expanded = shellexpand::tilde(token);
                paths.push(PathBuf::from(expanded.as_ref()));
            }
        }
        _ => {}
    }

    paths
}

/// Extract redirect target paths from a shell command
///
/// Finds paths that are targets of output redirection (> or >>).
#[must_use]
pub fn extract_redirect_targets(command: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    for cap in REDIRECT_REGEX.captures_iter(command) {
        if let Some(path) = cap.get(1) {
            let expanded = shellexpand::tilde(path.as_str());
            paths.push(PathBuf::from(expanded.as_ref()));
        }
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rm_paths() {
        let paths = extract_paths_from_command("rm -rf /tmp/test");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/tmp/test"));
    }

    #[test]
    fn test_extract_quoted_paths() {
        let paths = extract_paths_from_command(r#"rm "/path with spaces/file""#);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/path with spaces/file"));
    }

    #[test]
    fn test_extract_chmod_paths() {
        let paths = extract_paths_from_command("chmod 755 /tmp/script.sh");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/tmp/script.sh"));
    }

    #[test]
    fn test_extract_redirect_targets() {
        let paths = extract_redirect_targets("echo hello > /tmp/out.txt");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/tmp/out.txt"));
    }
}
