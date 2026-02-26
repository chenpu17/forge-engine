//! Prompt management and persona system for the Forge AI agent engine.
//!
//! This crate provides:
//! - Persona management ([`PersonaConfig`], [`PersonaOptions`])
//! - Prompt template composition ([`PromptManager`])
//! - Runtime prompt context ([`PromptContext`], [`PromptContextBuilder`])
//! - Project documentation discovery ([`ProjectPromptLoader`])

pub mod context;
pub mod error;
pub mod loader;
pub mod manager;
pub mod persona;

pub use context::{PromptContext, PromptContextBuilder};
pub use error::{PromptError, Result};
pub use loader::ProjectPromptLoader;
pub use manager::PromptManager;
pub use persona::{PersonaConfig, PersonaOptions, SkillInfo};
