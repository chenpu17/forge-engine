//! Core domain types and traits for the Forge AI agent engine.
//!
//! This crate defines the shared abstractions used across all forge crates:
//! - Agent event types ([`AgentEvent`], [`ToolCall`], [`ToolResult`])
//! - LLM event types ([`LlmEvent`], [`Usage`])
//! - Tool system traits ([`Tool`], [`ToolOutput`], [`ConfirmationLevel`], [`ToolExecutionContext`])
//! - Project analysis types ([`ProjectType`], [`ProjectAnalysis`])
//! - Common error types ([`ForgeError`], [`ToolError`])

pub mod error;
pub mod event;
pub mod tool;

// Re-export commonly used types at crate root.
pub use error::{ForgeError, Result, ToolError};
pub use event::{
    AgentEvent, LlmEvent, PathConfirmation, ProjectAnalysis, ProjectCommand, ProjectType,
    TodoItem, TodoStatus, ToolCall, ToolResult, Usage,
};
pub use tool::{
    ConfirmationLevel, RetryConfig, Tool, ToolDef, ToolExecutionContext, ToolOutput,
};
