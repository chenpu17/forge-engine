//! Shared path utilities for normalization and boundary checking.
//!
//! Provides a single implementation of lexical path normalization used across
//! permission policy, trust permission, and tool extension modules.

use std::path::{Component, Path, PathBuf};

/// Lexically normalize a path by resolving `.` and `..` components.
///
/// Unlike [`std::fs::canonicalize`], this does **not** require the path to exist
/// and does **not** follow symlinks. It is used as a fallback for non-existent
/// paths to prevent directory traversal attacks (e.g. `/project/../etc/passwd`).
///
/// `ParentDir` (`..`) pops the last `Normal` component but will not pop past
/// the root, matching the behaviour of most filesystems.
#[must_use]
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut components: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                // Pop the last normal component; don't pop past root
                if matches!(components.last(), Some(Component::Normal(_))) {
                    components.pop();
                }
            }
            Component::CurDir => {
                // Skip `.`
            }
            other => {
                components.push(other);
            }
        }
    }
    components.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_removes_dot() {
        let p = Path::new("/a/./b/./c");
        assert_eq!(normalize_path(p), PathBuf::from("/a/b/c"));
    }

    #[test]
    fn test_normalize_resolves_parent() {
        let p = Path::new("/a/b/../c");
        assert_eq!(normalize_path(p), PathBuf::from("/a/c"));
    }

    #[test]
    fn test_normalize_does_not_pop_past_root() {
        let p = Path::new("/a/../../b");
        assert_eq!(normalize_path(p), PathBuf::from("/b"));
    }

    #[test]
    fn test_normalize_relative() {
        let p = Path::new("a/b/../c");
        assert_eq!(normalize_path(p), PathBuf::from("a/c"));
    }

    #[test]
    fn test_normalize_empty() {
        let p = Path::new("");
        assert_eq!(normalize_path(p), PathBuf::from(""));
    }
}
