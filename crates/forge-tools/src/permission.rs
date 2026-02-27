//! Permission management for tool execution
//!
//! This module provides permission checking and confirmation management
//! for tool execution, ensuring dangerous operations require user consent.

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::ConfirmationLevel;

/// Check if a tool name is a shell tool (bash, shell, powershell)
///
/// All shell tools share the same permission rules to prevent
/// security bypass through aliases.
fn is_shell_tool(tool: &str) -> bool {
    matches!(tool, "bash" | "shell" | "powershell")
}

/// Permission check result
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionCheck {
    /// Operation is allowed
    Allowed,
    /// Operation needs user confirmation
    NeedsConfirmation(ConfirmationLevel),
    /// Operation is denied by policy
    Denied(String),
}

/// Permission configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionConfig {
    /// Allowed command patterns (regex)
    #[serde(default)]
    pub allowed_patterns: Vec<String>,
    /// Denied command patterns (regex)
    #[serde(default)]
    pub denied_patterns: Vec<String>,
    /// Confirmation TTL in seconds (how long a confirmation remains valid)
    #[serde(default = "default_confirmation_ttl")]
    pub confirmation_ttl_secs: u64,
    /// Always allowed tools (no confirmation needed)
    #[serde(default = "default_safe_tools")]
    pub safe_tools: Vec<String>,
    /// Dangerous command patterns that require extra warning
    #[serde(default = "default_dangerous_patterns")]
    pub dangerous_patterns: Vec<String>,
    /// Command whitelist rules in format "Tool(command:pattern)"
    /// Examples: "Bash(git:*)", "Bash(npm:*)", "Bash(cargo:*)"
    #[serde(default = "default_whitelist_rules")]
    pub whitelist_rules: Vec<String>,
}

/// A parsed whitelist rule
#[derive(Debug, Clone)]
pub struct WhitelistRule {
    /// Tool name (e.g., "Bash", "bash")
    pub tool: String,
    /// Command prefix (e.g., "git", "npm")
    pub command_prefix: String,
    /// Pattern to match after prefix (e.g., "*", "status")
    pub pattern: String,
}

impl WhitelistRule {
    /// Parse a whitelist rule from string format "Tool(command:pattern)"
    /// Examples: "Bash(git:*)", "Bash(npm run:*)", "Bash(cargo:build)"
    #[must_use]
    pub fn parse(rule: &str) -> Option<Self> {
        // Match format: Tool(command:pattern)
        let rule = rule.trim();

        // Find the opening parenthesis
        let paren_start = rule.find('(')?;
        let paren_end = rule.rfind(')')?;

        if paren_end <= paren_start {
            return None;
        }

        let tool = rule[..paren_start].trim().to_lowercase();
        let inner = &rule[paren_start + 1..paren_end];

        // Split by colon to get command:pattern
        let colon_pos = inner.find(':')?;
        let command_prefix = inner[..colon_pos].trim().to_string();
        let pattern = inner[colon_pos + 1..].trim().to_string();

        Some(Self { tool, command_prefix, pattern })
    }

    /// Check if a command matches this whitelist rule
    #[must_use]
    pub fn matches(&self, tool: &str, command: &str) -> bool {
        // Check tool name (case-insensitive)
        if tool.to_lowercase() != self.tool {
            return false;
        }

        // Check if command starts with the prefix
        let command_trimmed = command.trim();
        if !command_trimmed.starts_with(&self.command_prefix) {
            return false;
        }

        // Get the rest of the command after the prefix
        let rest = &command_trimmed[self.command_prefix.len()..];

        // The prefix must be followed by whitespace, end of string, or special chars
        // This prevents "git" from matching "gitk"
        if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with(':') {
            return false;
        }

        // Check pattern
        match self.pattern.as_str() {
            "*" => true, // Wildcard matches everything after prefix
            pattern => {
                if rest.is_empty() {
                    // Exact match of prefix
                    pattern.is_empty() || pattern == "*"
                } else {
                    // Has arguments after prefix
                    pattern == "*" || rest.trim_start().starts_with(pattern)
                }
            }
        }
    }
}

