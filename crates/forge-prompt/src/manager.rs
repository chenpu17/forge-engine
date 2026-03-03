//! Prompt manager — loads personas, templates, and builds system prompts.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::context::PromptContext;
use crate::error::{PromptError, Result};
use crate::persona::PersonaConfig;

/// Default persona name.
pub const DEFAULT_PERSONA: &str = "coder";

const BUILTIN_PERSONAS: &[&str] = &[DEFAULT_PERSONA];

fn is_builtin_persona(name: &str) -> bool {
    BUILTIN_PERSONAS.contains(&name)
}

/// Prompt Manager.
///
/// Manages loading and building system prompts from external files.
#[derive(Clone)]
pub struct PromptManager {
    /// Available personas.
    personas: HashMap<String, PersonaConfig>,
    /// Current persona name.
    current_persona: String,
    /// Template fragments.
    templates: HashMap<String, String>,
    /// Prompts directory.
    prompts_dir: PathBuf,
    /// Default system prompt.
    default_prompt: String,
}

/// Minimal fallback system prompt.
const FALLBACK_SYSTEM_PROMPT: &str = r"# Forge AI Agent

You are Forge, a general-purpose AI agent. Help users complete tasks.

## Core Guidelines

1. Be concise and direct
2. Use tools to complete tasks
3. Verify your work
4. Handle errors gracefully
";

impl PromptManager {
    /// Create a new `PromptManager` with embedded defaults.
    #[must_use]
    pub fn new() -> Self {
        Self {
            personas: HashMap::new(),
            current_persona: DEFAULT_PERSONA.to_string(),
            templates: HashMap::new(),
            prompts_dir: PathBuf::new(),
            default_prompt: FALLBACK_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Load prompts from a directory.
    ///
    /// # Errors
    /// Returns error if persona/template files cannot be read.
    pub fn from_dir(prompts_dir: impl AsRef<Path>) -> Result<Self> {
        let prompts_dir = prompts_dir.as_ref();
        if !prompts_dir.exists() {
            tracing::warn!("Prompts directory not found: {prompts_dir:?}, using defaults");
            return Ok(Self::new());
        }

        let personas = Self::load_personas(prompts_dir)?;
        let templates = Self::load_templates(prompts_dir)?;
        let default_prompt = Self::load_system_prompt(prompts_dir);

        let current_persona = if personas.contains_key(DEFAULT_PERSONA) {
            DEFAULT_PERSONA.to_string()
        } else {
            personas.keys().next().cloned().unwrap_or_else(|| DEFAULT_PERSONA.to_string())
        };

        tracing::info!(
            "Loaded {} personas and {} templates from {prompts_dir:?}",
            personas.len(),
            templates.len(),
        );

        Ok(Self {
            personas,
            current_persona,
            templates,
            prompts_dir: prompts_dir.to_path_buf(),
            default_prompt,
        })
    }

    /// Switch to a different persona.
    ///
    /// # Errors
    /// Returns error if the persona is not found.
    pub fn set_persona(&mut self, name: &str) -> Result<()> {
        if !self.personas.contains_key(name) && !is_builtin_persona(name) {
            return Err(PromptError::PersonaNotFound(name.to_string()));
        }
        self.current_persona = name.to_string();
        Ok(())
    }

    /// Get current persona name.
    #[must_use]
    pub fn current_persona(&self) -> &str {
        &self.current_persona
    }

    /// Get the prompts directory, if loaded from filesystem.
    #[must_use]
    pub fn prompts_dir(&self) -> Option<&Path> {
        if self.prompts_dir.as_os_str().is_empty() {
            None
        } else {
            Some(self.prompts_dir.as_path())
        }
    }

    /// List available personas.
    #[must_use]
    pub fn list_personas(&self) -> Vec<&str> {
        let mut names: Vec<&str> = BUILTIN_PERSONAS.to_vec();
        let mut external: Vec<&str> = self
            .personas
            .keys()
            .map(String::as_str)
            .filter(|name| !is_builtin_persona(name))
            .collect();
        external.sort_unstable();
        names.extend(external);
        names
    }

    /// Get current persona config.
    #[must_use]
    pub fn get_current_persona(&self) -> Option<&PersonaConfig> {
        self.personas.get(&self.current_persona)
    }

    /// Build the final system prompt.
    #[must_use]
    pub fn build_system_prompt(&self, ctx: &PromptContext) -> String {
        let mut prompt = String::new();

        // 1. Persona prompt or default
        if let Some(persona) = self.personas.get(&self.current_persona) {
            prompt.push_str(&persona.prompt);
            prompt.push_str("\n\n");
            for template_name in &persona.templates {
                if let Some(template) = self.templates.get(template_name) {
                    prompt.push_str(template);
                    prompt.push_str("\n\n");
                }
            }
        } else {
            prompt.push_str(&self.default_prompt);
            prompt.push_str("\n\n");
        }

        // 2. Context information
        prompt.push_str("## Current Context\n\n");
        let _ = writeln!(prompt, "- Working directory: {}", ctx.working_dir.display());
        let _ = writeln!(prompt, "- Model: {}", ctx.model);
        let _ = writeln!(prompt, "- Date: {}", ctx.today);
        if !ctx.available_tools.is_empty() {
            let _ = writeln!(prompt, "- Available tools: {}", ctx.available_tools.join(", "));
        }

        // 3. Project prompt
        if let Some(ref project_prompt) = ctx.project_prompt {
            prompt.push_str("\n## Project Instructions\n\n");
            prompt.push_str(project_prompt);
            prompt.push('\n');
        }

        // 4. Memory index
        if let Some(ref mem) = ctx.memory_user_index {
            prompt.push('\n');
            prompt.push_str(mem);
            prompt.push('\n');
        }
        if let Some(ref mem) = ctx.memory_project_index {
            prompt.push('\n');
            prompt.push_str(mem);
            prompt.push('\n');
        }

        // 5. Skills
        if !ctx.skills.is_empty() {
            prompt.push_str("\n## Available Skills\n\n");
            for skill in &ctx.skills {
                let hint = skill.argument_hint.as_deref().unwrap_or("");
                let _ = writeln!(
                    prompt,
                    "- `/{name}` {hint} — {desc}",
                    name = skill.name,
                    desc = skill.description,
                );
            }
        }

        prompt
    }

    /// Reload prompts from disk.
    ///
    /// # Errors
    /// Returns error if reload fails.
    pub fn reload(&mut self) -> Result<()> {
        if self.prompts_dir.as_os_str().is_empty() {
            return Ok(());
        }
        let reloaded = Self::from_dir(&self.prompts_dir)?;
        self.personas = reloaded.personas;
        self.templates = reloaded.templates;
        self.default_prompt = reloaded.default_prompt;
        if !self.personas.contains_key(&self.current_persona)
            && !is_builtin_persona(&self.current_persona)
        {
            self.current_persona = DEFAULT_PERSONA.to_string();
        }
        Ok(())
    }

    fn load_personas(prompts_dir: &Path) -> Result<HashMap<String, PersonaConfig>> {
        let personas_dir = prompts_dir.join("personas");
        let mut personas = HashMap::new();
        if !personas_dir.exists() {
            return Ok(personas);
        }

        let configs_dir =
            prompts_dir.parent().map(|p| p.join("configs/personas")).unwrap_or_default();

        let entries = std::fs::read_dir(&personas_dir)
            .map_err(|e| PromptError::Load(format!("personas dir: {e}")))?;

        for entry in entries {
            let entry = entry.map_err(|e| PromptError::Load(e.to_string()))?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| PromptError::Load("invalid filename".to_string()))?
                    .to_string();

                let prompt_text = std::fs::read_to_string(&path)
                    .map_err(|e| PromptError::Load(format!("{}: {e}", path.display())))?;

                let config_path = configs_dir.join(format!("{name}.toml"));
                let config = if config_path.exists() {
                    PersonaConfig::from_file(&config_path, prompt_text)?
                } else {
                    PersonaConfig::default_with_prompt(name.clone(), prompt_text)
                };

                personas.insert(name, config);
            }
        }
        Ok(personas)
    }

