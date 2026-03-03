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
        Err(crate::error::ForgeError::ConfigError(format!("Skill '{name}' not found")))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_slash_command_basic() {
        let (name, args) = parse_slash_command("/commit -m 'fix bug'").unwrap();
        assert_eq!(name, "commit");
        assert_eq!(args, "-m 'fix bug'");
    }

    #[test]
    fn test_parse_slash_command_no_args() {
        let (name, args) = parse_slash_command("/help").unwrap();
        assert_eq!(name, "help");
        assert_eq!(args, "");
    }

    #[test]
    fn test_parse_slash_command_not_a_command() {
        assert!(parse_slash_command("hello world").is_none());
    }

    #[test]
    fn test_parse_slash_command_empty_slash() {
        assert!(parse_slash_command("/").is_none());
    }

    #[test]
    fn test_parse_slash_command_whitespace_trimmed() {
        let (name, args) = parse_slash_command("  /review  some code  ").unwrap();
        assert_eq!(name, "review");
        assert_eq!(args, "some code");
    }

    #[test]
    fn test_skill_registry_builder() {
        let registry = SkillRegistryBuilder::new()
            .builtin_path(PathBuf::from("/builtin"))
            .user_path(PathBuf::from("/user"))
            .project_path(PathBuf::from("/project"))
            .build()
            .unwrap();

        let paths = registry.get_skill_paths();
        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0].1, SkillSource::Builtin);
        assert_eq!(paths[1].1, SkillSource::User);
        assert_eq!(paths[2].1, SkillSource::Project);
    }

    #[test]
    fn test_skill_registry_stub_methods() {
        let registry = SkillRegistryBuilder::new().build().unwrap();
        assert!(registry.expand_user_invocable("test", "").is_none());
        assert!(registry.list_model_invocable().is_empty());
        assert!(registry.list_all().is_empty());
        assert_eq!(registry.reload().unwrap(), 0);
    }

    #[test]
    fn test_skill_registry_get_full_not_found() {
        let registry = SkillRegistryBuilder::new().build().unwrap();
        let err = registry.get_full("nonexistent").unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn test_parse_slash_command_double_slash() {
        // "//double" — the name should be "/double" (everything after the first slash)
        let result = parse_slash_command("//double");
        assert!(result.is_some());
        let (name, args) = result.unwrap();
        assert_eq!(name, "/double");
        assert_eq!(args, "");
    }

    #[test]
    fn test_parse_slash_command_space_after_slash() {
        // "/ " — slash followed by whitespace only, name would be empty
        assert!(parse_slash_command("/ ").is_none());
    }

    #[test]
    fn test_parse_slash_command_unicode_name() {
        let result = parse_slash_command("/提交 some args");
        assert!(result.is_some());
        let (name, args) = result.unwrap();
        assert_eq!(name, "提交");
        assert_eq!(args, "some args");
    }

    #[test]
    fn test_parse_slash_command_empty_string() {
        assert!(parse_slash_command("").is_none());
    }

    #[test]
    fn test_skill_search_paths_no_trust() {
        let paths = skill_search_paths(Path::new("/tmp/fake"), false);
        // Project skills should NOT be included when trust is false
        for (_, source) in &paths {
            assert_ne!(*source, SkillSource::Project);
        }
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