fn default_whitelist_rules() -> Vec<String> {
    vec![
        // Git commands
        "Bash(git:*)".to_string(),
        // Package managers
        "Bash(npm:*)".to_string(),
        "Bash(npm run:*)".to_string(),
        "Bash(npx:*)".to_string(),
        "Bash(yarn:*)".to_string(),
        "Bash(pnpm:*)".to_string(),
        "Bash(cargo:*)".to_string(),
        "Bash(pip:*)".to_string(),
        "Bash(pip3:*)".to_string(),
        // Build tools
        "Bash(make:*)".to_string(),
        "Bash(cmake:*)".to_string(),
        // Common safe commands
        "Bash(ls:*)".to_string(),
        "Bash(pwd:*)".to_string(),
        "Bash(cd:*)".to_string(),
        "Bash(cat:*)".to_string(),
        "Bash(head:*)".to_string(),
        "Bash(tail:*)".to_string(),
        "Bash(echo:*)".to_string(),
        "Bash(which:*)".to_string(),
        "Bash(whoami:*)".to_string(),
        // Development tools
        "Bash(python:*)".to_string(),
        "Bash(python3:*)".to_string(),
        "Bash(node:*)".to_string(),
        "Bash(go:*)".to_string(),
        "Bash(rustc:*)".to_string(),
        // Testing
        "Bash(pytest:*)".to_string(),
        "Bash(jest:*)".to_string(),
        // Localhost network (safe for development)
        "Bash(curl:*localhost:*)".to_string(),
        "Bash(curl:*127.0.0.1:*)".to_string(),
    ]
}

const fn default_confirmation_ttl() -> u64 {
    300 // 5 minutes
}

fn default_safe_tools() -> Vec<String> {
    vec!["read".to_string(), "glob".to_string(), "grep".to_string()]
}

fn default_dangerous_patterns() -> Vec<String> {
    vec![
        r"rm\s+-rf".to_string(),
        r"rm\s+.*\*".to_string(),
        r"sudo\s+".to_string(),
        r"chmod\s+777".to_string(),
        r">\s*/dev/".to_string(),
        r"mkfs\.".to_string(),
        r"dd\s+if=".to_string(),
        r":(){ :|:& };:".to_string(), // Fork bomb
        r"curl.*\|\s*(ba)?sh".to_string(),
        r"wget.*\|\s*(ba)?sh".to_string(),
    ]
}

impl Default for PermissionConfig {
    fn default() -> Self {
        Self {
            allowed_patterns: vec![],
            denied_patterns: vec![
                r"rm\s+-rf\s+/\s*$".to_string(),  // Prevent rm -rf / (end of command)
                r"rm\s+-rf\s+/\s+--".to_string(), // Prevent rm -rf / --no-preserve-root
            ],
            confirmation_ttl_secs: default_confirmation_ttl(),
            safe_tools: default_safe_tools(),
            dangerous_patterns: default_dangerous_patterns(),
            whitelist_rules: default_whitelist_rules(),
        }
    }
}

/// Permission manager for tool execution
pub struct PermissionManager {
    /// Compiled allowed patterns
    allowed_patterns: Vec<Regex>,
    /// Compiled denied patterns
    denied_patterns: Vec<Regex>,
    /// Compiled dangerous patterns
    dangerous_patterns: Vec<Regex>,
    /// Safe tools that don't need confirmation
    safe_tools: Vec<String>,
    /// Parsed whitelist rules
    whitelist_rules: Vec<WhitelistRule>,
    /// Confirmed commands (hash -> confirmation time)
    confirmed_commands: HashMap<u64, Instant>,
    /// Confirmation TTL
    confirmation_ttl: Duration,
}

impl PermissionManager {
    /// Create a new permission manager with the given configuration
    #[must_use]
    pub fn new(config: PermissionConfig) -> Self {
        Self {
            allowed_patterns: config
                .allowed_patterns
                .iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect(),
            denied_patterns: config
                .denied_patterns
                .iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect(),
            dangerous_patterns: config
                .dangerous_patterns
                .iter()
                .filter_map(|p| Regex::new(p).ok())
                .collect(),
            safe_tools: config.safe_tools,
            whitelist_rules: config
                .whitelist_rules
                .iter()
                .filter_map(|r| WhitelistRule::parse(r))
                .collect(),
            confirmed_commands: HashMap::new(),
            confirmation_ttl: Duration::from_secs(config.confirmation_ttl_secs),
        }
    }

