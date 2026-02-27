//! Plugin system for external script-based tools
//!
//! Scans `.forge/tools/` directories for plugin manifests (`tool.json`)
//! and registers them as `ScriptTool` instances.
//!
//! # Plugin Directory Structure
//!
//! ```text
//! .forge/tools/
//! ├── my_tool/
//! │   ├── tool.json      # Manifest (name, description, parameters, etc.)
//! │   └── tool.sh        # Executable script (or .py, .js, .rb, etc.)
//! └── another_tool/
//!     ├── tool.json
//!     └── tool.py
//! ```

pub mod script;

pub use script::{PluginManifest, ScriptTool};

use crate::Tool;
use std::path::Path;
use std::sync::Arc;

/// Known script file names to look for (in priority order)
const SCRIPT_NAMES: &[&str] =
    &["tool.sh", "tool.py", "tool.js", "tool.ts", "tool.rb", "tool.bash", "tool.zsh"];

/// Load all plugins from a directory
///
/// Scans the given directory for subdirectories containing `tool.json` manifests.
/// Returns a list of `ScriptTool` instances ready to be registered.
///
/// Invalid plugins are logged and skipped (non-fatal).
pub fn load_plugins(tools_dir: &Path) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();

    if !tools_dir.is_dir() {
        tracing::debug!(path = %tools_dir.display(), "Plugin directory does not exist, skipping");
        return tools;
    }

    let entries = match std::fs::read_dir(tools_dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(path = %tools_dir.display(), error = %e, "Failed to read plugin directory");
            return tools;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        match load_single_plugin(&path) {
            Ok(tool) => {
                tracing::info!(
                    name = %tool.manifest.name,
                    script = %tool.script_path.display(),
                    "Loaded plugin"
                );
                tools.push(Arc::new(tool));
            }
            Err(e) => {
                tracing::warn!(
                    dir = %path.display(),
                    error = %e,
                    "Skipping invalid plugin"
                );
            }
        }
    }

    tracing::info!(count = tools.len(), dir = %tools_dir.display(), "Plugins loaded");
    tools
}

/// Load all plugins from both project-level and user-level directories
///
/// Searches:
/// 1. `{working_dir}/.forge/tools/` (project-level)
/// 2. `~/.forge/tools/` (user-level)
///
/// Project-level plugins take precedence (registered first).
pub fn load_all_plugins(working_dir: &Path) -> Vec<Arc<dyn Tool>> {
    let mut tools = Vec::new();

    // Project-level plugins
    let project_dir = working_dir.join(".forge").join("tools");
    tools.extend(load_plugins(&project_dir));

    // User-level plugins
    let user_dir = forge_infra::data_dir().join("tools");
    if user_dir != project_dir {
        // Avoid loading duplicates if project dir IS the user dir
        let user_tools = load_plugins(&user_dir);
        // Skip user plugins whose names conflict with project plugins
        let existing_names: std::collections::HashSet<_> =
            tools.iter().map(|t| t.name().to_string()).collect();
        for tool in user_tools {
            if existing_names.contains(tool.name()) {
                tracing::debug!(
                    name = %tool.name(),
                    "Skipping user-level plugin (overridden by project-level)"
                );
            } else {
                tools.push(tool);
            }
        }
    }

    tools
}

/// Load a single plugin from a directory
fn load_single_plugin(plugin_dir: &Path) -> std::result::Result<ScriptTool, String> {
    let manifest_path = plugin_dir.join("tool.json");
    if !manifest_path.is_file() {
        return Err("No tool.json manifest found".to_string());
    }

    // Parse manifest
    let manifest_content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Failed to read tool.json: {e}"))?;

    let manifest: PluginManifest =
        serde_json::from_str(&manifest_content).map_err(|e| format!("Invalid tool.json: {e}"))?;

    // Validate name (non-empty, alphanumeric + underscores only)
    if manifest.name.is_empty() || !manifest.name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(format!(
            "Invalid tool name '{}': must be alphanumeric + underscores",
            manifest.name
        ));
    }

    // Validate parameters schema — must be a JSON object with "type": "object"
    if let Some(obj) = manifest.parameters.as_object() {
        match obj.get("type").and_then(|t| t.as_str()) {
            Some("object") => {} // valid
            Some(other) => {
                return Err(format!(
                    "Invalid parameters schema: 'type' must be 'object', got '{other}'"
                ));
            }
            None => {
                return Err("Invalid parameters schema: missing 'type' field (expected 'object')"
                    .to_string());
            }
        }
    } else {
        return Err("Invalid parameters schema: must be a JSON object".to_string());
    }

    // Find the script file and verify it's inside the plugin directory
    let script_path = find_script(plugin_dir)?;

    // Security: canonicalize both paths to resolve symlinks, then verify
    // the script is actually inside the plugin directory (prevents symlink escape).
    let canonical_dir = std::fs::canonicalize(plugin_dir)
        .map_err(|e| format!("Cannot canonicalize plugin dir: {e}"))?;
    let canonical_script = std::fs::canonicalize(&script_path)
        .map_err(|e| format!("Cannot canonicalize script path: {e}"))?;
    if !canonical_script.starts_with(&canonical_dir) {
        return Err(format!(
            "Script path escapes plugin directory (possible symlink attack): {}",
            script_path.display()
        ));
    }

    Ok(ScriptTool::new(manifest, script_path, plugin_dir.to_path_buf()))
}

