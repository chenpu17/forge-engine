//! MCP (Model Context Protocol) server management extension.
//!
//! Provides helpers for loading, connecting, and querying MCP servers
//! from SDK configuration.

use std::path::{Path, PathBuf};

/// Collect MCP config file paths in priority order (highest first).
///
/// If `explicit_path` is set, only that path is returned.
/// Otherwise: `.forge/mcp.toml` (project) then `~/.forge/mcp.toml` (user).
pub fn collect_mcp_config_paths(
    explicit_path: Option<PathBuf>,
    working_dir: &Path,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(path) = explicit_path {
        if path.exists() {
            paths.push(path);
        } else {
            tracing::warn!(
                path = %path.display(),
                "MCP config path explicitly set but does not exist"
            );
        }
        return paths;
    }

    // 1. Project-level (highest priority)
    let project_path = working_dir.join(".forge/mcp.toml");
    if project_path.exists() {
        paths.push(project_path);
    }

    // 2. User-level
    let user_path = forge_infra::data_dir().join("mcp.toml");
    if user_path.exists() {
        paths.push(user_path);
    }

    paths
}
