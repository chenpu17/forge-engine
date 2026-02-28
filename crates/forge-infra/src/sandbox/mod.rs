//! Process sandbox for shell command execution
//!
//! Provides resource limiting and environment isolation for child processes.
//! Platform-specific implementations use `ulimit` (Unix) for resource caps
//! and environment variable cleanup for security.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[cfg(unix)]
mod unix;

#[cfg(unix)]
pub use unix::{apply_sandbox, sandbox_wrap_command};

#[cfg(not(unix))]
mod fallback;

#[cfg(not(unix))]
pub use fallback::apply_sandbox;

/// Fallback wrap command for non-Unix (no-op)
#[cfg(not(unix))]
#[must_use]
pub fn sandbox_wrap_command(command: &str, _config: &SandboxConfig) -> String {
    command.to_string()
}

/// Sandbox configuration for child processes
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxConfig {
    /// Whether sandboxing is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Paths the process is allowed to access (project root + extras)
    #[serde(default)]
    pub allowed_paths: Vec<PathBuf>,

    /// Whether network access is allowed (default: true)
    #[serde(default = "default_true")]
    pub allow_network: bool,

    /// CPU time limit in seconds (default: 300 = 5 minutes)
    #[serde(default = "default_cpu_limit")]
    pub max_cpu_secs: u64,

    /// Virtual memory limit in bytes (default: 2GB)
    #[serde(default = "default_memory_limit")]
    pub max_memory_bytes: u64,

    /// Maximum number of open file descriptors (default: 1024)
    #[serde(default = "default_fd_limit")]
    pub max_file_descriptors: u64,

    /// Maximum file size in bytes (default: 100MB)
    #[serde(default = "default_file_size_limit")]
    pub max_file_size_bytes: u64,

    /// Maximum number of child processes (default: 64)
    #[serde(default = "default_nproc_limit")]
    pub max_processes: u64,

    /// Environment variables to remove before execution
    #[serde(default = "default_env_denylist")]
    pub env_denylist: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_cpu_limit() -> u64 {
    300 // 5 minutes
}

fn default_memory_limit() -> u64 {
    2 * 1024 * 1024 * 1024 // 2 GB
}

fn default_fd_limit() -> u64 {
    1024
}

fn default_file_size_limit() -> u64 {
    100 * 1024 * 1024 // 100 MB
}

fn default_nproc_limit() -> u64 {
    64
}

fn default_env_denylist() -> Vec<String> {
    vec![
        // LLM API keys
        "ANTHROPIC_API_KEY".to_string(),
        "OPENAI_API_KEY".to_string(),
        "GOOGLE_API_KEY".to_string(),
        "GEMINI_API_KEY".to_string(),
        "FORGE_LLM_API_KEY".to_string(),
        // Cloud provider credentials
        "AWS_ACCESS_KEY_ID".to_string(),
        "AWS_SECRET_ACCESS_KEY".to_string(),
        "AWS_SESSION_TOKEN".to_string(),
        "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
        "AZURE_CLIENT_SECRET".to_string(),
        "AZURE_TENANT_ID".to_string(),
        // VCS and CI tokens
        "GITHUB_TOKEN".to_string(),
        "GH_TOKEN".to_string(),
        "GITLAB_TOKEN".to_string(),
        // Package registry tokens
        "NPM_TOKEN".to_string(),
        "PYPI_TOKEN".to_string(),
        "CARGO_REGISTRY_TOKEN".to_string(),
        "DOCKER_PASSWORD".to_string(),
        // AI/ML tokens
        "HF_TOKEN".to_string(),
        "HUGGING_FACE_HUB_TOKEN".to_string(),
        // Communication tokens
        "SLACK_TOKEN".to_string(),
        "SLACK_WEBHOOK".to_string(),
        // Database credentials
        "DATABASE_PASSWORD".to_string(),
        "DB_PASSWORD".to_string(),
        "REDIS_PASSWORD".to_string(),
        "MONGO_PASSWORD".to_string(),
        // Generic secrets
        "SECRET_KEY".to_string(),
        "PRIVATE_KEY".to_string(),
        "ENCRYPTION_KEY".to_string(),
        // SSH agent (prevents key forwarding)
        "SSH_AUTH_SOCK".to_string(),
    ]
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_paths: vec![],
            allow_network: true,
            max_cpu_secs: default_cpu_limit(),
            max_memory_bytes: default_memory_limit(),
            max_file_descriptors: default_fd_limit(),
            max_file_size_bytes: default_file_size_limit(),
            max_processes: default_nproc_limit(),
            env_denylist: default_env_denylist(),
        }
    }
}

impl SandboxConfig {
    /// Create a permissive config (no resource limits, no env cleanup)
    #[must_use]
    pub fn permissive() -> Self {
        Self { enabled: false, ..Default::default() }
    }

    /// Create a strict config with tighter limits
    #[must_use]
    pub fn strict() -> Self {
        Self {
            enabled: true,
            allow_network: false,
            max_cpu_secs: 60,
            max_memory_bytes: 512 * 1024 * 1024, // 512 MB
            max_file_descriptors: 256,
            max_file_size_bytes: 10 * 1024 * 1024, // 10 MB
            max_processes: 16,
            ..Default::default()
        }
    }
}

/// Result of applying sandbox to a command
#[derive(Debug)]
pub struct SandboxApplied {
    /// Environment variables that were removed
    pub removed_env_vars: Vec<String>,
    /// Resource limits that were set
    pub resource_limits: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SandboxConfig::default();
        assert!(config.enabled);
        assert!(config.allow_network);
        assert_eq!(config.max_cpu_secs, 300);
        assert_eq!(config.max_memory_bytes, 2 * 1024 * 1024 * 1024);
        assert!(!config.env_denylist.is_empty());
    }

    #[test]
    fn test_permissive_config() {
        let config = SandboxConfig::permissive();
        assert!(!config.enabled);
    }

    #[test]
    fn test_strict_config() {
        let config = SandboxConfig::strict();
        assert!(config.enabled);
        assert!(!config.allow_network);
        assert_eq!(config.max_cpu_secs, 60);
        assert_eq!(config.max_memory_bytes, 512 * 1024 * 1024);
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = SandboxConfig::default();
        let json = serde_json::to_string(&config).expect("serialize");
        let parsed: SandboxConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.max_cpu_secs, config.max_cpu_secs);
        assert_eq!(parsed.max_memory_bytes, config.max_memory_bytes);
        assert_eq!(parsed.enabled, config.enabled);
    }
}
