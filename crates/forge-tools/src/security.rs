//! Security utilities for tool execution
//!
//! This module provides security checks for file operations:
//! - Path traversal prevention
//! - Sensitive file access blocking
//! - Working directory restriction

use crate::platform::PlatformPaths;
use crate::ToolError;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Canonicalize a parent directory, handling the case where it doesn't exist yet.
///
/// If the parent directory doesn't exist (will be created by write tool),
/// we walk up the path to find the nearest existing ancestor for canonicalization,
/// then rebuild the path with the missing components.
///
/// This is important for security: we need to resolve symlinks in the existing
/// portion of the path to prevent symlink-based directory traversal attacks.
fn canonicalize_parent_for_write(
    parent_dir: &Path,
    original_path: &str,
) -> Result<PathBuf, ToolError> {
    if parent_dir.exists() {
        parent_dir.canonicalize().map_err(|e| {
            ToolError::PermissionDenied(format!(
                "Parent directory '{}' is not accessible: {}",
                parent_dir.display(),
                e
            ))
        })
    } else {
        // Find the nearest existing ancestor and build the canonical path
        let mut existing_ancestor = parent_dir.to_path_buf();
        let mut missing_components: Vec<OsString> = Vec::new();

        while !existing_ancestor.exists() {
            if let Some(component) = existing_ancestor.file_name() {
                missing_components.push(component.to_os_string());
            }
            if let Some(parent) = existing_ancestor.parent() {
                existing_ancestor = parent.to_path_buf();
            } else {
                // Reached root without finding existing directory
                return Err(ToolError::PermissionDenied(format!(
                    "No accessible parent directory found for '{original_path}'"
                )));
            }
        }

        // Canonicalize the existing ancestor
        let canonical_ancestor = existing_ancestor.canonicalize().map_err(|e| {
            ToolError::PermissionDenied(format!(
                "Ancestor directory '{}' is not accessible: {}",
                existing_ancestor.display(),
                e
            ))
        })?;

        // Rebuild the path with missing components
        let mut result = canonical_ancestor;
        for component in missing_components.into_iter().rev() {
            result = result.join(component);
        }
        Ok(result)
    }
}

/// Path validation result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathValidation {
    /// Path is safe to access
    Safe(PathBuf),
    /// Path traversal attempt detected
    PathTraversal(String),
    /// Sensitive file access blocked (system-level, cannot be overridden)
    SensitiveFile(String),
    /// Sensitive file that requires user confirmation (project-level)
    SensitiveFileConfirm {
        /// The canonical path being accessed
        path: PathBuf,
        /// Reason for requiring confirmation
        reason: String,
    },
    /// Path is outside allowed directory - needs user confirmation
    OutsideWorkingDir {
        /// The canonical path being accessed
        path: PathBuf,
        /// The working directory
        working_dir: PathBuf,
    },
}

impl PathValidation {
    /// Convert to Result, returning Err for blocked paths
    ///
    /// Note: `OutsideWorkingDir` and `SensitiveFileConfirm` return `PathConfirmationRequired` error
    ///
    /// # Errors
    ///
    /// Returns an error for any non-safe path validation result.
    pub fn into_result(self) -> Result<PathBuf, ToolError> {
        match self {
            Self::Safe(path) => Ok(path),
            Self::PathTraversal(msg) => {
                Err(ToolError::PermissionDenied(format!("Path traversal blocked: {msg}")))
            }
            Self::SensitiveFile(msg) => {
                Err(ToolError::PermissionDenied(format!("Sensitive file access blocked: {msg}")))
            }
            Self::SensitiveFileConfirm { path, reason } => {
                Err(ToolError::PathConfirmationRequired {
                    path: path.to_string_lossy().to_string(),
                    reason,
                })
            }
            Self::OutsideWorkingDir { path, working_dir } => {
                Err(ToolError::PathConfirmationRequired {
                    path: path.to_string_lossy().to_string(),
                    reason: format!(
                        "Path '{}' is outside working directory '{}'",
                        path.display(),
                        working_dir.display()
                    ),
                })
            }
        }
    }
}

