//! Trust-aware permission management
//!
//! This module extends the base `PermissionManager` with trust level support.

use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::hardcoded_safety::HardcodedSafety;
use crate::path_utils::normalize_path;
use crate::permission::{PermissionConfig, PermissionManager};
use crate::permission_policy::{PermissionPolicy, PermissionRule, PolicyResult};
use crate::trust_level::{PermissionCheckResult, TrustLevel};
use crate::ConfirmationLevel;

/// Trust-aware permission manager
///
/// Extends `PermissionManager` with trust level support, hardcoded safety,
/// and pattern-matching permission policy.
pub struct TrustAwarePermissionManager {
    /// Base permission manager
    base: PermissionManager,
    /// Current trust level
    trust_level: TrustLevel,
    /// Hardcoded safety checker
    hardcoded_safety: HardcodedSafety,
    /// Pattern-matching permission policy
    permission_policy: PermissionPolicy,
    /// Project root for boundary detection
    project_root: Option<PathBuf>,
}

impl TrustAwarePermissionManager {
    /// Create a new trust-aware permission manager
    #[must_use]
    pub fn new(config: PermissionConfig) -> Self {
        Self {
            base: PermissionManager::new(config),
            trust_level: TrustLevel::default(),
            hardcoded_safety: HardcodedSafety::new(),
            permission_policy: PermissionPolicy::new(),
            project_root: None,
        }
    }

    /// Set permission rules from config
    ///
    /// Converts `PermissionRuleConfig` from forge-config into `PermissionRule`
    /// and rebuilds the policy engine.
    pub fn set_permission_rules(&mut self, rules: Vec<forge_config::PermissionRuleConfig>) {
        let converted: Vec<PermissionRule> = rules
            .into_iter()
            .map(|r| PermissionRule {
                pattern: r.pattern,
                action: match r.action {
                    forge_config::PolicyAction::Allow => {
                        crate::permission_policy::PolicyAction::Allow
                    }
                    forge_config::PolicyAction::Deny => crate::permission_policy::PolicyAction::Deny,
                },
                operations: r
                    .operations
                    .into_iter()
                    .map(|op| match op {
                        forge_config::OperationType::Read => {
                            crate::permission_policy::OperationType::Read
                        }
                        forge_config::OperationType::Write => {
                            crate::permission_policy::OperationType::Write
                        }
                        forge_config::OperationType::Execute => {
                            crate::permission_policy::OperationType::Execute
                        }
                    })
                    .collect(),
                description: r.description,
            })
            .collect();
        self.permission_policy = PermissionPolicy::with_rules(converted);
    }

    /// Set the trust level
    #[allow(clippy::missing_const_for_fn)]
    pub fn set_trust_level(&mut self, level: TrustLevel) {
        self.trust_level = level;
    }

    /// Get the current trust level
    #[must_use]
    pub const fn trust_level(&self) -> TrustLevel {
        self.trust_level
    }

    /// Set the project root
    pub fn set_project_root(&mut self, root: PathBuf) {
        self.hardcoded_safety = HardcodedSafety::new().with_project_root(root.clone());
        self.project_root = Some(root);
    }

    /// Record that a command was confirmed by the user
    ///
    /// This delegates to the base permission manager for "Once" level caching.
    pub fn record_confirmation(&mut self, tool: &str, params: &Value) {
        self.base.record_confirmation(tool, params);
    }

    /// Check permission with trust level and hardcoded safety
    ///
    /// The `working_dir` is used to resolve relative paths for project boundary checks.
    #[must_use]
    pub fn check(
        &self,
        tool: &str,
        params: &Value,
        level: ConfirmationLevel,
        working_dir: &Path,
    ) -> PermissionCheckResult {
        // 1. Check hardcoded safety first (unbypassable)
        if let Some(reason) = self.hardcoded_safety.check(tool, params, working_dir) {
            return PermissionCheckResult::HardBlocked { reason };
        }

        // 2. Check permission policy rules (pattern-matching, applies to all trust levels)
        let policy_allowed = match self.permission_policy.evaluate_tool(tool, params, working_dir) {
            PolicyResult::Denied { pattern, description } => {
                let reason = description.map_or_else(
                    || format!("Permission policy denied: {pattern}"),
                    |desc| format!("Permission policy denied: {pattern} ({desc})"),
                );
                return PermissionCheckResult::Denied { reason };
            }
            PolicyResult::Allowed => true,
            PolicyResult::NoMatch => false,
        };

        // 3. Check deny rules (applies to all trust levels except Yolo).
        // Even an explicit policy Allow does not bypass base deny rules —
        // this prevents project-level configs from overriding workspace-level denies.
        if self.trust_level != TrustLevel::Yolo {
            if let Some(reason) = self.base.is_denied(tool, params) {
                return PermissionCheckResult::Denied { reason };
            }
        }

        // 4. If policy explicitly allowed, skip trust-level confirmation
        if policy_allowed {
            return PermissionCheckResult::Allowed;
        }

        // 5. Yolo mode skips other checks
        if self.trust_level == TrustLevel::Yolo {
            return PermissionCheckResult::Allowed;
        }

        // 6. Check by trust level
        self.check_by_trust_level(tool, params, level, working_dir)
    }

