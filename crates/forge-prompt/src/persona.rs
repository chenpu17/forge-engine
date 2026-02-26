//! Persona configuration types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::error::{PromptError, Result};

const fn default_true() -> bool {
    true
}

/// Persona configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaConfig {
    /// Persona name.
    pub name: String,
    /// Persona description.
    pub description: String,
    /// Persona prompt content (loaded from .md file).
    #[serde(skip)]
    pub prompt: String,
    /// Enabled templates.
    #[serde(default)]
    pub templates: Vec<String>,
    /// Disabled tools.
    #[serde(default)]
    pub disabled_tools: Vec<String>,
    /// Additional options.
    #[serde(default)]
    pub options: PersonaOptions,
}

/// Persona additional options.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaOptions {
    /// Bash read-only mode.
    #[serde(default)]
    pub bash_readonly: bool,
    /// Max iterations override.
    pub max_iterations: Option<usize>,
    /// Whether reflection is enabled.
    #[serde(default = "default_true")]
    pub reflection_enabled: bool,
    /// Override the default repeated-tool-call threshold.
    pub max_same_tool_calls: Option<usize>,
    /// Per-tool repeated-call limits.
    #[serde(default)]
    pub tool_call_limits: HashMap<String, usize>,
}

impl Default for PersonaOptions {
    fn default() -> Self {
        Self {
            bash_readonly: false,
            max_iterations: None,
            reflection_enabled: true,
            max_same_tool_calls: None,
            tool_call_limits: HashMap::new(),
        }
    }
}

impl PersonaConfig {
    /// Create default config with prompt content.
    #[must_use]
    pub fn default_with_prompt(name: String, prompt: String) -> Self {
        Self {
            description: format!("{name} persona"),
            name,
            prompt,
            templates: vec!["tool_usage".to_string()],
            disabled_tools: Vec::new(),
            options: PersonaOptions::default(),
        }
    }

    /// Load from TOML config file.
    ///
    /// # Errors
    /// Returns error if the file cannot be read or parsed.
    pub fn from_file(path: &Path, prompt: String) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| PromptError::Load(format!("{}: {e}", path.display())))?;

        let toml_value: toml::Value = toml::from_str(&content)
            .map_err(|e| PromptError::Parse(format!("{}: {e}", path.display())))?;

        let persona = toml_value.get("persona").unwrap_or(&toml_value);
        let name = persona
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let description = persona
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let templates = toml_value
            .get("templates")
            .and_then(|t| t.get("enabled"))
            .and_then(|v| v.as_array())
            .map_or_else(
                || vec!["tool_usage".to_string()],
                |arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                },
            );

        let disabled_tools = toml_value
            .get("tools")
            .and_then(|t| t.get("disabled"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let options = toml_value
            .get("options")
            .map(|o| PersonaOptions {
                bash_readonly: o
                    .get("bash_readonly")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false),
                max_iterations: o
                    .get("max_iterations")
                    .and_then(toml::Value::as_integer)
                    .map(|i| i as usize),
                reflection_enabled: o
                    .get("reflection_enabled")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(true),
                max_same_tool_calls: o
                    .get("max_same_tool_calls")
                    .and_then(toml::Value::as_integer)
                    .map(|i| i as usize),
                tool_call_limits: o
                    .get("tool_call_limits")
                    .and_then(|v| v.as_table())
                    .map(|t| {
                        t.iter()
                            .filter_map(|(k, v)| {
                                v.as_integer().map(|n| (k.clone(), n as usize))
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .unwrap_or_default();

        Ok(Self {
            name,
            description,
            prompt,
            templates,
            disabled_tools,
            options,
        })
    }
}

/// Minimal skill info for prompt building.
///
/// The full `SkillRegistry` lives in a separate crate; this struct carries
/// just enough data to render skill sections in the system prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillInfo {
    /// Skill name (e.g., "commit").
    pub name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Short description.
    pub description: String,
    /// Argument hint (e.g., "[message]").
    pub argument_hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_persona_default_with_prompt() {
        let p = PersonaConfig::default_with_prompt(
            "test".to_string(),
            "You are a test agent.".to_string(),
        );
        assert_eq!(p.name, "test");
        assert_eq!(p.prompt, "You are a test agent.");
        assert!(p.disabled_tools.is_empty());
        assert!(p.options.reflection_enabled);
    }

    #[test]
    fn test_persona_from_toml_file() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("test.toml");
        std::fs::write(
            &path,
            r#"
[persona]
name = "coder"
description = "AI coder"

[templates]
enabled = ["tool_usage", "git_protocol"]

[tools]
disabled = ["write"]

[options]
bash_readonly = false
reflection_enabled = true
"#,
        )
        .expect("write toml");

        let config =
            PersonaConfig::from_file(&path, "prompt content".to_string())
                .expect("parse");
        assert_eq!(config.name, "coder");
        assert_eq!(config.templates, vec!["tool_usage", "git_protocol"]);
        assert_eq!(config.disabled_tools, vec!["write"]);
        assert!(config.options.reflection_enabled);
    }
}