    fn load_templates(prompts_dir: &Path) -> Result<HashMap<String, String>> {
        let templates_dir = prompts_dir.join("templates");
        let mut templates = HashMap::new();
        if !templates_dir.exists() {
            return Ok(templates);
        }

        let entries = std::fs::read_dir(&templates_dir)
            .map_err(|e| PromptError::Load(format!("templates dir: {e}")))?;

        for entry in entries {
            let entry = entry.map_err(|e| PromptError::Load(e.to_string()))?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| PromptError::Load("invalid filename".to_string()))?
                    .to_string();

                let content = std::fs::read_to_string(&path)
                    .map_err(|e| PromptError::Load(format!("{}: {e}", path.display())))?;

                templates.insert(name, content);
            }
        }
        Ok(templates)
    }

    fn load_system_prompt(prompts_dir: &Path) -> String {
        let system_path = prompts_dir.join("system.md");
        if system_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&system_path) {
                return content;
            }
        }
        FALLBACK_SYSTEM_PROMPT.to_string()
    }
}

impl Default for PromptManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_manager_default() {
        let manager = PromptManager::new();
        assert_eq!(manager.current_persona(), DEFAULT_PERSONA);
        assert!(manager.prompts_dir().is_none());
    }

    #[test]
    fn test_build_system_prompt_fallback() {
        let manager = PromptManager::new();
        let ctx = PromptContext::default();
        let prompt = manager.build_system_prompt(&ctx);
        assert!(prompt.contains("Forge AI Agent"));
        assert!(prompt.contains("Current Context"));
    }

    #[test]
    fn test_set_persona_not_found() {
        let mut manager = PromptManager::new();
        let result = manager.set_persona("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_from_dir_with_personas() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let prompts = dir.path().join("prompts");
        let personas = prompts.join("personas");
        let templates = prompts.join("templates");
        std::fs::create_dir_all(&personas).expect("mkdir");
        std::fs::create_dir_all(&templates).expect("mkdir");

        std::fs::write(personas.join("coder.md"), "You are a coding assistant.").expect("write");
        std::fs::write(templates.join("tool_usage.md"), "Use tools wisely.").expect("write");

        let manager = PromptManager::from_dir(&prompts).expect("load");
        assert!(manager.get_current_persona().is_some());
        assert_eq!(manager.current_persona(), "coder");
    }
}