/// Find the executable script in a plugin directory
fn find_script(plugin_dir: &Path) -> std::result::Result<std::path::PathBuf, String> {
    for name in SCRIPT_NAMES {
        let path = plugin_dir.join(name);
        if path.is_file() {
            return Ok(path);
        }
    }

    // Also check for any executable file
    if let Ok(entries) = std::fs::read_dir(plugin_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.file_name().is_some_and(|n| n != "tool.json") {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(meta) = path.metadata() {
                        if meta.permissions().mode() & 0o111 != 0 {
                            return Ok(path);
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    return Ok(path);
                }
            }
        }
    }

    Err(format!(
        "No script found in {}. Expected one of: {}",
        plugin_dir.display(),
        SCRIPT_NAMES.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_plugins_empty_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tools = load_plugins(dir.path());
        assert!(tools.is_empty());
    }

    #[test]
    fn test_load_plugins_nonexistent_dir() {
        let tools = load_plugins(Path::new("/nonexistent/path"));
        assert!(tools.is_empty());
    }

    #[test]
    fn test_load_single_plugin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plugin_dir = dir.path().join("my_tool");
        std::fs::create_dir(&plugin_dir).expect("mkdir");

        std::fs::write(
            plugin_dir.join("tool.json"),
            r#"{"name": "my_tool", "description": "Test tool"}"#,
        )
        .expect("write manifest");

        std::fs::write(plugin_dir.join("tool.sh"), "#!/bin/bash\necho ok\n").expect("write script");

        let tool = load_single_plugin(&plugin_dir).expect("load plugin");
        assert_eq!(tool.manifest.name, "my_tool");
        assert_eq!(tool.manifest.description, "Test tool");
        assert!(tool.script_path.ends_with("tool.sh"));
    }

    #[test]
    fn test_load_single_plugin_no_manifest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = load_single_plugin(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No tool.json"));
    }

    #[test]
    fn test_load_single_plugin_no_script() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("tool.json"),
            r#"{"name": "orphan", "description": "No script"}"#,
        )
        .expect("write manifest");

        let result = load_single_plugin(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No script found"));
    }

    #[test]
    fn test_load_single_plugin_invalid_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("tool.json"),
            r#"{"name": "bad-name!", "description": "Invalid"}"#,
        )
        .expect("write manifest");
        std::fs::write(dir.path().join("tool.sh"), "#!/bin/bash").expect("write script");

        let result = load_single_plugin(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid tool name"));
    }

    #[test]
    fn test_load_single_plugin_invalid_schema_type() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("tool.json"),
            r#"{"name": "bad_schema", "description": "Bad schema", "parameters": {"type": "array"}}"#,
        )
        .expect("write manifest");
        std::fs::write(dir.path().join("tool.sh"), "#!/bin/bash").expect("write script");

        let result = load_single_plugin(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be 'object'"));
    }

    #[test]
    fn test_load_single_plugin_invalid_schema_not_object() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("tool.json"),
            r#"{"name": "bad_schema2", "description": "Bad schema", "parameters": "not an object"}"#,
        )
        .expect("write manifest");
        std::fs::write(dir.path().join("tool.sh"), "#!/bin/bash").expect("write script");

        let result = load_single_plugin(dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be a JSON object"));
    }

    #[test]
    fn test_load_plugins_directory() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Create two valid plugins
        for name in &["tool_a", "tool_b"] {
            let plugin_dir = dir.path().join(name);
            std::fs::create_dir(&plugin_dir).expect("mkdir");
            std::fs::write(
                plugin_dir.join("tool.json"),
                format!(r#"{{"name": "{name}", "description": "Tool {name}"}}"#),
            )
            .expect("write manifest");
            std::fs::write(plugin_dir.join("tool.sh"), "#!/bin/bash\necho ok\n")
                .expect("write script");
        }

        // Create one invalid plugin (no script)
        let bad_dir = dir.path().join("bad_tool");
        std::fs::create_dir(&bad_dir).expect("mkdir");
        std::fs::write(
            bad_dir.join("tool.json"),
            r#"{"name": "bad_tool", "description": "Missing script"}"#,
        )
        .expect("write manifest");

        let tools = load_plugins(dir.path());
        assert_eq!(tools.len(), 2);
    }

    #[test]
    fn test_find_script_priority() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Create both .sh and .py — .sh should win (higher priority)
        std::fs::write(dir.path().join("tool.sh"), "#!/bin/bash").expect("write sh");
        std::fs::write(dir.path().join("tool.py"), "#!/usr/bin/env python3").expect("write py");

        let script = find_script(dir.path()).expect("find script");
        assert!(script.ends_with("tool.sh"));
    }

    #[test]
    fn test_find_script_python() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("tool.py"), "print('hello')").expect("write py");

        let script = find_script(dir.path()).expect("find script");
        assert!(script.ends_with("tool.py"));
    }
}