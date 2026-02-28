//! Skill registration and loading extension.
//!
//! Manages skill discovery from user-level and project-level directories,
//! providing the skill registry to the agent loop.

use std::path::{Path, PathBuf};

use forge_agent::skill::SkillDefinition;
use forge_prompt::SkillInfo;

/// Build skill search paths for the registry.
///
/// Returns `(path, source)` pairs in discovery order:
/// 1. User-level: `~/.forge/skills/`
/// 2. Project-level: `<working_dir>/.forge/skills/` (if trusted)
pub fn skill_search_paths(
    working_dir: &Path,
    trust_project_skills: bool,
) -> Vec<(PathBuf, SkillSource)> {
    let mut paths = Vec::new();

    // User-level skills (always loaded)
    let user_dir = forge_infra::data_dir().join("skills");
    if user_dir.is_dir() {
        paths.push((user_dir, SkillSource::User));
    }

    // Project-level skills (only if trusted)
    if trust_project_skills {
        let project_dir = working_dir.join(".forge/skills");
        if project_dir.is_dir() {
            paths.push((project_dir, SkillSource::Project));
        }
    }

    paths
}

/// Source of a skill definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    /// Built-in skill (from prompts directory)
    Builtin,
    /// User-level skill (`~/.forge/skills/`)
    User,
    /// Project-level skill (`.forge/skills/`)
    Project,
}

/// Skill registry — discovers and manages skill definitions.
///
/// Stub implementation for forge-engine. The full skill system will be
/// migrated in a future iteration.
pub struct SkillRegistry {
    skill_paths: Vec<(PathBuf, SkillSource)>,
}

impl SkillRegistry {
    /// Expand a user-invocable slash command into a prompt.
    #[must_use]
    pub fn expand_user_invocable(&self, _name: &str, _args: &str) -> Option<String> {
        None
    }

    /// List skills that can be invoked by the model.
    #[must_use]
    pub fn list_model_invocable(&self) -> Vec<SkillInfo> {
        Vec::new()
    }

    /// List all discovered skills.
    #[must_use]
    pub fn list_all(&self) -> Vec<SkillInfo> {
        Vec::new()
    }

    /// Reload skills from disk.
    ///
    /// # Errors
    ///
    /// Returns error if skill loading fails.
    pub fn reload(&self) -> crate::error::Result<usize> {
        Ok(0)
    }

    /// Get a skill with full prompt content loaded.
    ///
    /// # Errors
    ///
    /// Returns error if skill not found.
    pub fn get_full(&self, name: &str) -> crate::error::Result<SkillDefinition> {
        Err(crate::error::ForgeError::ConfigError(format!(
            "Skill '{name}' not found"
        )))
    }

    /// Get configured skill search paths.
    #[must_use]
    pub fn get_skill_paths(&self) -> Vec<(PathBuf, SkillSource)> {
        self.skill_paths.clone()
    }
}

/// Builder for [`SkillRegistry`].
pub struct SkillRegistryBuilder {
    paths: Vec<(PathBuf, SkillSource)>,
}

impl SkillRegistryBuilder {
    /// Create a new builder.
    #[must_use]
    pub fn new() -> Self {
        Self { paths: Vec::new() }
    }

    /// Add a built-in skills directory.
    #[must_use]
    pub fn builtin_path(mut self, dir: PathBuf) -> Self {
        self.paths.push((dir, SkillSource::Builtin));
        self
    }

    /// Add a user-level skills directory.
    #[must_use]
    pub fn user_path(mut self, dir: PathBuf) -> Self {
        self.paths.push((dir, SkillSource::User));
        self
    }

    /// Add a project-level skills directory.
    #[must_use]
    pub fn project_path(mut self, dir: PathBuf) -> Self {
        self.paths.push((dir, SkillSource::Project));
        self
    }

    /// Build the registry.
    ///
    /// # Errors
    ///
    /// Returns error if initialization fails.
    pub fn build(self) -> crate::error::Result<SkillRegistry> {
        Ok(SkillRegistry { skill_paths: self.paths })
    }
}

impl Default for SkillRegistryBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a slash command from user input.
///
/// Returns `(name, args)` if the input starts with `/`.
#[must_use]
pub fn parse_slash_command(input: &str) -> Option<(String, String)> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let without_slash = &trimmed[1..];
    let mut parts = without_slash.splitn(2, char::is_whitespace);
    let name = parts.next()?.to_string();
    if name.is_empty() {
        return None;
    }
    let args = parts.next().unwrap_or("").trim().to_string();
    Some((name, args))
}