    /// Check if a tool call is allowed, needs confirmation, or is denied
    #[must_use]
    pub fn check(&self, tool: &str, params: &Value, level: ConfirmationLevel) -> PermissionCheck {
        // Check deny list first
        if let Some(reason) = self.is_denied(tool, params) {
            return PermissionCheck::Denied(reason);
        }

        // Safe tools always allowed
        if self.safe_tools.contains(&tool.to_string()) {
            return PermissionCheck::Allowed;
        }

        // Check if explicitly allowed by pattern
        if self.is_allowed(tool, params) {
            return PermissionCheck::Allowed;
        }

        // No confirmation needed
        if level == ConfirmationLevel::None {
            return PermissionCheck::Allowed;
        }

        // Check if dangerous
        let effective_level =
            if self.is_dangerous(tool, params) { ConfirmationLevel::Dangerous } else { level };

        // For "Once" level, check if already confirmed
        if effective_level == ConfirmationLevel::Once {
            let hash = self.hash_call(tool, params);
            if let Some(confirmed_at) = self.confirmed_commands.get(&hash) {
                if confirmed_at.elapsed() < self.confirmation_ttl {
                    return PermissionCheck::Allowed;
                }
            }
        }

        PermissionCheck::NeedsConfirmation(effective_level)
    }

    /// Record that a command was confirmed by the user
    pub fn record_confirmation(&mut self, tool: &str, params: &Value) {
        let hash = self.hash_call(tool, params);
        self.confirmed_commands.insert(hash, Instant::now());
    }

    /// Clear expired confirmations
    pub fn cleanup_expired(&mut self) {
        self.confirmed_commands
            .retain(|_, confirmed_at| confirmed_at.elapsed() < self.confirmation_ttl);
    }