    /// Check permission based on trust level
    fn check_by_trust_level(
        &self,
        tool: &str,
        params: &Value,
        level: ConfirmationLevel,
        working_dir: &Path,
    ) -> PermissionCheckResult {
        match self.trust_level {
            TrustLevel::Cautious => {
                // In Cautious mode, all operations with confirmation level need confirmation
                if level == ConfirmationLevel::None {
                    PermissionCheckResult::Allowed
                } else {
                    PermissionCheckResult::NeedsConfirmation { level, reason: None }
                }
            }
            TrustLevel::Development => {
                // Project-internal operations auto-allowed (except dangerous)
                if self.is_within_project(params, working_dir)
                    && level != ConfirmationLevel::Dangerous
                {
                    PermissionCheckResult::Allowed
                } else {
                    self.base.check(tool, params, level).into()
                }
            }
            TrustLevel::Trusted => {
                // Only dangerous commands need confirmation
                match level {
                    ConfirmationLevel::None | ConfirmationLevel::Once => {
                        PermissionCheckResult::Allowed
                    }
                    _ => self.base.check(tool, params, level).into(),
                }
            }
            TrustLevel::Yolo => unreachable!(),
        }
    }

    /// Check if operation is within project boundary
    ///
    /// Uses path normalization to prevent path traversal attacks.
    /// Converts relative paths to absolute before checking.
    /// Returns false if no paths can be extracted (fail-safe).
    fn is_within_project(&self, params: &Value, working_dir: &Path) -> bool {
        let Some(ref root) = self.project_root else {
            return false;
        };

        // Normalize root for comparison
        let normalized_root = root.canonicalize().unwrap_or_else(|_| normalize_path(root));

        // Check file_path parameter
        if let Some(path_str) = params.get("file_path").and_then(Value::as_str) {
            let path = Path::new(path_str);
            // Convert relative paths to absolute using working_dir
            let abs_path =
                if path.is_relative() { working_dir.join(path) } else { path.to_path_buf() };
            let normalized = abs_path.canonicalize().unwrap_or_else(|_| normalize_path(&abs_path));
            return normalized.starts_with(&normalized_root);
        }

        // Check command for shell tools
        if let Some(cmd) = params.get("command").and_then(Value::as_str) {
            let paths = crate::shell_path::extract_paths_from_command(cmd);
            // If no paths extracted, fail-safe: return false
            if paths.is_empty() {
                return false;
            }
            return paths.iter().all(|p| {
                // Convert relative paths to absolute using working_dir
                let abs_path = if p.is_relative() { working_dir.join(p) } else { p.clone() };
                let normalized =
                    abs_path.canonicalize().unwrap_or_else(|_| normalize_path(&abs_path));
                normalized.starts_with(&normalized_root)
            });
        }

        // No file_path or command found - fail-safe: return false
        false
    }
}

