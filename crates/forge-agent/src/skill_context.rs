//! Skill Execution Context
//!
//! Manages the execution environment for skills, including:
//! - Tool filtering based on `allowed-tools` configuration
//! - Model override for skill-specific model requirements
//! - Skill state tracking during execution

use std::collections::HashSet;

use crate::skill::{SkillDefinition, SkillError};

/// Execution context for an active skill
///
/// When a skill is invoked, this context is created to manage:
/// - Tool access restrictions
/// - Model override settings
/// - The active skill definition
#[derive(Debug, Clone)]
pub struct SkillExecutionContext {
    /// The active skill definition
    skill: SkillDefinition,

    /// Tools allowed during this skill execution
    /// None means all tools are allowed
    allowed_tools: Option<HashSet<String>>,

    /// Whether the context is active
    active: bool,
}

impl SkillExecutionContext {
    /// Create a new skill execution context
    ///
    /// # Arguments
    /// * `skill` - The skill definition (must have content loaded)
    ///
    /// # Returns
    /// Result containing the context, or error if skill content not loaded
    ///
    /// # Errors
    /// Returns `SkillError::ContentNotLoaded` if the skill content has not been loaded.
    pub fn new(skill: SkillDefinition) -> Result<Self, SkillError> {
        if !skill.is_loaded() {
            return Err(SkillError::content_not_loaded(&skill.name));
        }

        let allowed_tools =
            skill.allowed_tools().map(|tools| tools.iter().map(|t| t.to_lowercase()).collect());

        Ok(Self { skill, allowed_tools, active: true })
    }

    /// Get the active skill definition
    #[must_use]
    pub const fn skill(&self) -> &SkillDefinition {
        &self.skill
    }

    /// Get the skill name
    #[must_use]
    pub fn skill_name(&self) -> &str {
        &self.skill.name
    }

    /// Get the skill prompt
    #[must_use]
    pub fn prompt(&self) -> Option<&str> {
        self.skill.prompt()
    }

    /// Check if a tool is allowed in this context
    ///
    /// Returns true if:
    /// - No tool restrictions (`allowed_tools` is None)
    /// - Tool name is in the allowed set
    /// - Tool name matches with different casing
    #[must_use]
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        self.allowed_tools
            .as_ref()
            .is_none_or(|allowed| allowed.contains(&tool_name.to_lowercase()))
    }

    /// Get the list of allowed tools
    ///
    /// Returns None if all tools are allowed
    #[must_use]
    pub const fn allowed_tools(&self) -> Option<&HashSet<String>> {
        self.allowed_tools.as_ref()
    }

    /// Get the model override for this skill
    ///
    /// Returns None if the skill uses the default model
    #[must_use]
    pub fn model_override(&self) -> Option<&str> {
        self.skill.model_override()
    }

    /// Check if this context uses a custom model
    #[must_use]
    pub fn has_model_override(&self) -> bool {
        self.skill.model_override().is_some()
    }

    /// Get the effective model name
    ///
    /// Returns the skill's model override if set, otherwise the provided default
    #[must_use]
    pub fn effective_model<'a>(&'a self, default: &'a str) -> &'a str {
        self.skill.model_override().unwrap_or(default)
    }

    /// Check if the context is still active
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Deactivate this context (skill execution complete)
    pub const fn deactivate(&mut self) {
        self.active = false;
    }

    /// Filter a list of tool names to only those allowed
    #[must_use]
    pub fn filter_tools(&self, tools: &[String]) -> Vec<String> {
        self.allowed_tools.as_ref().map_or_else(
            || tools.to_vec(),
            |allowed| {
                tools
                    .iter()
                    .filter(|t| allowed.contains(&t.to_lowercase()))
                    .cloned()
                    .collect()
            },
        )
    }

    /// Validate that a tool is allowed, returning error if not
    ///
    /// # Errors
    /// Returns `SkillError::ToolNotAllowed` if the tool is not in the allowed set.
    pub fn validate_tool(&self, tool_name: &str) -> Result<(), SkillError> {
        if self.is_tool_allowed(tool_name) {
            Ok(())
        } else {
            let allowed = self
                .allowed_tools
                .as_ref()
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            Err(SkillError::tool_not_allowed(tool_name, &self.skill.name, allowed))
        }
    }
}

/// Builder for `SkillExecutionContext` with additional configuration
pub struct SkillContextBuilder {
    skill: SkillDefinition,
    additional_tools: Option<Vec<String>>,
    override_model: Option<String>,
}

impl SkillContextBuilder {
    /// Create a new builder with a skill
    #[must_use]
    pub const fn new(skill: SkillDefinition) -> Self {
        Self { skill, additional_tools: None, override_model: None }
    }

    /// Add additional tools beyond what the skill specifies
    ///
    /// These tools will be added to the allowed list (only effective if
    /// skill has tool restrictions)
    #[must_use]
    pub fn with_additional_tools(mut self, tools: Vec<String>) -> Self {
        self.additional_tools = Some(tools);
        self
    }

    /// Override the skill's model setting
    #[must_use]
    pub fn with_model_override(mut self, model: String) -> Self {
        self.override_model = Some(model);
        self
    }