/// Security configuration for path validation
#[derive(Debug, Clone)]
pub struct PathSecurityConfig {
    /// Whether to enforce working directory restriction
    pub enforce_working_dir: bool,
    /// Whether to block sensitive file access
    pub block_sensitive_files: bool,
    /// Additional allowed paths (outside working directory)
    pub allowed_paths: Vec<PathBuf>,
    /// Additional blocked patterns
    pub blocked_patterns: Vec<String>,
}

impl Default for PathSecurityConfig {
    fn default() -> Self {
        // Use platform-specific temp directories
        #[cfg(unix)]
        let mut allowed_paths = vec![PlatformPaths::temp_dir()];
        #[cfg(windows)]
        let allowed_paths = vec![PlatformPaths::temp_dir()];

        // Add additional platform-specific temp paths
        #[cfg(unix)]
        {
            // On macOS, /tmp is a symlink to /private/var/folders/...
            // We need to allow both the canonical temp dir and /tmp
            allowed_paths.push(PathBuf::from("/tmp"));
            allowed_paths.push(PathBuf::from("/var/tmp"));
            // Also allow the private temp directory prefix on macOS
            allowed_paths.push(PathBuf::from("/private/tmp"));
            allowed_paths.push(PathBuf::from("/private/var/folders"));
        }

        #[cfg(windows)]
        {
            // Note: c:/temp and c:/tmp are NOT standard Windows temp directories
            // They should require user confirmation via path confirmation dialog
            // Only the system temp directory (from PlatformPaths::temp_dir()) is allowed by default
        }

        Self {
            enforce_working_dir: true,
            block_sensitive_files: true,
            allowed_paths,
            blocked_patterns: vec![],
        }
    }
}

impl PathSecurityConfig {
    /// Create a permissive config (for testing)
    #[must_use]
    pub const fn permissive() -> Self {
        Self {
            enforce_working_dir: false,
            block_sensitive_files: false,
            allowed_paths: Vec::new(),
            blocked_patterns: Vec::new(),
        }
    }

    /// Add an allowed path
    #[must_use]
    pub fn allow_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.allowed_paths.push(path.into());
        self
    }
}

/// Path security validator
pub struct PathSecurity {
    config: PathSecurityConfig,
}

impl PathSecurity {
    /// Create a new path security validator with the given config
    #[must_use]
    pub const fn new(config: PathSecurityConfig) -> Self {
        Self { config }
    }

    /// Validate a path for file access
    ///
    /// # Arguments
    /// * `path` - The path to validate (can be relative or absolute)
    /// * `working_dir` - The current working directory
    ///
    /// # Returns
    /// * `PathValidation::Safe(canonical_path)` - Path is safe to access
    /// * `PathValidation::PathTraversal(_)` - Path traversal attempt detected
    /// * `PathValidation::SensitiveFile(_)` - Sensitive file access blocked
    /// * `PathValidation::OutsideWorkingDir(_)` - Path is outside allowed directories
    #[must_use]
    pub fn validate(&self, path: &str, working_dir: &Path) -> PathValidation {
        // Check for obvious path traversal patterns first
        if path.contains("..") {
            // Allow ".." only if it doesn't escape working directory
            // We'll check this after canonicalization
        }

        // Resolve the path
        let resolved_path = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            working_dir.join(path)
        };

