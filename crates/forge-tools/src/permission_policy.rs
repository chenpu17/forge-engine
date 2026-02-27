//! Pattern-matching permission policy for fine-grained file access control
//!
//! This module provides glob-based permission rules that sit between the
//! hardcoded safety layer and the trust-level system. Rules use first-match-wins
//! semantics, with built-in rules for common sensitive files.

use glob::Pattern;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::path_utils::normalize_path;

/// Action to take when a rule matches
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyAction {
    /// Allow the operation
    Allow,
    /// Deny the operation
    Deny,
}

/// Operation type that a rule applies to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationType {
    /// File read operations
    Read,
    /// File write/edit operations
    Write,
    /// Shell command execution
    Execute,
}

/// A single permission rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    /// Glob pattern to match against file paths (e.g., "*.env", ".env*", "/etc/**")
    pub pattern: String,
    /// Action to take when matched
    pub action: PolicyAction,
    /// Operations this rule applies to (empty = all operations)
    #[serde(default)]
    pub operations: Vec<OperationType>,
    /// Optional human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Permission policy engine
///
/// Evaluates glob-based rules in first-match-wins order.
/// Built-in rules are appended after user rules.
pub struct PermissionPolicy {
    /// Compiled rules (user rules first, then built-in)
    rules: Vec<CompiledRule>,
}

/// A compiled permission rule with pre-parsed glob pattern
struct CompiledRule {
    /// Compiled glob pattern
    pattern: Pattern,
    /// Original pattern string (for display)
    pattern_str: String,
    /// Action to take
    action: PolicyAction,
    /// Operations this rule applies to (empty = all)
    operations: Vec<OperationType>,
    /// Description
    description: Option<String>,
}

impl CompiledRule {
    /// Check if this rule applies to the given operation type
    fn applies_to(&self, op: OperationType) -> bool {
        self.operations.is_empty() || self.operations.contains(&op)
    }

    /// Check if the pattern matches a path
    ///
    /// Matches against both the full path and the filename component.
    fn matches(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        // Try matching against full path
        if self.pattern.matches(&path_str) {
            return true;
        }

        // Try matching against just the filename
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if self.pattern.matches(name) {
                return true;
            }
        }

        false
    }
}

/// Result of a policy evaluation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyResult {
    /// A rule matched and allows the operation
    Allowed,
    /// A rule matched and denies the operation
    Denied {
        /// The pattern that matched
        pattern: String,
        /// Optional description
        description: Option<String>,
    },
    /// No rule matched — defer to trust level system
    NoMatch,
}

/// Built-in rules that protect common sensitive files
fn builtin_rules() -> Vec<PermissionRule> {
    vec![
        // Allow .env.example (must come before .env* deny)
        PermissionRule {
            pattern: "*.env.example".to_string(),
            action: PolicyAction::Allow,
            operations: vec![],
            description: Some("Allow .env.example files".to_string()),
        },
        PermissionRule {
            pattern: ".env.example".to_string(),
            action: PolicyAction::Allow,
            operations: vec![],
            description: Some("Allow .env.example files".to_string()),
        },
        // Deny .env files (all variants: .env, .env.local, .env.production, etc.)
        PermissionRule {
            pattern: ".env".to_string(),
            action: PolicyAction::Deny,
            operations: vec![OperationType::Write],
            description: Some("Protect environment files from writes".to_string()),
        },
        PermissionRule {
            pattern: ".env.*".to_string(),
            action: PolicyAction::Deny,
            operations: vec![OperationType::Write],
            description: Some("Protect environment files from writes".to_string()),
        },
        PermissionRule {
            pattern: "*.env".to_string(),
            action: PolicyAction::Deny,
            operations: vec![OperationType::Write],
            description: Some("Protect environment files from writes".to_string()),
        },
        // Deny SSH keys (all common types)
        PermissionRule {
            pattern: "id_rsa".to_string(),
            action: PolicyAction::Deny,
            operations: vec![],
            description: Some("Protect SSH private keys".to_string()),
        },
        PermissionRule {
            pattern: "id_ed25519".to_string(),
            action: PolicyAction::Deny,
            operations: vec![],
            description: Some("Protect SSH private keys".to_string()),
        },
        PermissionRule {
            pattern: "id_ecdsa*".to_string(),
            action: PolicyAction::Deny,
            operations: vec![],
            description: Some("Protect SSH private keys".to_string()),
        },
        PermissionRule {
            pattern: "id_dsa*".to_string(),
            action: PolicyAction::Deny,
            operations: vec![],
            description: Some("Protect SSH private keys".to_string()),
        },
        PermissionRule {
            pattern: "authorized_keys".to_string(),
            action: PolicyAction::Deny,
            operations: vec![OperationType::Write],
            description: Some("Protect SSH authorized_keys".to_string()),
        },
        PermissionRule {
            pattern: "*.pem".to_string(),
            action: PolicyAction::Deny,
            operations: vec![OperationType::Write],
            description: Some("Protect PEM certificate/key files".to_string()),
        },
        // Deny credential files
        PermissionRule {
            pattern: "credentials.json".to_string(),
            action: PolicyAction::Deny,
            operations: vec![OperationType::Write],
            description: Some("Protect credential files".to_string()),
        },
        PermissionRule {
            pattern: "*.key".to_string(),
            action: PolicyAction::Deny,
            operations: vec![OperationType::Write],
            description: Some("Protect key files".to_string()),
        },
    ]
}