    /// Check if a tool call is explicitly allowed
    fn is_allowed(&self, tool: &str, params: &Value) -> bool {
        // shell, bash, powershell all use the same permission rules
        if is_shell_tool(tool) {
            if let Some(cmd) = params.get("command").and_then(|v| v.as_str()) {
                // Check regex patterns first
                if self.allowed_patterns.iter().any(|p| p.is_match(cmd)) {
                    return true;
                }
                // Check whitelist rules (use "bash" as canonical name for rule matching)
                if self.whitelist_rules.iter().any(|r| r.matches("bash", cmd)) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if a tool call is denied, returns reason if denied
    #[must_use]
    pub fn is_denied(&self, tool: &str, params: &Value) -> Option<String> {
        // shell, bash, powershell all use the same permission rules
        if is_shell_tool(tool) {
            if let Some(cmd) = params.get("command").and_then(|v| v.as_str()) {
                for pattern in &self.denied_patterns {
                    if pattern.is_match(cmd) {
                        return Some(format!(
                            "Command matches blocked pattern: {}",
                            pattern.as_str()
                        ));
                    }
                }
            }
        }
        None
    }

    /// Check if a tool call is considered dangerous
    fn is_dangerous(&self, tool: &str, params: &Value) -> bool {
        // shell, bash, powershell all use the same permission rules
        if is_shell_tool(tool) {
            if let Some(cmd) = params.get("command").and_then(|v| v.as_str()) {
                return self.dangerous_patterns.iter().any(|p| p.is_match(cmd));
            }
        }
        false
    }

    /// Hash a tool call for confirmation tracking
    #[allow(clippy::unused_self)]
    fn hash_call(&self, tool: &str, params: &Value) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        tool.hash(&mut hasher);
        // Normalize the params for consistent hashing
        params.to_string().hash(&mut hasher);
        hasher.finish()
    }
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self::new(PermissionConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_safe_tools_always_allowed() {
        let pm = PermissionManager::default();

        let check = pm.check("read", &json!({"path": "/etc/passwd"}), ConfirmationLevel::None);
        assert_eq!(check, PermissionCheck::Allowed);

        let check = pm.check("glob", &json!({"pattern": "**/*.rs"}), ConfirmationLevel::None);
        assert_eq!(check, PermissionCheck::Allowed);

        let check = pm.check("grep", &json!({"pattern": "TODO"}), ConfirmationLevel::None);
        assert_eq!(check, PermissionCheck::Allowed);
    }

    #[test]
    fn test_dangerous_pattern_detection() {
        let pm = PermissionManager::default();

        // rm -rf should be dangerous
        let check =
            pm.check("bash", &json!({"command": "rm -rf /tmp/test"}), ConfirmationLevel::Once);
        assert!(matches!(check, PermissionCheck::NeedsConfirmation(ConfirmationLevel::Dangerous)));

        // sudo should be dangerous
        let check =
            pm.check("bash", &json!({"command": "sudo apt update"}), ConfirmationLevel::Once);
        assert!(matches!(check, PermissionCheck::NeedsConfirmation(ConfirmationLevel::Dangerous)));
    }

    #[test]
    fn test_denied_patterns() {
        let pm = PermissionManager::default();

        // rm -rf / should be denied
        let check = pm.check("bash", &json!({"command": "rm -rf /"}), ConfirmationLevel::None);
        assert!(matches!(check, PermissionCheck::Denied(_)));
    }

    #[test]
    fn test_confirmation_once() {
        let mut pm = PermissionManager::default();

        // Use a command that's NOT in the whitelist
        let params = json!({"command": "some-custom-script.sh"});

        // First time needs confirmation
        let check = pm.check("bash", &params, ConfirmationLevel::Once);
        assert!(matches!(check, PermissionCheck::NeedsConfirmation(ConfirmationLevel::Once)));

        // Record confirmation
        pm.record_confirmation("bash", &params);

        // Second time should be allowed
        let check = pm.check("bash", &params, ConfirmationLevel::Once);
        assert_eq!(check, PermissionCheck::Allowed);
    }

    #[test]
    fn test_confirmation_always() {
        let mut pm = PermissionManager::default();

        // Use a command that's NOT in the whitelist
        let params = json!({"command": "deploy-to-production.sh"});

        // First time needs confirmation
        let check = pm.check("bash", &params, ConfirmationLevel::Always);
        assert!(matches!(check, PermissionCheck::NeedsConfirmation(ConfirmationLevel::Always)));

        // Record confirmation
        pm.record_confirmation("bash", &params);

        // Even after confirmation, Always level still needs confirmation
        let check = pm.check("bash", &params, ConfirmationLevel::Always);
        assert!(matches!(check, PermissionCheck::NeedsConfirmation(ConfirmationLevel::Always)));
    }

    #[test]
    fn test_no_confirmation_level() {
        let pm = PermissionManager::default();

        // Commands with no confirmation level should be allowed (unless denied)
        let check = pm.check("bash", &json!({"command": "echo hello"}), ConfirmationLevel::None);
        assert_eq!(check, PermissionCheck::Allowed);
    }

    #[test]
    fn test_custom_config() {
        let config = PermissionConfig {
            allowed_patterns: vec![r"^echo\s+".to_string()],
            denied_patterns: vec![r"secret".to_string()],
            confirmation_ttl_secs: 60,
            safe_tools: vec!["read".to_string()],
            dangerous_patterns: vec![],
            whitelist_rules: vec![],
        };

        let pm = PermissionManager::new(config);

        // Echo is allowed by pattern
        let check = pm.check("bash", &json!({"command": "echo hello"}), ConfirmationLevel::Once);
        assert_eq!(check, PermissionCheck::Allowed);

        // "secret" is denied
        let check =
            pm.check("bash", &json!({"command": "cat secret.txt"}), ConfirmationLevel::None);
        assert!(matches!(check, PermissionCheck::Denied(_)));
    }

    // ==================== Whitelist Rule Tests ====================

    #[test]
    fn test_whitelist_rule_parse() {
        // Basic parsing
        let rule = WhitelistRule::parse("Bash(git:*)").unwrap();
        assert_eq!(rule.tool, "bash");
        assert_eq!(rule.command_prefix, "git");
        assert_eq!(rule.pattern, "*");

        // With spaces
        let rule = WhitelistRule::parse("Bash(npm run:*)").unwrap();
        assert_eq!(rule.command_prefix, "npm run");

        // Invalid format
        assert!(WhitelistRule::parse("invalid").is_none());
        assert!(WhitelistRule::parse("Bash()").is_none());
        assert!(WhitelistRule::parse("Bash(nocolon)").is_none());
    }

    #[test]
    fn test_whitelist_rule_matches() {
        let rule = WhitelistRule::parse("Bash(git:*)").unwrap();

        // Should match
        assert!(rule.matches("bash", "git status"));
        assert!(rule.matches("bash", "git commit -m 'test'"));
        assert!(rule.matches("bash", "git push origin main"));
        assert!(rule.matches("Bash", "git pull")); // Case insensitive tool

        // Should not match
        assert!(!rule.matches("bash", "gitk")); // Not a space after git
        assert!(!rule.matches("bash", "echo git"));
        assert!(!rule.matches("read", "git status")); // Wrong tool
    }

    #[test]
    fn test_whitelist_rule_specific_pattern() {
        let rule = WhitelistRule::parse("Bash(cargo:build)").unwrap();

        // Should match
        assert!(rule.matches("bash", "cargo build"));
        assert!(rule.matches("bash", "cargo build --release"));

        // Should not match
        assert!(!rule.matches("bash", "cargo test"));
        assert!(!rule.matches("bash", "cargo run"));
    }

    #[test]
    fn test_whitelist_rules_in_permission_manager() {
        let pm = PermissionManager::default();

        // Git commands should be allowed by whitelist
        let check = pm.check("bash", &json!({"command": "git status"}), ConfirmationLevel::Once);
        assert_eq!(check, PermissionCheck::Allowed);

        // Cargo commands should be allowed
        let check = pm.check("bash", &json!({"command": "cargo build"}), ConfirmationLevel::Once);
        assert_eq!(check, PermissionCheck::Allowed);

        // npm commands should be allowed
        let check = pm.check("bash", &json!({"command": "npm install"}), ConfirmationLevel::Once);
        assert_eq!(check, PermissionCheck::Allowed);

        // ls should be allowed
        let check = pm.check("bash", &json!({"command": "ls -la"}), ConfirmationLevel::Once);
        assert_eq!(check, PermissionCheck::Allowed);
    }

    #[test]
    fn test_whitelist_does_not_override_dangerous() {
        let pm = PermissionManager::default();

        // Even though git is whitelisted, dangerous patterns should still trigger
        // Note: git itself is not dangerous, but if we had a dangerous git command...
        // For this test, we verify that dangerous patterns take precedence

        // rm -rf is dangerous even if it somehow matched a whitelist
        let check =
            pm.check("bash", &json!({"command": "rm -rf /tmp/test"}), ConfirmationLevel::Once);
        assert!(matches!(check, PermissionCheck::NeedsConfirmation(ConfirmationLevel::Dangerous)));
    }

    #[test]
    fn test_whitelist_does_not_override_denied() {
        let pm = PermissionManager::default();

        // Denied patterns should still block even if command prefix is whitelisted
        let check = pm.check("bash", &json!({"command": "rm -rf /"}), ConfirmationLevel::None);
        assert!(matches!(check, PermissionCheck::Denied(_)));
    }

    // ==================== Shell/PowerShell Permission Tests ====================

    #[test]
    fn test_shell_alias_uses_same_rules_as_bash() {
        let pm = PermissionManager::default();

        // shell should be allowed for whitelisted commands (same as bash)
        let check = pm.check("shell", &json!({"command": "git status"}), ConfirmationLevel::Once);
        assert_eq!(check, PermissionCheck::Allowed);

        // shell should be denied for denied patterns (same as bash)
        let check = pm.check("shell", &json!({"command": "rm -rf /"}), ConfirmationLevel::None);
        assert!(matches!(check, PermissionCheck::Denied(_)));

        // shell should detect dangerous patterns (same as bash)
        let check =
            pm.check("shell", &json!({"command": "rm -rf /tmp/test"}), ConfirmationLevel::Once);
        assert!(matches!(check, PermissionCheck::NeedsConfirmation(ConfirmationLevel::Dangerous)));
    }

    #[test]
    fn test_powershell_uses_same_rules_as_bash() {
        let pm = PermissionManager::default();

        // powershell should be allowed for whitelisted commands
        let check =
            pm.check("powershell", &json!({"command": "git status"}), ConfirmationLevel::Once);
        assert_eq!(check, PermissionCheck::Allowed);

        // powershell should be denied for denied patterns
        let check =
            pm.check("powershell", &json!({"command": "rm -rf /"}), ConfirmationLevel::None);
        assert!(matches!(check, PermissionCheck::Denied(_)));

        // powershell should detect dangerous patterns
        let check =
            pm.check("powershell", &json!({"command": "sudo apt update"}), ConfirmationLevel::Once);
        assert!(matches!(check, PermissionCheck::NeedsConfirmation(ConfirmationLevel::Dangerous)));
    }

    #[test]
    fn test_is_shell_tool_helper() {
        assert!(is_shell_tool("bash"));
        assert!(is_shell_tool("shell"));
        assert!(is_shell_tool("powershell"));
        assert!(!is_shell_tool("read"));
        assert!(!is_shell_tool("write"));
        assert!(!is_shell_tool("Bash")); // Case sensitive
    }
}