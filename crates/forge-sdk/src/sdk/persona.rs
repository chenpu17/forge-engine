//! Persona, skill, and runtime config management for ForgeSDK

use super::*;

impl ForgeSDK {
    /// Set the current persona.
    ///
    /// # Errors
    ///
    /// Returns error if persona not found.
    pub async fn set_persona(&self, name: &str) -> Result<()> {
        {
            let mut pm = self.prompt_manager.write().await;
            pm.set_persona(name)?;
        }
        self.refresh_subagent_security().await;
        Ok(())
    }

    /// Get the current persona name.
    pub async fn current_persona(&self) -> String {
        self.prompt_manager.read().await.current_persona().to_string()
    }

    /// List available personas.
    pub async fn list_personas(&self) -> Vec<String> {
        self.prompt_manager
            .read()
            .await
            .list_personas()
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// List all discovered skills (metadata only).
    pub async fn list_skills(&self) -> Vec<SkillInfo> {
        self.skill_registry.list_all()
    }

    /// Reload skills from disk.
    ///
    /// # Errors
    ///
    /// Returns error if skill loading fails.
    pub async fn reload_skills(&self) -> Result<usize> {
        Ok(self.skill_registry.reload()?)
    }

    /// Get a skill with full prompt content loaded.
    ///
    /// # Errors
    ///
    /// Returns error if skill not found.
    pub async fn get_skill_full(&self, name: &str) -> Result<forge_agent::skill::SkillDefinition> {
        Ok(self.skill_registry.get_full(name)?)
    }

    /// Get configured skill search paths.
    pub async fn get_skill_paths(&self) -> Vec<(PathBuf, SkillSource)> {
        self.skill_registry.get_skill_paths()
    }

    /// Whether project-local skills are trusted and loaded.
    pub async fn is_project_skills_trusted(&self) -> bool {
        self.config.read().await.trust_project_skills
    }

    /// Get a copy of the configuration.
    pub async fn config(&self) -> ForgeConfig {
        self.config.read().await.clone()
    }

    /// Update thinking mode configuration at runtime.
    pub async fn set_thinking_enabled(&self, enabled: bool, budget_tokens: Option<usize>) {
        let mut config = self.config.write().await;
        config.llm.thinking = Some(forge_config::ThinkingConfig {
            enabled,
            budget_tokens: if enabled { budget_tokens } else { None },
            effort: config.llm.thinking.as_ref().and_then(|t| t.effort),
            preserve_history: config.llm.thinking.as_ref().and_then(|t| t.preserve_history),
        });
    }

    /// Reload prompts from disk.
    ///
    /// # Errors
    ///
    /// Returns error if prompt loading fails.
    pub async fn reload_prompts(&self) -> Result<()> {
        self.prompt_manager.write().await.reload()?;
        self.refresh_subagent_security().await;
        Ok(())
    }
}