impl PermissionPolicy {
    /// Create a new policy with only built-in rules
    #[must_use]
    pub fn new() -> Self {
        Self::with_rules(vec![])
    }

    /// Create a policy with user rules prepended before built-in rules
    ///
    /// User rules take priority (first-match-wins).
    #[must_use]
    pub fn with_rules(user_rules: Vec<PermissionRule>) -> Self {
        let mut all_rules = user_rules;
        all_rules.extend(builtin_rules());

        let rules = all_rules
            .into_iter()
            .filter_map(|rule| match Pattern::new(&rule.pattern) {
                Ok(pattern) => Some(CompiledRule {
                    pattern,
                    pattern_str: rule.pattern,
                    action: rule.action,
                    operations: rule.operations,
                    description: rule.description,
                }),
                Err(e) => {
                    tracing::warn!(
                        pattern = %rule.pattern,
                        error = %e,
                        "Invalid glob pattern in permission rule, skipping"
                    );
                    None
                }
            })
            .collect();

        Self { rules }
    }

    /// Evaluate a file path against the policy
    ///
    /// The `working_dir` is used to resolve relative paths.
    /// Returns `PolicyResult::NoMatch` if no rule matches (defer to trust system).
    #[must_use]
    pub fn evaluate(
        &self,
        path: &Path,
        operation: OperationType,
        working_dir: &Path,
    ) -> PolicyResult {
        // Resolve to absolute path
        let abs_path = if path.is_relative() { working_dir.join(path) } else { path.to_path_buf() };
        // Resolve symlinks when possible (prevents symlink bypass attacks).
        // Falls back to lexical normalization for non-existent paths.
        let normalized =
            std::fs::canonicalize(&abs_path).unwrap_or_else(|_| normalize_path(&abs_path));

        for rule in &self.rules {
            if rule.applies_to(operation) && rule.matches(&normalized) {
                return match rule.action {
                    PolicyAction::Allow => PolicyResult::Allowed,
                    PolicyAction::Deny => PolicyResult::Denied {
                        pattern: rule.pattern_str.clone(),
                        description: rule.description.clone(),
                    },
                };
            }
        }

        PolicyResult::NoMatch
    }

    /// Evaluate a tool call by extracting paths from params
    ///
    /// Returns `PolicyResult::NoMatch` if no paths found or no rules match.
    ///
    /// For shell tools (`bash`, `shell`, `powershell`), path extraction is
    /// heuristic-based — it parses literal paths from the command string but
    /// cannot resolve shell variables, subshell expansions, or dynamically
    /// constructed paths. This is a best-effort defense layer, not a guarantee.
    #[must_use]
    pub fn evaluate_tool(
        &self,
        tool: &str,
        params: &serde_json::Value,
        working_dir: &Path,
    ) -> PolicyResult {
        // Determine operation type from tool name.
        // Unknown tools with a `file_path` param are treated as Read by default
        // to ensure the policy still evaluates sensitive file access.
        let operation = match tool {
            "read" | "glob" | "grep" => OperationType::Read,
            "write" | "edit" => OperationType::Write,
            "bash" | "shell" | "powershell" => OperationType::Execute,
            _ => {
                // For unknown tools (e.g. MCP tools), check file_path against
                // all operation types and return the most restrictive result.
                if let Some(path_str) = params.get("file_path").and_then(serde_json::Value::as_str)
                {
                    let path = Path::new(path_str);
                    for op in [OperationType::Read, OperationType::Write, OperationType::Execute] {
                        let result = self.evaluate(path, op, working_dir);
                        if matches!(result, PolicyResult::Denied { .. }) {
                            return result;
                        }
                    }
                    return self.evaluate(path, OperationType::Read, working_dir);
                }
                return PolicyResult::NoMatch;
            }
        };

        // Extract file path from params
        if let Some(path_str) = params.get("file_path").and_then(serde_json::Value::as_str) {
            return self.evaluate(Path::new(path_str), operation, working_dir);
        }

        // For shell tools, check paths in command
        if matches!(tool, "bash" | "shell" | "powershell") {
            if let Some(command) = params.get("command").and_then(serde_json::Value::as_str) {
                let paths = crate::shell_path::extract_paths_from_command(command);
                let mut any_allowed = false;
                for path in &paths {
                    let result = self.evaluate(path, operation, working_dir);
                    match result {
                        PolicyResult::Denied { .. } => return result,
                        PolicyResult::Allowed => any_allowed = true,
                        PolicyResult::NoMatch => {}
                    }
                }

                // Also check redirect targets
                let redirects = crate::shell_path::extract_redirect_targets(command);
                for target in &redirects {
                    let result =
                        self.evaluate(Path::new(target), OperationType::Write, working_dir);
                    match result {
                        PolicyResult::Denied { .. } => return result,
                        PolicyResult::Allowed => any_allowed = true,
                        PolicyResult::NoMatch => {}
                    }
                }

                if any_allowed {
                    return PolicyResult::Allowed;
                }
            }
        }

        PolicyResult::NoMatch
    }
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn wd() -> PathBuf {
        PathBuf::from("/home/user/project")
    }

