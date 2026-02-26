//! Runtime prompt context.

use std::path::PathBuf;

use crate::persona::SkillInfo;

/// Runtime context for prompt building.
#[derive(Debug, Clone)]
pub struct PromptContext {
    /// Working directory.
    pub working_dir: PathBuf,
    /// Available tools.
    pub available_tools: Vec<String>,
    /// Project-specific prompt (from CLAUDE.md / FORGE.md).
    pub project_prompt: Option<String>,
    /// Model name.
    pub model: String,
    /// Today's date.
    pub today: String,
    /// Available skills (model-invocable).
    pub skills: Vec<SkillInfo>,
    /// Memory index content for user scope.
    pub memory_user_index: Option<String>,
    /// Memory index content for project scope.
    pub memory_project_index: Option<String>,
}

impl Default for PromptContext {
    fn default() -> Self {
        Self {
            working_dir: std::env::current_dir().unwrap_or_default(),
            available_tools: Vec::new(),
            project_prompt: None,
            model: "unknown".to_string(),
            today: chrono::Local::now().format("%Y-%m-%d").to_string(),
            skills: Vec::new(),
            memory_user_index: None,
            memory_project_index: None,
        }
    }
}

impl PromptContext {
    /// Create a builder.
    #[must_use]
    pub fn builder() -> PromptContextBuilder {
        PromptContextBuilder::default()
    }
}

/// Builder for [`PromptContext`].
#[derive(Default)]
pub struct PromptContextBuilder {
    ctx: PromptContext,
}

impl PromptContextBuilder {
    /// Set working directory.
    #[must_use]
    pub fn working_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.ctx.working_dir = path.into();
        self
    }

    /// Set available tools.
    #[must_use]
    pub fn tools(mut self, tools: Vec<String>) -> Self {
        self.ctx.available_tools = tools;
        self
    }

    /// Set project prompt.
    #[must_use]
    pub fn project_prompt(mut self, prompt: Option<String>) -> Self {
        self.ctx.project_prompt = prompt;
        self
    }

    /// Set model name.
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.ctx.model = model.into();
        self
    }

    /// Set available skills.
    #[must_use]
    pub fn skills(mut self, skills: Vec<SkillInfo>) -> Self {
        self.ctx.skills = skills;
        self
    }

    /// Set user memory index content.
    #[must_use]
    pub fn memory_user_index(mut self, index: Option<String>) -> Self {
        self.ctx.memory_user_index = index;
        self
    }

    /// Set project memory index content.
    #[must_use]
    pub fn memory_project_index(mut self, index: Option<String>) -> Self {
        self.ctx.memory_project_index = index;
        self
    }

    /// Build the context.
    #[must_use]
    pub fn build(self) -> PromptContext {
        self.ctx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_context_builder() {
        let ctx = PromptContext::builder()
            .working_dir("/tmp/test")
            .model("claude-3")
            .tools(vec!["bash".to_string(), "read".to_string()])
            .build();
        assert_eq!(ctx.working_dir.to_str().unwrap(), "/tmp/test");
        assert_eq!(ctx.model, "claude-3");
        assert_eq!(ctx.available_tools.len(), 2);
    }
}
