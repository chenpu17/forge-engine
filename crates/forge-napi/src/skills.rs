//! Skill-related NAPI bindings

use napi_derive::napi;

/// Skill info for listing skills in JS/TS
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSkillInfo {
    pub name: String,
    pub description: String,
    pub source: String,
}

/// Skill search path entry
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSkillPath {
    pub path: String,
    pub source: String,
}
