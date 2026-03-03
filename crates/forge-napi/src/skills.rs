//! Skill-related NAPI bindings

use napi_derive::napi;

/// Skill info for listing skills in JS/TS.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSkillInfo {
    /// Skill name (e.g., "commit").
    pub name: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Skill description.
    pub description: String,
    /// Argument hint (e.g., "[message]").
    pub argument_hint: Option<String>,
}

impl From<forge_agent::SkillInfo> for JsSkillInfo {
    fn from(info: forge_agent::SkillInfo) -> Self {
        Self {
            name: info.name,
            display_name: info.display_name,
            description: info.description,
            argument_hint: info.argument_hint,
        }
    }
}

/// Skill search path entry.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSkillPath {
    /// Directory path.
    pub path: String,
    /// Path source ("builtin", "user", "project").
    pub source: String,
}

/// Full skill definition for viewing details in JS/TS.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSkillFull {
    /// Skill name.
    pub name: String,
    /// Skill description.
    pub description: String,
    /// Skill prompt content (if loaded).
    pub prompt: Option<String>,
    /// File path.
    pub path: String,
    /// Source: "builtin", "user", or "project".
    pub source: String,
    /// Allowed tools (None = all tools allowed).
    pub allowed_tools: Option<Vec<String>>,
    /// Whether this skill can be invoked by users.
    pub user_invocable: bool,
}

impl From<forge_agent::skill::SkillDefinition> for JsSkillFull {
    fn from(skill: forge_agent::skill::SkillDefinition) -> Self {
        let source = match skill.source {
            forge_agent::skill::SkillSource::Builtin => "builtin",
            forge_agent::skill::SkillSource::User => "user",
            forge_agent::skill::SkillSource::Project => "project",
        };
        Self {
            name: skill.name.clone(),
            description: skill.metadata.description.clone().unwrap_or_default(),
            prompt: skill.prompt().map(str::to_string),
            path: skill.path.to_string_lossy().to_string(),
            source: source.to_string(),
            allowed_tools: skill.metadata.allowed_tools.clone(),
            user_invocable: skill.user_invocable,
        }
    }
}
