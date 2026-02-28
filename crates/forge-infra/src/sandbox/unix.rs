//! Unix sandbox implementation using bash ulimit and environment cleanup

use std::io;
use tokio::process::Command;

use super::{SandboxApplied, SandboxConfig};

/// Apply sandbox restrictions to a tokio Command
pub fn apply_sandbox(cmd: &mut Command, config: &SandboxConfig) -> io::Result<SandboxApplied> {
    if !config.enabled {
        return Ok(SandboxApplied { removed_env_vars: vec![], resource_limits: vec![] });
    }

    let mut removed_env_vars = Vec::new();

    for var in &config.env_denylist {
        cmd.env_remove(var);
        if std::env::var(var).is_ok() {
            removed_env_vars.push(var.clone());
        }
    }

    tracing::debug!(
        removed_env = ?removed_env_vars,
        "Sandbox env cleanup applied"
    );

    Ok(SandboxApplied { removed_env_vars, resource_limits: vec![] })
}

/// Wrap a shell command with ulimit resource limits
#[must_use]
pub fn sandbox_wrap_command(command: &str, config: &SandboxConfig) -> String {
    if !config.enabled {
        return command.to_string();
    }

    let mut limits = Vec::new();

    // CPU time limit (seconds)
    limits.push(format!("ulimit -t {} 2>/dev/null", config.max_cpu_secs));

    // Virtual memory limit (ulimit -v uses KB)
    let mem_kb = config.max_memory_bytes / 1024;
    limits.push(format!("ulimit -v {mem_kb} 2>/dev/null"));

    // File descriptor limit
    limits.push(format!("ulimit -n {} 2>/dev/null", config.max_file_descriptors));

    // File size limit (ulimit -f uses 512-byte blocks)
    let fsize_blocks = config.max_file_size_bytes / 512;
    limits.push(format!("ulimit -f {fsize_blocks} 2>/dev/null"));

    // Max child processes
    limits.push(format!("ulimit -u {} 2>/dev/null", config.max_processes));

    limits.push(command.to_string());
    limits.join("; ")
}
