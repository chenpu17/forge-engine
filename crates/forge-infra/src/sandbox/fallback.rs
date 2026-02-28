//! Fallback sandbox implementation for non-Unix platforms

use std::io;
use tokio::process::Command;

use super::{SandboxApplied, SandboxConfig};

/// Apply sandbox restrictions (fallback: env cleanup only)
pub fn apply_sandbox(cmd: &mut Command, config: &SandboxConfig) -> io::Result<SandboxApplied> {
    if !config.enabled {
        return Ok(SandboxApplied { removed_env_vars: vec![], resource_limits: vec![] });
    }

    let mut removed_env_vars = Vec::new();

    for var in &config.env_denylist {
        if std::env::var(var).is_ok() {
            cmd.env_remove(var);
            removed_env_vars.push(var.clone());
        }
    }

    tracing::debug!(
        removed_env = ?removed_env_vars,
        "Sandbox applied (fallback: env cleanup only)"
    );

    Ok(SandboxApplied { removed_env_vars, resource_limits: vec![] })
}