    // --- Built-in rule tests ---

    #[test]
    fn test_deny_env_file_write() {
        let policy = PermissionPolicy::new();
        let result =
            policy.evaluate(Path::new("/home/user/project/.env"), OperationType::Write, &wd());
        assert!(matches!(result, PolicyResult::Denied { .. }));
    }

    #[test]
    fn test_deny_env_variant_write() {
        let policy = PermissionPolicy::new();
        let result = policy.evaluate(
            Path::new("/home/user/project/.env.local"),
            OperationType::Write,
            &wd(),
        );
        assert!(matches!(result, PolicyResult::Denied { .. }));
    }

    #[test]
    fn test_allow_env_read() {
        let policy = PermissionPolicy::new();
        // .env read is not denied (only write is blocked by built-in rules)
        let result =
            policy.evaluate(Path::new("/home/user/project/.env"), OperationType::Read, &wd());
        assert_eq!(result, PolicyResult::NoMatch);
    }

    #[test]
    fn test_allow_env_example() {
        let policy = PermissionPolicy::new();
        let result = policy.evaluate(
            Path::new("/home/user/project/.env.example"),
            OperationType::Write,
            &wd(),
        );
        assert_eq!(result, PolicyResult::Allowed);
    }

    #[test]
    fn test_deny_ssh_key() {
        let policy = PermissionPolicy::new();
        let result =
            policy.evaluate(Path::new("/home/user/.ssh/id_rsa"), OperationType::Read, &wd());
        assert!(matches!(result, PolicyResult::Denied { .. }));
    }

    #[test]
    fn test_deny_pem_write() {
        let policy = PermissionPolicy::new();
        let result =
            policy.evaluate(Path::new("/home/user/certs/server.pem"), OperationType::Write, &wd());
        assert!(matches!(result, PolicyResult::Denied { .. }));
    }

    #[test]
    fn test_normal_file_no_match() {
        let policy = PermissionPolicy::new();
        let result = policy.evaluate(
            Path::new("/home/user/project/src/main.rs"),
            OperationType::Write,
            &wd(),
        );
        assert_eq!(result, PolicyResult::NoMatch);
    }

    // --- User rule tests ---

    #[test]
    fn test_user_rule_deny() {
        let policy = PermissionPolicy::with_rules(vec![PermissionRule {
            pattern: "*.secret".to_string(),
            action: PolicyAction::Deny,
            operations: vec![],
            description: Some("Block secret files".to_string()),
        }]);

        let result = policy.evaluate(
            Path::new("/home/user/project/config.secret"),
            OperationType::Read,
            &wd(),
        );
        assert!(matches!(result, PolicyResult::Denied { .. }));
    }

    #[test]
    fn test_user_rule_allow_overrides_builtin() {
        // User rule to allow .env writes (overrides built-in deny)
        let policy = PermissionPolicy::with_rules(vec![PermissionRule {
            pattern: ".env".to_string(),
            action: PolicyAction::Allow,
            operations: vec![OperationType::Write],
            description: Some("Allow .env writes in this project".to_string()),
        }]);

        let result =
            policy.evaluate(Path::new("/home/user/project/.env"), OperationType::Write, &wd());
        // User rule matches first → Allow
        assert_eq!(result, PolicyResult::Allowed);
    }