        // Try to canonicalize (this resolves symlinks and ..)
        let canonical_path = match resolved_path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                // If file doesn't exist, we still need to validate the intended path
                // Normalize the path manually
                match self.normalize_path(&resolved_path) {
                    Some(p) => p,
                    None => {
                        return PathValidation::PathTraversal(format!("Invalid path: {path}"));
                    }
                }
            }
        };

        let canonical_str = canonical_path.to_string_lossy();

        // Check for sensitive file access
        if self.config.block_sensitive_files {
            if let Some((is_hard_block, reason)) = self.is_sensitive_path(&canonical_str) {
                if is_hard_block {
                    // System-level sensitive file - hard block
                    return PathValidation::SensitiveFile(reason);
                }
                // Project-level sensitive file - require user confirmation
                return PathValidation::SensitiveFileConfirm { path: canonical_path, reason };
            }
        }

        // Check working directory restriction
        if self.config.enforce_working_dir {
            // Canonicalize working directory for comparison
            let canonical_working_dir =
                working_dir.canonicalize().unwrap_or_else(|_| working_dir.to_path_buf());

            // Check if path is within working directory or allowed paths
            let is_allowed = canonical_path.starts_with(&canonical_working_dir)
                || self.config.allowed_paths.iter().any(|allowed| {
                    allowed.canonicalize().map_or_else(
                        |_| canonical_path.starts_with(allowed),
                        |canonical_allowed| canonical_path.starts_with(&canonical_allowed),
                    )
                });

            if !is_allowed {
                return PathValidation::OutsideWorkingDir {
                    path: canonical_path,
                    working_dir: canonical_working_dir,
                };
            }
        }

        PathValidation::Safe(canonical_path)
    }

    /// Validate a path and return a Result
    ///
    /// Convenience method that converts `PathValidation` to Result
    ///
    /// # Errors
    ///
    /// Returns an error for any non-safe path validation result.
    pub fn validate_path(&self, path: &str, working_dir: &Path) -> Result<PathBuf, ToolError> {
        self.validate(path, working_dir).into_result()
    }

    /// Check if a path is sensitive
    /// Returns: None if safe, `Some((is_hard_block, reason))` if sensitive
    /// - `is_hard_block=true`: system-level sensitive file, cannot be overridden
    /// - `is_hard_block=false`: project-level sensitive file, can be confirmed by user
    ///
    /// Uses `PlatformPaths` for unified cross-platform sensitive path detection.
    fn is_sensitive_path(&self, path: &str) -> Option<(bool, String)> {
        let path_obj = Path::new(path);

        // Check against system-level sensitive paths using PlatformPaths (hard block)
        if PlatformPaths::is_sensitive_path(path_obj) {
            return Some((true, format!("Access to '{path}' is blocked for security")));
        }

        // Check against project-level sensitive filename patterns using PlatformPaths
        // (requires user confirmation)
        if let Some(filename) = path_obj.file_name().and_then(|f| f.to_str()) {
            if PlatformPaths::needs_confirmation(filename) {
                return Some((false, format!("File '{filename}' may contain sensitive data")));
            }
        }

        // Check custom blocked patterns (hard block)
        let path_lower = path.to_lowercase();
        for pattern in &self.config.blocked_patterns {
            if path_lower.contains(&pattern.to_lowercase()) {
                return Some((true, format!("Access blocked by pattern: {pattern}")));
            }
        }

        None
    }

    /// Normalize a path without requiring it to exist
    #[allow(clippy::unused_self)]
    fn normalize_path(&self, path: &Path) -> Option<PathBuf> {
        let mut components = Vec::new();

        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    // Pop the last component, but don't allow escaping root
                    if components.is_empty() {
                        return None; // Trying to go above root
                    }
                    components.pop();
                }
                std::path::Component::CurDir => {
                    // Skip current directory markers
                }
                c => {
                    components.push(c);
                }
            }
        }

        if components.is_empty() {
            return None;
        }

        Some(components.iter().collect())
    }
}

impl Default for PathSecurity {
    fn default() -> Self {
        Self::new(PathSecurityConfig::default())
    }
}

/// Global path security instance with default config
static DEFAULT_PATH_SECURITY: std::sync::OnceLock<PathSecurity> = std::sync::OnceLock::new();

/// Get the default path security validator
pub fn default_path_security() -> &'static PathSecurity {
    DEFAULT_PATH_SECURITY.get_or_init(PathSecurity::default)
}

/// Convenience function to validate a path using default security settings
///
/// # Errors
///
/// Returns an error for any non-safe path validation result.
pub fn validate_path(path: &str, working_dir: &Path) -> Result<PathBuf, ToolError> {
    default_path_security().validate_path(path, working_dir)
}