/// Detect project root from a path
///
/// Searches for project markers in ancestor directories.
#[must_use]
pub fn detect_project_root(path: &Path) -> PathBuf {
    for ancestor in path.ancestors() {
        // Check for .forge directory
        if ancestor.join(".forge").is_dir() {
            return ancestor.to_path_buf();
        }
        // Check for .git directory
        if ancestor.join(".git").is_dir() {
            return ancestor.to_path_buf();
        }
        // Check for build config files
        if ancestor.join("Cargo.toml").is_file() {
            return ancestor.to_path_buf();
        }
        if ancestor.join("package.json").is_file() {
            return ancestor.to_path_buf();
        }
        if ancestor.join("pyproject.toml").is_file() {
            return ancestor.to_path_buf();
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_working_dir() -> PathBuf {
        PathBuf::from("/home/user/project")
    }

    #[test]
    fn test_yolo_mode_allows_most_operations() {
        let mut manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        manager.set_trust_level(TrustLevel::Yolo);
        let wd = test_working_dir();

        let params = serde_json::json!({"command": "ls -la"});
        let result = manager.check("bash", &params, ConfirmationLevel::Always, &wd);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_yolo_mode_blocks_dangerous() {
        let mut manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        manager.set_trust_level(TrustLevel::Yolo);
        let wd = test_working_dir();

        let params = serde_json::json!({"command": "rm -rf /"});
        let result = manager.check("bash", &params, ConfirmationLevel::Always, &wd);
        assert!(result.is_hard_blocked());
    }

    #[test]
    fn test_cautious_mode_requires_confirmation() {
        let manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        let wd = test_working_dir();

        let params = serde_json::json!({"command": "echo hello"});
        let result = manager.check("bash", &params, ConfirmationLevel::Always, &wd);
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_cautious_mode_allows_none_level() {
        let manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        let wd = test_working_dir();

        let params = serde_json::json!({"command": "echo hello"});
        let result = manager.check("bash", &params, ConfirmationLevel::None, &wd);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_development_mode_allows_project_internal() {
        let mut manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        manager.set_trust_level(TrustLevel::Development);
        manager.set_project_root(PathBuf::from("/home/user/project"));
        let wd = test_working_dir();

        // File within project should be allowed
        let params = serde_json::json!({"file_path": "/home/user/project/src/main.rs"});
        let result = manager.check("write", &params, ConfirmationLevel::Once, &wd);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_development_mode_requires_confirmation_outside_project() {
        let mut manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        manager.set_trust_level(TrustLevel::Development);
        manager.set_project_root(PathBuf::from("/home/user/project"));
        let wd = test_working_dir();

        // File outside project should require confirmation
        let params = serde_json::json!({"file_path": "/home/user/other/file.txt"});
        let result = manager.check("write", &params, ConfirmationLevel::Once, &wd);
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_trusted_mode_allows_once_level() {
        let mut manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        manager.set_trust_level(TrustLevel::Trusted);
        let wd = test_working_dir();

        let params = serde_json::json!({"command": "custom-script.sh"});
        let result = manager.check("bash", &params, ConfirmationLevel::Once, &wd);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_trusted_mode_requires_confirmation_for_dangerous() {
        let mut manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        manager.set_trust_level(TrustLevel::Trusted);
        let wd = test_working_dir();

        let params = serde_json::json!({"command": "custom-script.sh"});
        let result = manager.check("bash", &params, ConfirmationLevel::Dangerous, &wd);
        assert!(!result.is_allowed());
    }

    #[test]
    fn test_detect_project_root_with_git() {
        // This test uses the actual project root
        let current_dir = std::env::current_dir().unwrap();
        let detected = detect_project_root(&current_dir);
        // Should find a project root (either .git or Cargo.toml)
        assert!(detected.join(".git").is_dir() || detected.join("Cargo.toml").is_file());
    }

    // ==================== Permission Policy Integration Tests ====================

    #[test]
    fn test_policy_denies_env_write() {
        let manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        let wd = test_working_dir();

        // Writing to .env should be denied by built-in policy
        let params = serde_json::json!({"file_path": "/home/user/project/.env"});
        let result = manager.check("write", &params, ConfirmationLevel::None, &wd);
        assert!(matches!(result, PermissionCheckResult::Denied { .. }));
    }

    #[test]
    fn test_policy_allows_env_example_write() {
        let mut manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        manager.set_trust_level(TrustLevel::Yolo);
        let wd = test_working_dir();

        // Writing to .env.example should be explicitly allowed by policy
        let params = serde_json::json!({"file_path": "/home/user/project/.env.example"});
        let result = manager.check("write", &params, ConfirmationLevel::None, &wd);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_policy_denies_env_even_in_yolo() {
        let mut manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        manager.set_trust_level(TrustLevel::Yolo);
        let wd = test_working_dir();

        // Policy deny rules apply even in Yolo mode (checked before trust level)
        let params = serde_json::json!({"file_path": "/home/user/project/.env"});
        let result = manager.check("write", &params, ConfirmationLevel::None, &wd);
        assert!(matches!(result, PermissionCheckResult::Denied { .. }));
    }

    #[test]
    fn test_policy_env_read_defers_to_trust() {
        let manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        let wd = test_working_dir();

        // Reading .env is not blocked by policy (only write is), defers to trust level
        let params = serde_json::json!({"file_path": "/home/user/project/.env"});
        let result = manager.check("read", &params, ConfirmationLevel::None, &wd);
        // Cautious mode with None level → allowed
        assert!(result.is_allowed());
    }

    #[test]
    fn test_policy_user_rules_override_builtin() {
        let mut manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        // User rule: allow .env writes
        manager.set_permission_rules(vec![forge_config::PermissionRuleConfig {
            pattern: ".env".to_string(),
            action: forge_config::PolicyAction::Allow,
            operations: vec![forge_config::OperationType::Write],
            description: Some("Allow .env in this project".to_string()),
        }]);
        manager.set_trust_level(TrustLevel::Yolo);
        let wd = test_working_dir();

        let params = serde_json::json!({"file_path": "/home/user/project/.env"});
        let result = manager.check("write", &params, ConfirmationLevel::None, &wd);
        // User rule takes priority → allowed
        assert!(result.is_allowed());
    }

    #[test]
    fn test_policy_denies_ssh_key_read() {
        let manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        let wd = test_working_dir();

        let params = serde_json::json!({"file_path": "/home/user/.ssh/id_rsa"});
        let result = manager.check("read", &params, ConfirmationLevel::None, &wd);
        assert!(matches!(result, PermissionCheckResult::Denied { .. }));
    }

    #[test]
    fn test_policy_normal_file_defers_to_trust() {
        let manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        let wd = test_working_dir();

        // Normal file should not be affected by policy
        let params = serde_json::json!({"file_path": "/home/user/project/src/main.rs"});
        let result = manager.check("write", &params, ConfirmationLevel::Once, &wd);
        // Cautious mode with Once level → needs confirmation (policy didn't match)
        assert!(matches!(result, PermissionCheckResult::NeedsConfirmation { .. }));
    }
}