    /// Build the execution context
    ///
    /// # Errors
    /// Returns `SkillError::ContentNotLoaded` if the skill content has not been loaded.
    pub fn build(mut self) -> Result<SkillExecutionContext, SkillError> {
        if !self.skill.is_loaded() {
            return Err(SkillError::content_not_loaded(&self.skill.name));
        }

        // Apply model override if specified
        if let Some(model) = self.override_model {
            self.skill.metadata.model = Some(model);
        }

        let mut allowed_tools = self
            .skill
            .allowed_tools()
            .map(|tools| tools.iter().map(|t| t.to_lowercase()).collect::<HashSet<_>>());

        // Add additional tools if specified and skill has restrictions
        if let (Some(additional), Some(ref mut allowed)) =
            (&self.additional_tools, &mut allowed_tools)
        {
            for tool in additional {
                allowed.insert(tool.to_lowercase());
            }
        }

        Ok(SkillExecutionContext { skill: self.skill, allowed_tools, active: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::{SkillContent, SkillFrontmatter, SkillSource};
    use std::path::PathBuf;

    fn create_test_skill(name: &str, allowed_tools: Option<Vec<String>>) -> SkillDefinition {
        let metadata =
            SkillFrontmatter { name: Some(name.to_string()), allowed_tools, ..Default::default() };

        let content = SkillContent::new("Test prompt".to_string());

        SkillDefinition::with_content(
            name.to_string(),
            metadata,
            content,
            SkillSource::Builtin,
            PathBuf::from("test"),
            true,
        )
    }

    #[test]
    fn test_no_tool_restrictions() {
        let skill = create_test_skill("test", None);
        let ctx = SkillExecutionContext::new(skill).unwrap();

        assert!(ctx.is_tool_allowed("Bash"));
        assert!(ctx.is_tool_allowed("Read"));
        assert!(ctx.is_tool_allowed("anything"));
        assert!(ctx.allowed_tools().is_none());
    }

    #[test]
    fn test_with_tool_restrictions() {
        let skill = create_test_skill(
            "commit",
            Some(vec!["Bash".to_string(), "Read".to_string(), "Glob".to_string()]),
        );
        let ctx = SkillExecutionContext::new(skill).unwrap();

        assert!(ctx.is_tool_allowed("Bash"));
        assert!(ctx.is_tool_allowed("bash")); // Case insensitive
        assert!(ctx.is_tool_allowed("BASH"));
        assert!(ctx.is_tool_allowed("Read"));
        assert!(ctx.is_tool_allowed("Glob"));

        assert!(!ctx.is_tool_allowed("Write"));
        assert!(!ctx.is_tool_allowed("Edit"));
    }

    #[test]
    fn test_validate_tool() {
        let skill = create_test_skill("test", Some(vec!["Read".to_string()]));
        let ctx = SkillExecutionContext::new(skill).unwrap();

        assert!(ctx.validate_tool("Read").is_ok());
        assert!(ctx.validate_tool("read").is_ok());

        let err = ctx.validate_tool("Write").unwrap_err();
        match err {
            SkillError::ToolNotAllowed { tool, skill, .. } => {
                assert_eq!(tool, "Write");
                assert_eq!(skill, "test");
            }
            _ => panic!("Expected ToolNotAllowed error"),
        }
    }

    #[test]
    fn test_filter_tools() {
        let skill = create_test_skill("test", Some(vec!["Bash".to_string(), "Read".to_string()]));
        let ctx = SkillExecutionContext::new(skill).unwrap();

        let all_tools =
            vec!["Bash".to_string(), "Read".to_string(), "Write".to_string(), "Edit".to_string()];

        let filtered = ctx.filter_tools(&all_tools);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.contains(&"Bash".to_string()));
        assert!(filtered.contains(&"Read".to_string()));
    }

    #[test]
    fn test_model_override() {
        let mut skill = create_test_skill("test", None);
        skill.metadata.model = Some("claude-opus-4-5".to_string());

        let ctx = SkillExecutionContext::new(skill).unwrap();

        assert!(ctx.has_model_override());
        assert_eq!(ctx.model_override(), Some("claude-opus-4-5"));
        assert_eq!(ctx.effective_model("claude-sonnet"), "claude-opus-4-5");
    }

    #[test]
    fn test_no_model_override() {
        let skill = create_test_skill("test", None);
        let ctx = SkillExecutionContext::new(skill).unwrap();

        assert!(!ctx.has_model_override());
        assert_eq!(ctx.model_override(), None);
        assert_eq!(ctx.effective_model("claude-sonnet"), "claude-sonnet");
    }

    #[test]
    fn test_context_lifecycle() {
        let skill = create_test_skill("test", None);
        let mut ctx = SkillExecutionContext::new(skill).unwrap();

        assert!(ctx.is_active());
        ctx.deactivate();
        assert!(!ctx.is_active());
    }

    #[test]
    fn test_builder_with_additional_tools() {
        let skill = create_test_skill("test", Some(vec!["Bash".to_string()]));

        let ctx = SkillContextBuilder::new(skill)
            .with_additional_tools(vec!["WebSearch".to_string()])
            .build()
            .unwrap();

        assert!(ctx.is_tool_allowed("Bash"));
        assert!(ctx.is_tool_allowed("WebSearch"));
        assert!(!ctx.is_tool_allowed("Write"));
    }

    #[test]
    fn test_error_on_unloaded_skill() {
        let metadata = SkillFrontmatter::default();
        let skill = SkillDefinition::metadata_only(
            "test".to_string(),
            metadata,
            SkillSource::Builtin,
            PathBuf::from("test"),
            true,
        );

        let result = SkillExecutionContext::new(skill);
        assert!(matches!(result, Err(SkillError::ContentNotLoaded { .. })));
    }
}