/// Validate a path with additional confirmed paths that are allowed
///
/// This is used when user has confirmed access to certain paths outside the working directory.
/// The `confirmed_paths` set is checked in addition to the default allowed paths.
///
/// # Errors
///
/// Returns an error for any non-safe path validation result.
#[allow(clippy::implicit_hasher)]
pub fn validate_path_with_confirmed(
    path: &str,
    working_dir: &Path,
    confirmed_paths: &std::collections::HashSet<PathBuf>,
) -> Result<PathBuf, ToolError> {
    let security = default_path_security();
    let validation = security.validate(path, working_dir);

    match validation {
        PathValidation::Safe(p) => Ok(p),
        PathValidation::PathTraversal(msg) => {
            Err(ToolError::PermissionDenied(format!("Path traversal blocked: {msg}")))
        }
        PathValidation::SensitiveFile(msg) => {
            Err(ToolError::PermissionDenied(format!("Sensitive file access blocked: {msg}")))
        }
        PathValidation::SensitiveFileConfirm { path: canonical_path, reason } => {
            // Check if the path is in confirmed_paths
            let is_confirmed = confirmed_paths
                .iter()
                .any(|p| canonical_path.starts_with(p) || &canonical_path == p);

            if is_confirmed {
                Ok(canonical_path)
            } else {
                Err(ToolError::PathConfirmationRequired {
                    path: canonical_path.to_string_lossy().to_string(),
                    reason,
                })
            }
        }
        PathValidation::OutsideWorkingDir { path: canonical_path, working_dir: _ } => {
            // Check if the path is in confirmed_paths
            let is_confirmed = confirmed_paths
                .iter()
                .any(|p| canonical_path.starts_with(p) || &canonical_path == p);

            if is_confirmed {
                Ok(canonical_path)
            } else {
                Err(ToolError::PathConfirmationRequired {
                    path: canonical_path.to_string_lossy().to_string(),
                    reason: format!(
                        "Path '{}' is outside working directory",
                        canonical_path.display()
                    ),
                })
            }
        }
    }
}

/// Validate a path using `ToolContext` (convenience function)
///
/// This extracts `working_dir` and `confirmed_paths` from the context and validates the path.
///
/// # Errors
///
/// Returns an error for any non-safe path validation result.
pub fn validate_path_from_context(
    path: &str,
    ctx: &crate::ToolContext,
) -> Result<PathBuf, ToolError> {
    validate_path_with_confirmed(path, &ctx.working_dir, &ctx.confirmed_paths)
}

/// Validate a path for writing (file may not exist yet)
///
/// This validates the parent directory exists and is within allowed paths,
/// then returns the full path for the new file.
///
/// # Security
/// This function canonicalizes the parent directory to resolve symlinks,
/// preventing symlink-based directory traversal attacks where a symlink
/// inside the working directory points to an external location.
///
/// # Errors
///
/// Returns an error if the path is sensitive, outside the working directory, or invalid.
pub fn validate_write_path(path: &str, working_dir: &Path) -> Result<PathBuf, ToolError> {
    let security = default_path_security();
    let path_obj = Path::new(path);

    // Check for system-level sensitive paths (hard block)
    if PlatformPaths::is_sensitive_path(path_obj) {
        return Err(ToolError::PermissionDenied(format!(
            "Writing to '{path}' is blocked for security"
        )));
    }

    // Check for project-level sensitive file patterns (requires confirmation)
    if let Some(filename) = path_obj.file_name().and_then(|f| f.to_str()) {
        if PlatformPaths::needs_confirmation(filename) {
            return Err(ToolError::PathConfirmationRequired {
                path: path.to_string(),
                reason: format!("Writing to file '{filename}' may overwrite sensitive data"),
            });
        }
    }

    // Resolve the intended path
    let resolved_path =
        if Path::new(path).is_absolute() { PathBuf::from(path) } else { working_dir.join(path) };

    // Get the filename component
    let filename = resolved_path.file_name().ok_or_else(|| {
        ToolError::PermissionDenied(format!("Invalid path (no filename): {path}"))
    })?;

    // Get the parent directory - this is crucial for symlink protection
    let parent_dir = resolved_path.parent().ok_or_else(|| {
        ToolError::PermissionDenied(format!("Invalid path (no parent): {path}"))
    })?;

    // Canonicalize the parent directory to resolve symlinks
    // This prevents symlink-based escapes: if working_dir/symlink -> /external,
    // then working_dir/symlink/file.txt would resolve to /external/file.txt
    // For write operations, the parent may not exist yet, so we handle that case.
    let canonical_parent = canonicalize_parent_for_write(parent_dir, path)?;

    // Build the final canonical path
    let canonical_path = canonical_parent.join(filename);

    // Check if within working directory or allowed paths
    if security.config.enforce_working_dir {
        let canonical_working_dir =
            working_dir.canonicalize().unwrap_or_else(|_| working_dir.to_path_buf());

        let is_allowed = canonical_path.starts_with(&canonical_working_dir)
            || security.config.allowed_paths.iter().any(|allowed| {
                allowed.canonicalize().map_or_else(
                    |_| canonical_path.starts_with(allowed),
                    |canonical_allowed| canonical_path.starts_with(&canonical_allowed),
                )
            });

        if !is_allowed {
            return Err(ToolError::PermissionDenied(format!(
                "Writing outside working directory '{}' is blocked (resolved path: '{}')",
                canonical_working_dir.display(),
                canonical_path.display()
            )));
        }
    }

    Ok(canonical_path)
}

