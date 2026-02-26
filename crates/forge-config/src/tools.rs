//! Tool-related configuration (general-purpose parts).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::memory::MemorySettings;
use crate::proxy::ProxyConfig;

const fn default_command_timeout() -> u64 {
    120
}

fn default_dangerous_commands() -> Vec<String> {
    vec![
        "rm -rf".to_string(),
        "sudo".to_string(),
        "mkfs".to_string(),
    ]
}

const fn default_true() -> bool {
    true
}

/// Tool-related configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    /// Default command timeout (seconds).
    #[serde(default = "default_command_timeout")]
    pub command_timeout: u64,
    /// Commands that always require confirmation.
    #[serde(default = "default_dangerous_commands")]
    pub dangerous_commands: Vec<String>,
    /// MCP settings (flattened to keep `mcp_enabled`/`mcp_config_path` at `[tools]` root).
    #[serde(default, flatten)]
    pub mcp: McpSettings,
    /// Global proxy configuration for tools.
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// Trust level configuration for tool execution.
    #[serde(default)]
    pub trust: TrustLevelConfig,
    /// Environment exposure policy for tools.
    #[serde(default)]
    pub env_policy: EnvPolicy,
    /// Memory system settings.
    #[serde(default)]
    pub memory: MemorySettings,
    /// Permission rules for fine-grained file access control.
    ///
    /// Rules are evaluated in first-match-wins order, before trust level checks.
    #[serde(default)]
    pub permission_rules: Vec<PermissionRuleConfig>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            command_timeout: default_command_timeout(),
            dangerous_commands: default_dangerous_commands(),
            mcp: McpSettings::default(),
            proxy: ProxyConfig::default(),
            trust: TrustLevelConfig::default(),
            env_policy: EnvPolicy::default(),
            memory: MemorySettings::default(),
            permission_rules: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// MCP Settings
// ---------------------------------------------------------------------------

/// MCP-related configuration shared across crates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpSettings {
    /// Enable MCP servers.
    #[serde(default = "default_true")]
    pub mcp_enabled: bool,
    /// MCP configuration file path.
    pub mcp_config_path: Option<PathBuf>,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            mcp_enabled: true,
            mcp_config_path: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Trust Level
// ---------------------------------------------------------------------------

/// Trust level setting for tool execution.
///
/// Determines how much confirmation is required for tool operations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevelSetting {
    /// Most restrictive — all write operations need confirmation.
    #[default]
    Cautious,
    /// Project-internal operations auto-allowed.
    Development,
    /// Only dangerous commands need confirmation.
    Trusted,
    /// No confirmation needed (except hardcoded safety blocks).
    Yolo,
}

/// Trust level configuration for tool execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustLevelConfig {
    /// Default trust level.
    #[serde(default)]
    pub level: TrustLevelSetting,
    /// Per-project trust level overrides (project path → trust level).
    #[serde(default)]
    pub project_overrides: HashMap<String, TrustLevelSetting>,
}

impl TrustLevelConfig {
    /// Get effective trust level for a project path.
    ///
    /// Returns the project-specific override if set, otherwise the default level.
    #[must_use]
    pub fn effective_level(&self, project_path: Option<&str>) -> TrustLevelSetting {
        if let Some(path) = project_path {
            if let Some(override_level) = self.project_overrides.get(path) {
                return *override_level;
            }
        }
        self.level
    }
}

// ---------------------------------------------------------------------------
// Permission Rules
// ---------------------------------------------------------------------------

/// Action for a permission rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyAction {
    /// Allow the operation.
    Allow,
    /// Deny the operation.
    Deny,
}

/// Operation type that a permission rule applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationType {
    /// File read operations.
    Read,
    /// File write/edit operations.
    Write,
    /// Shell command execution.
    Execute,
}

/// A permission rule for fine-grained file access control.
///
/// Rules use glob patterns and are evaluated in first-match-wins order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRuleConfig {
    /// Glob pattern to match against file paths (e.g., `*.env`, `.env*`).
    pub pattern: String,
    /// Action to take when matched.
    pub action: PolicyAction,
    /// Operations this rule applies to (empty = all operations).
    #[serde(default)]
    pub operations: Vec<OperationType>,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// Environment Policy
// ---------------------------------------------------------------------------

/// Environment variable exposure policy mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvPolicyMode {
    /// Expose all environment variables.
    #[default]
    All,
    /// Expose only allowlisted variables.
    Allowlist,
    /// Expose all except denylisted variables.
    Denylist,
}

/// Environment variable policy (allow/deny list with optional presets).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvPolicy {
    /// Policy mode.
    #[serde(default)]
    pub mode: EnvPolicyMode,
    /// Allowlist (used when mode = allowlist).
    #[serde(default)]
    pub allowlist: Vec<String>,
    /// Denylist (used when mode = denylist).
    #[serde(default)]
    pub denylist: Vec<String>,
    /// Optional preset (e.g. "recommended").
    #[serde(default)]
    pub preset: Option<String>,
}
