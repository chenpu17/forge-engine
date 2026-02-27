//! Skill definition types.
//!
//! Minimal skill types for the agent crate. These will be replaced
//! when the full skill system is migrated.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

/// Skill-related errors
#[derive(Debug, Error)]
pub enum SkillError {
    /// Skill content not loaded
    #[error("Skill '{name}' content not loaded")]
    ContentNotLoaded {
        /// Skill name
        name: String,
    },

    /// Tool not allowed in skill context
    #[error("Tool '{tool}' not allowed in skill '{skill}'")]
    ToolNotAllowed {
        /// Tool name
        tool: String,
        /// Skill name
        skill: String,
        /// Allowed tools
        allowed: Vec<String>,
    },
}

impl SkillError {
    /// Create a content-not-loaded error
    #[must_use]
    pub fn content_not_loaded(name: &str) -> Self {
        Self::ContentNotLoaded { name: name.to_string() }
    }

    /// Create a tool-not-allowed error
    #[must_use]
    pub fn tool_not_allowed(tool: &str, skill: &str, allowed: Vec<String>) -> Self {
        Self::ToolNotAllowed {
            tool: tool.to_string(),
            skill: skill.to_string(),
            allowed,
        }
    }
}

/// Skill source location
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    /// Built-in skill
    Builtin,
    /// User-defined skill
    User,
    /// Project-level skill
    Project,
}

/// Skill frontmatter metadata
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Skill name override
    pub name: Option<String>,
    /// Allowed tools (None = all tools allowed)
    pub allowed_tools: Option<Vec<String>>,
    /// Model override
    pub model: Option<String>,
    /// Description
    pub description: Option<String>,
}

/// Loaded skill content
#[derive(Debug, Clone)]
pub struct SkillContent {
    /// The prompt text
    prompt: String,
}

impl SkillContent {
    /// Create new skill content
    #[must_use]
    pub const fn new(prompt: String) -> Self {
        Self { prompt }
    }

    /// Get the prompt text
    #[must_use]
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
}

/// Skill definition
#[derive(Debug, Clone)]
pub struct SkillDefinition {
    /// Skill name
    pub name: String,
    /// Metadata from frontmatter
    pub metadata: SkillFrontmatter,
    /// Loaded content (None if not yet loaded)
    content: Option<SkillContent>,
    /// Source location
    pub source: SkillSource,
    /// File path
    pub path: PathBuf,
    /// Whether this skill is user-invocable
    pub user_invocable: bool,
}

impl SkillDefinition {
    /// Create a skill with content loaded
    #[must_use]
    pub const fn with_content(
        name: String,
        metadata: SkillFrontmatter,
        content: SkillContent,
        source: SkillSource,
        path: PathBuf,
        user_invocable: bool,
    ) -> Self {
        Self { name, metadata, content: Some(content), source, path, user_invocable }
    }

    /// Create a skill with metadata only (content not loaded)
    #[must_use]
    pub const fn metadata_only(
        name: String,
        metadata: SkillFrontmatter,
        source: SkillSource,
        path: PathBuf,
        user_invocable: bool,
    ) -> Self {
        Self { name, metadata, content: None, source, path, user_invocable }
    }

    /// Check if content is loaded
    #[must_use]
    pub const fn is_loaded(&self) -> bool {
        self.content.is_some()
    }

    /// Get the prompt text
    #[must_use]
    pub fn prompt(&self) -> Option<&str> {
        self.content.as_ref().map(SkillContent::prompt)
    }

    /// Get allowed tools from metadata
    #[must_use]
    pub fn allowed_tools(&self) -> Option<&[String]> {
        self.metadata.allowed_tools.as_deref()
    }

    /// Get model override from metadata
    #[must_use]
    pub fn model_override(&self) -> Option<&str> {
        self.metadata.model.as_deref()
    }
}