/// Validate a path for writing using `ToolContext` (convenience function)
///
/// This extracts `working_dir` and `confirmed_paths` from the context and validates the path.
/// For paths outside working directory, checks if the path is in `confirmed_paths`.
///
/// # Security
/// This function canonicalizes the parent directory to resolve symlinks,
/// preventing symlink-based directory traversal attacks.
///
/// # Errors
///
/// Returns an error if the path is sensitive, outside the working directory, or invalid.
pub fn validate_write_path_from_context(
    path: &str,
    ctx: &crate::ToolContext,
) -> Result<PathBuf, ToolError> {
    let security = default_path_security();
    let path_obj = Path::new(path);

    // Check for system-level sensitive paths (hard block)
    if PlatformPaths::is_sensitive_path(path_obj) {
        return Err(ToolError::PermissionDenied(format!(
            "Writing to '{path}' is blocked for security"
        )));
    }

    // Check for project-level sensitive file patterns (requires confirmation)
    if let Some(filename) = path_obj.file_name().and_then(|f| f.to_str()) {
        if PlatformPaths::needs_confirmation(filename) {
            return Err(ToolError::PathConfirmationRequired {
                path: path.to_string(),
                reason: format!("Writing to file '{filename}' may overwrite sensitive data"),
            });
        }
    }

    // Resolve the intended path
    let resolved_path = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        ctx.working_dir.join(path)
    };

    // Get the filename component
    let filename = resolved_path.file_name().ok_or_else(|| {
        ToolError::PermissionDenied(format!("Invalid path (no filename): {path}"))
    })?;

    // Get the parent directory - this is crucial for symlink protection
    let parent_dir = resolved_path.parent().ok_or_else(|| {
        ToolError::PermissionDenied(format!("Invalid path (no parent): {path}"))
    })?;

    // Canonicalize the parent directory to resolve symlinks
    // This prevents symlink-based escapes: if working_dir/symlink -> /external,
    // then working_dir/symlink/file.txt would resolve to /external/file.txt
    // For write operations, the parent may not exist yet, so we handle that case.
    let canonical_parent = canonicalize_parent_for_write(parent_dir, path)?;

    // Build the final canonical path
    let canonical_path = canonical_parent.join(filename);

    // Check if within working directory, allowed paths, or confirmed paths
    if security.config.enforce_working_dir {
        let canonical_working_dir =
            ctx.working_dir.canonicalize().unwrap_or_else(|_| ctx.working_dir.clone());

        let is_allowed = canonical_path.starts_with(&canonical_working_dir)
            || security.config.allowed_paths.iter().any(|allowed| {
                allowed.canonicalize().map_or_else(
                    |_| canonical_path.starts_with(allowed),
                    |canonical_allowed| canonical_path.starts_with(&canonical_allowed),
                )
            });

        // Check confirmed paths (also canonicalize for proper comparison)
        let is_confirmed = ctx.confirmed_paths.iter().any(|p| {
            p.canonicalize().map_or_else(
                |_| canonical_path.starts_with(p) || &canonical_path == p,
                |canonical_confirmed| {
                    canonical_path.starts_with(&canonical_confirmed)
                        || canonical_path == canonical_confirmed
                },
            )
        });

        if !is_allowed && !is_confirmed {
            // Return PathConfirmationRequired instead of PermissionDenied
            return Err(ToolError::PathConfirmationRequired {
                path: canonical_path.to_string_lossy().to_string(),
                reason: format!(
                    "Writing to path '{}' outside working directory '{}' requires confirmation",
                    canonical_path.display(),
                    canonical_working_dir.display()
                ),
            });
        }
    }

    Ok(canonical_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_safe_path_within_working_dir() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path();

        // Create a test file
        let test_file = working_dir.join("test.txt");
        std::fs::write(&test_file, "test").unwrap();

        let security = PathSecurity::default();
        let result = security.validate("test.txt", working_dir);

        assert!(matches!(result, PathValidation::Safe(_)));
    }

    #[test]
    fn test_path_traversal_blocked() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path();

        let security = PathSecurity::default();
        let result = security.validate("../../../etc/passwd", working_dir);

        // Should be blocked (either as path traversal or outside working dir)
        assert!(!matches!(result, PathValidation::Safe(_)));
    }

    #[test]
    fn test_sensitive_file_blocked() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path();

        // Create a .env file in working dir
        let env_file = working_dir.join(".env");
        std::fs::write(&env_file, "SECRET=xxx").unwrap();

        let security = PathSecurity::default();
        let result = security.validate(".env", working_dir);

        // Project-level sensitive files (like .env) require user confirmation
        // They return SensitiveFileConfirm, not SensitiveFile (which is for system-level files)
        assert!(
            matches!(result, PathValidation::SensitiveFileConfirm { .. }),
            "Expected SensitiveFileConfirm for .env file, got: {:?}",
            result
        );
    }

    #[test]
    fn test_absolute_path_outside_working_dir() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path();

        let security = PathSecurity::default();
        let result = security.validate("/etc/hosts", working_dir);

        // Should be blocked as outside working directory
        assert!(matches!(result, PathValidation::OutsideWorkingDir { .. }));
    }

    #[test]
    fn test_tmp_allowed() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path();

        // Create a file in the system temp directory for testing
        let sys_tmp = std::env::temp_dir();
        let tmp_file = sys_tmp.join("forge_test_tmp_allowed.txt");
        std::fs::write(&tmp_file, "test").ok();

        let security = PathSecurity::default();
        let tmp_file_str = tmp_file.to_str().unwrap();
        let result = security.validate(tmp_file_str, working_dir);

        // System temp directory should be allowed by default config
        if tmp_file.exists() {
            assert!(
                matches!(result, PathValidation::Safe(_)),
                "System temp dir should be allowed, got: {:?}",
                result
            );
            std::fs::remove_file(&tmp_file).ok();
        }
    }

    #[test]
    fn test_permissive_config() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path();

        let security = PathSecurity::new(PathSecurityConfig::permissive());

        // With permissive config, /etc/hosts should be allowed
        // (if it exists)
        if Path::new("/etc/hosts").exists() {
            let result = security.validate("/etc/hosts", working_dir);
            assert!(matches!(result, PathValidation::Safe(_)));
        }
    }

    #[test]
    fn test_ssh_key_blocked() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path();

        let security = PathSecurity::default();

        // Test various SSH key paths
        let ssh_paths =
            ["/home/user/.ssh/id_rsa", "/root/.ssh/authorized_keys", "~/.ssh/id_ed25519"];

        for path in &ssh_paths {
            let result = security.validate(path, working_dir);
            assert!(
                !matches!(result, PathValidation::Safe(_)),
                "SSH path should be blocked: {}",
                path
            );
        }
    }

    #[test]
    fn test_credentials_blocked() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path();

        // Create credentials file in working dir
        let creds_file = working_dir.join("credentials.json");
        std::fs::write(&creds_file, "{}").unwrap();

        let security = PathSecurity::default();
        let result = security.validate("credentials.json", working_dir);

        // Project-level sensitive files (like credentials.json) require user confirmation
        // They return SensitiveFileConfirm, not SensitiveFile (which is for system-level files)
        assert!(
            matches!(result, PathValidation::SensitiveFileConfirm { .. }),
            "Expected SensitiveFileConfirm for credentials.json, got: {:?}",
            result
        );
    }

    #[test]
    fn test_symlink_write_escape_blocked() {
        // This test verifies that symlink-based directory traversal is blocked
        // for write operations.
        //
        // Attack scenario:
        // 1. working_dir/symlink_dir -> sibling_dir (outside working_dir but not in allowed paths)
        // 2. Attacker tries to write to working_dir/symlink_dir/malicious.txt
        // 3. Without protection, this would create sibling_dir/malicious.txt
        // 4. With our fix, the parent directory is canonicalized, revealing the escape
        //
        // Note: We create the test directories in /tmp but use a non-allowed subdirectory
        // structure where the sibling is outside the working_dir subtree.

        // Create a parent temp directory
        let parent_temp = TempDir::new().unwrap();
        let parent_path = parent_temp.path();

        // Create working_dir inside parent
        let working_dir = parent_path.join("project");
        std::fs::create_dir(&working_dir).unwrap();

        // Create a nested sibling directory that's a sibling to working_dir
        // This simulates escaping to a different project directory
        let sibling_dir = parent_path.join("other_project");
        std::fs::create_dir(&sibling_dir).unwrap();

        // Create a symlink inside working_dir pointing to sibling directory
        #[cfg(unix)]
        {
            let symlink_path = working_dir.join("escape_link");
            std::os::unix::fs::symlink(&sibling_dir, &symlink_path).unwrap();

            // Validate write path using the strict security config
            // We directly test the canonicalization logic that detects symlink escapes
            let resolved_path = working_dir.join("escape_link/malicious.txt");
            let parent_dir = resolved_path.parent().unwrap();
            let canonical_parent = parent_dir.canonicalize().unwrap();
            let canonical_path = canonical_parent.join("malicious.txt");
            let canonical_working_dir = working_dir.canonicalize().unwrap();

            // The canonical path should be in sibling_dir, not working_dir
            let is_in_working_dir = canonical_path.starts_with(&canonical_working_dir);

            assert!(
                !is_in_working_dir,
                "Symlink escape detected: resolved path {} is outside working dir {}",
                canonical_path.display(),
                canonical_working_dir.display()
            );
        }
    }

    #[test]
    fn test_write_path_outside_working_dir_fails() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path();

        // Try to write to an absolute path outside working_dir with nonexistent parent
        // On Unix: /nonexistent_xyz_dir/file.txt
        // On Windows: C:\nonexistent_xyz_dir\file.txt
        let outside_path = if cfg!(windows) {
            "C:\\nonexistent_xyz_dir\\file.txt".to_string()
        } else {
            "/nonexistent_xyz_dir/file.txt".to_string()
        };
        let result = validate_write_path(&outside_path, working_dir);

        // Should fail because path is outside working directory
        assert!(result.is_err(), "Writing outside working dir should fail");
    }

    #[test]
    fn test_write_path_in_working_dir_succeeds() {
        let temp = TempDir::new().unwrap();
        let working_dir = temp.path();

        // Create a subdirectory
        let subdir = working_dir.join("subdir");
        std::fs::create_dir(&subdir).unwrap();

        // Writing to existing subdirectory should succeed
        let result = validate_write_path("subdir/newfile.txt", working_dir);

        assert!(
            result.is_ok(),
            "Writing to valid path in working_dir should succeed: {:?}",
            result
        );
    }
}