    #[test]
    fn test_first_match_wins() {
        let policy = PermissionPolicy::with_rules(vec![
            PermissionRule {
                pattern: "*.log".to_string(),
                action: PolicyAction::Allow,
                operations: vec![],
                description: None,
            },
            PermissionRule {
                pattern: "*.log".to_string(),
                action: PolicyAction::Deny,
                operations: vec![],
                description: None,
            },
        ]);

        let result =
            policy.evaluate(Path::new("/home/user/project/app.log"), OperationType::Read, &wd());
        // First rule wins
        assert_eq!(result, PolicyResult::Allowed);
    }

    // --- evaluate_tool tests ---

    #[test]
    fn test_evaluate_tool_write_env() {
        let policy = PermissionPolicy::new();
        let params = json!({"file_path": "/home/user/project/.env"});
        let result = policy.evaluate_tool("write", &params, &wd());
        assert!(matches!(result, PolicyResult::Denied { .. }));
    }

    #[test]
    fn test_evaluate_tool_read_normal() {
        let policy = PermissionPolicy::new();
        let params = json!({"file_path": "/home/user/project/src/main.rs"});
        let result = policy.evaluate_tool("read", &params, &wd());
        assert_eq!(result, PolicyResult::NoMatch);
    }

    #[test]
    fn test_evaluate_tool_unknown_tool_sensitive() {
        let policy = PermissionPolicy::new();
        // Unknown tools with sensitive file_path should be denied
        // (checks all operation types, Write deny on .env triggers)
        let params = json!({"file_path": "/home/user/.env"});
        let result = policy.evaluate_tool("custom_tool", &params, &wd());
        assert!(matches!(result, PolicyResult::Denied { .. }));
    }

    #[test]
    fn test_evaluate_tool_unknown_tool_normal() {
        let policy = PermissionPolicy::new();
        // Unknown tools with normal file_path should not match
        let params = json!({"file_path": "/home/user/project/src/main.rs"});
        let result = policy.evaluate_tool("custom_tool", &params, &wd());
        assert_eq!(result, PolicyResult::NoMatch);
    }

    // --- Relative path tests ---

    #[test]
    fn test_relative_path_resolved() {
        let policy = PermissionPolicy::new();
        let result = policy.evaluate(Path::new(".env"), OperationType::Write, &wd());
        assert!(matches!(result, PolicyResult::Denied { .. }));
    }

    #[test]
    fn test_path_traversal_normalized() {
        let policy = PermissionPolicy::new();
        let result = policy.evaluate(
            Path::new("/home/user/project/src/../../project/.env"),
            OperationType::Write,
            &wd(),
        );
        assert!(matches!(result, PolicyResult::Denied { .. }));
    }

    // --- Serde tests ---

    #[test]
    fn test_permission_rule_serde_roundtrip() {
        let rule = PermissionRule {
            pattern: "*.env".to_string(),
            action: PolicyAction::Deny,
            operations: vec![OperationType::Read, OperationType::Write],
            description: Some("Block env files".to_string()),
        };

        let json = serde_json::to_string(&rule).expect("serialize");
        let parsed: PermissionRule = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.pattern, "*.env");
        assert_eq!(parsed.action, PolicyAction::Deny);
        assert_eq!(parsed.operations.len(), 2);
    }

    #[test]
    fn test_permission_rule_toml_roundtrip() {
        let rule = PermissionRule {
            pattern: ".env*".to_string(),
            action: PolicyAction::Deny,
            operations: vec![OperationType::Write],
            description: None,
        };

        let toml_str = toml::to_string(&rule).expect("serialize");
        let parsed: PermissionRule = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(parsed.pattern, ".env*");
        assert_eq!(parsed.action, PolicyAction::Deny);
    }

    #[test]
    fn test_operation_specific_rule() {
        let policy = PermissionPolicy::with_rules(vec![PermissionRule {
            pattern: "*.sql".to_string(),
            action: PolicyAction::Deny,
            operations: vec![OperationType::Execute],
            description: Some("Block SQL file execution".to_string()),
        }]);

        // Read should not match (rule only applies to Execute)
        let result =
            policy.evaluate(Path::new("/home/user/project/schema.sql"), OperationType::Read, &wd());
        assert_eq!(result, PolicyResult::NoMatch);

        // Execute should be denied
        let result = policy.evaluate(
            Path::new("/home/user/project/schema.sql"),
            OperationType::Execute,
            &wd(),
        );
        assert!(matches!(result, PolicyResult::Denied { .. }));
    }
}
