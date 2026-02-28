//! Skill-related NAPI bindings

use napi_derive::napi;

/// Skill info for listing skills in JS/TS.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSkillInfo {
    /// Skill name.
    pub name: String,
    /// Skill description.
    pub description: String,
    /// Skill source ("builtin", "user", "project").
    pub source: String,
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
