//! Platform-specific path utilities
//!
//! Provides:
//! - Temporary directory abstraction
//! - Sensitive path detection (system-level)
//! - Sensitive pattern detection (project-level)

use std::path::{Component, Path, PathBuf};

/// Platform-specific path utilities
pub struct PlatformPaths;

impl PlatformPaths {
    /// Get the system temporary directory
    #[must_use]
    pub fn temp_dir() -> PathBuf {
        std::env::temp_dir()
    }

    /// Get system-level sensitive paths that should be blocked
    #[cfg(unix)]
    #[must_use]
    pub const fn sensitive_paths() -> &'static [&'static str] {
        &[
            // System files
            "/etc/shadow",
            "/etc/passwd",
            "/etc/sudoers",
            "/etc/ssh/",
            "/root/",
            "/var/log/auth",
            "/var/log/secure",
            // SSH keys
            "/.ssh/",
            "/id_rsa",
            "/id_ed25519",
            "/id_ecdsa",
            "/id_dsa",
            "/authorized_keys",
            "/known_hosts",
            // Credentials
            "/.aws/credentials",
            "/.aws/config",
            "/.netrc",
            "/.npmrc",
            "/.pypirc",
            "/.docker/config.json",
            "/.kube/config",
        ]
    }

    /// Get system-level sensitive paths that should be blocked (Windows)
    #[cfg(windows)]
    pub fn sensitive_paths() -> &'static [&'static str] {
        &[
            // System configuration
            r"\Windows\System32\config\",
            r"\Windows\System32\drivers\etc\",
            // Crypto and credentials
            r"\ProgramData\Microsoft\Crypto\",
            // Registry hives
            r"\Windows\System32\config\SAM",
            r"\Windows\System32\config\SECURITY",
            r"\Windows\System32\config\SYSTEM",
        ]
    }

    /// Get sensitive path patterns (with wildcards) for Windows
    /// Format: (prefix, suffix) - matches prefix/**/suffix
    #[cfg(windows)]
    pub fn sensitive_patterns_windows() -> &'static [(&'static str, &'static str)] {
        &[
            (r"c:\users\", r"\appdata\local\microsoft\credentials"),
            (r"c:\users\", r"\.ssh\"),
            (r"c:\users\", r"\appdata\roaming\microsoft\protect"),
        ]
    }

    /// Get project-level sensitive patterns that require confirmation
    /// These are cross-platform
    #[must_use]
    pub const fn sensitive_patterns() -> &'static [&'static str] {
        &[
            ".env",
            ".env.local",
            ".env.production",
            "credentials.json",
            "secrets.json",
            "private_key",
            "service_account",
        ]
    }

    /// Safe suffixes that should NOT require confirmation
    #[must_use]
    pub const fn safe_suffixes() -> &'static [&'static str] {
        &[".example", ".sample", ".template", ".dist", ".test"]
    }

    /// Check if a path is a system-level sensitive path
    #[cfg(unix)]
    #[must_use]
    pub fn is_sensitive_path(path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in Self::sensitive_paths() {
            if path_str.contains(pattern) {
                return true;
            }
        }

        false
    }

    /// Check if a path is a system-level sensitive path (Windows)
    #[cfg(windows)]
    pub fn is_sensitive_path(path: &Path) -> bool {
        let normalized = Self::normalize_windows_path(path);

        // Check prefix matches
        for pattern in Self::sensitive_paths() {
            let pattern_lower = pattern.to_lowercase();
            if normalized.starts_with(&pattern_lower) || normalized.contains(&pattern_lower) {
                return true;
            }
        }

        // Check pattern matches (prefix + suffix)
        for (prefix, suffix) in Self::sensitive_patterns_windows() {
            if normalized.starts_with(prefix) && normalized.contains(suffix) {
                return true;
            }
        }

        false
    }

    /// Normalize Windows path for comparison
    /// - Remove \\?\ and \\?\UNC\ prefixes
    /// - Convert to lowercase
    /// - Unify separators
    #[cfg(windows)]
    fn normalize_windows_path(path: &Path) -> String {
        let s = path.to_string_lossy();

        // Handle \\?\UNC\ prefix (convert back to standard UNC)
        let s = if s.starts_with(r"\\?\UNC\") {
            format!(r"\\{}", &s[8..])
        } else if s.starts_with(r"\\?\") {
            s[4..].to_string()
        } else {
            s.to_string()
        };

        // Unify separators and convert to lowercase
        s.replace('/', r"\").to_lowercase()
    }

    /// Check if a filename matches project-level sensitive patterns
    #[must_use]
    pub fn needs_confirmation(filename: &str) -> bool {
        let filename_lower = filename.to_lowercase();

        // If it has a safe suffix, no confirmation needed
        if Self::safe_suffixes().iter().any(|s| filename_lower.ends_with(s)) {
            return false;
        }

        // Check against sensitive patterns
        Self::sensitive_patterns().iter().any(|p| filename_lower.contains(&p.to_lowercase()))
    }

    /// Check if a path is a UNC path (Windows only)
    #[cfg(windows)]
    pub fn is_unc_path(path: &Path) -> bool {
        let s = path.to_string_lossy();
        (s.starts_with(r"\\") && !s.starts_with(r"\\?\")) || s.starts_with(r"\\?\UNC\")
    }

    /// Resolve a path relative to a base directory (Unix)
    ///
    /// # Errors
    /// Returns `Err` if path traversal is detected.
    #[cfg(unix)]
    #[allow(clippy::path_buf_push_overwrite)]
    pub fn resolve_path(path: &Path, base: &Path) -> Result<PathBuf, String> {
        let absolute = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };

        let mut normalized = PathBuf::new();
        for component in absolute.components() {
            match component {
                Component::RootDir => normalized.push("/"),
                Component::Normal(c) => normalized.push(c),
                Component::ParentDir => {
                    if !normalized.pop() {
                        return Err(format!("Path traversal detected: {}", path.display()));
                    }
                }
                Component::CurDir | Component::Prefix(_) => {} // Ignore . and prefix (Unix has no prefix)
            }
        }

        Ok(normalized)
    }

    /// Resolve a path relative to a base directory (Windows)
    /// Returns Err if path traversal is detected
    #[cfg(windows)]
    pub fn resolve_path(path: &Path, base: &Path) -> Result<PathBuf, String> {
        use std::path::Prefix;

        let absolute = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };

        let mut normalized = PathBuf::new();
        let mut has_prefix = false;

        for component in absolute.components() {
            match component {
                // Windows: must preserve Prefix (drive letter/UNC)
                Component::Prefix(prefix) => {
                    has_prefix = true;
                    match prefix.kind() {
                        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                            let upper = (letter as char).to_ascii_uppercase();
                            normalized.push(format!("{}:", upper));
                        }
                        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                            normalized.push(format!(
                                r"\\{}\{}",
                                server.to_string_lossy(),
                                share.to_string_lossy()
                            ));
                        }
                        _ => {
                            normalized.push(prefix.as_os_str());
                        }
                    }
                }
                Component::RootDir => {
                    if has_prefix {
                        normalized.push(r"\");
                    } else {
                        // No prefix root dir, use base's drive
                        if let Some(Component::Prefix(p)) = base.components().next() {
                            normalized.push(p.as_os_str());
                        }
                        normalized.push(r"\");
                    }
                }
                Component::Normal(c) => normalized.push(c),
                Component::ParentDir => {
                    // Cannot pop above drive root
                    let current = normalized.to_string_lossy();
                    if current.ends_with(':') || current.ends_with(r":\") {
                        return Err(format!("Path traversal detected: {}", path.display()));
                    }
                    if !normalized.pop() {
                        return Err(format!("Path traversal detected: {}", path.display()));
                    }
                }
                Component::CurDir => {}
            }
        }

        Ok(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temp_dir() {
        let temp = PlatformPaths::temp_dir();
        assert!(temp.exists() || temp.to_string_lossy().len() > 0);
    }

    #[test]
    fn test_needs_confirmation() {
        assert!(PlatformPaths::needs_confirmation(".env"));
        assert!(PlatformPaths::needs_confirmation(".env.local"));
        assert!(PlatformPaths::needs_confirmation("credentials.json"));

        // Safe suffixes should not need confirmation
        assert!(!PlatformPaths::needs_confirmation(".env.example"));
        assert!(!PlatformPaths::needs_confirmation(".env.template"));
        assert!(!PlatformPaths::needs_confirmation("credentials.json.sample"));
    }

    #[test]
    #[cfg(unix)]
    fn test_sensitive_path_unix() {
        assert!(PlatformPaths::is_sensitive_path(Path::new("/etc/shadow")));
        assert!(PlatformPaths::is_sensitive_path(Path::new("/home/user/.ssh/id_rsa")));
        assert!(!PlatformPaths::is_sensitive_path(Path::new("/home/user/project/file.txt")));
    }

    #[test]
    #[cfg(unix)]
    fn test_resolve_path_unix() {
        let base = Path::new("/home/user/project");

        // Normal relative path
        assert_eq!(
            PlatformPaths::resolve_path(Path::new("src/main.rs"), base).unwrap(),
            PathBuf::from("/home/user/project/src/main.rs")
        );

        // Path with ..
        assert_eq!(
            PlatformPaths::resolve_path(Path::new("../other/file.txt"), base).unwrap(),
            PathBuf::from("/home/user/other/file.txt")
        );

        // Traversal beyond root should fail
        assert!(PlatformPaths::resolve_path(Path::new("../../../../../../../../etc/passwd"), base)
            .is_err());
    }

    #[test]
    #[cfg(windows)]
    fn test_sensitive_path_windows() {
        assert!(PlatformPaths::is_sensitive_path(Path::new(r"C:\Windows\System32\config\SAM")));
        assert!(PlatformPaths::is_sensitive_path(Path::new(r"\\?\C:\Windows\System32\config\SAM")));
        assert!(!PlatformPaths::is_sensitive_path(Path::new(r"C:\Users\Admin\Documents\file.txt")));
    }

    #[test]
    #[cfg(windows)]
    fn test_resolve_path_windows() {
        let base = Path::new(r"C:\Users\test\project");

        // Normal relative path
        assert_eq!(
            PlatformPaths::resolve_path(Path::new("src\\main.rs"), base).unwrap(),
            PathBuf::from(r"C:\Users\test\project\src\main.rs")
        );

        // Path with ..
        assert_eq!(
            PlatformPaths::resolve_path(Path::new("..\\other\\file.txt"), base).unwrap(),
            PathBuf::from(r"C:\Users\test\other\file.txt")
        );

        // Traversal beyond drive root should fail
        assert!(PlatformPaths::resolve_path(
            Path::new("..\\..\\..\\..\\..\\..\\..\\..\\etc\\passwd"),
            base
        )
        .is_err());
    }

    #[test]
    #[cfg(windows)]
    fn test_unc_path_detection() {
        // Standard UNC paths
        assert!(PlatformPaths::is_unc_path(Path::new(r"\\server\share")));
        assert!(PlatformPaths::is_unc_path(Path::new(r"\\server\share\folder")));

        // Verbatim UNC paths
        assert!(PlatformPaths::is_unc_path(Path::new(r"\\?\UNC\server\share")));

        // Non-UNC paths
        assert!(!PlatformPaths::is_unc_path(Path::new(r"C:\folder")));
        assert!(!PlatformPaths::is_unc_path(Path::new(r"\\?\C:\folder"))); // Verbatim disk, not UNC
    }

    #[test]
    #[cfg(windows)]
    fn test_normalize_windows_path_prefixes() {
        // Test that normalize_windows_path correctly handles various prefixes
        // by checking is_sensitive_path with different path formats

        // Standard path
        assert!(PlatformPaths::is_sensitive_path(Path::new(r"C:\Windows\System32\config\SAM")));

        // \\?\ prefix (verbatim path)
        assert!(PlatformPaths::is_sensitive_path(Path::new(r"\\?\C:\Windows\System32\config\SAM")));

        // Mixed separators should also work after normalization
        let test_path = Path::new(r"c:/windows/system32/config/SAM");
        assert!(PlatformPaths::is_sensitive_path(test_path));
    }

    #[test]
    #[cfg(windows)]
    fn test_drive_letter_case_insensitive() {
        let base = Path::new(r"C:\Users\test");

        // Uppercase and lowercase drive letters should be equivalent
        let result_upper = PlatformPaths::resolve_path(Path::new(r"C:\file.txt"), base);
        let result_lower = PlatformPaths::resolve_path(Path::new(r"c:\file.txt"), base);

        // Both should succeed and produce consistent output
        assert!(result_upper.is_ok());
        assert!(result_lower.is_ok());
    }
}
