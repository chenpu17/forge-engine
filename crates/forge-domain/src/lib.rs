//! Core domain types and traits for the Forge AI agent engine.
//!
//! This crate defines the shared abstractions used across all forge crates:
//! - Agent event types ([`AgentEvent`], [`ToolCall`], [`ToolResult`])
//! - LLM event types ([`LlmEvent`], [`Usage`])
//! - Tool system traits ([`Tool`], [`ToolOutput`], [`ConfirmationLevel`], [`ToolExecutionContext`])
//! - Project analysis types ([`ProjectType`], [`ProjectAnalysis`])
//! - Common error types ([`ForgeError`], [`ToolError`])

pub mod agent_output;
pub mod cost;
pub mod error;
pub mod event;
pub mod tool;
pub mod trace;

// Re-export commonly used types at crate root.
pub use agent_output::{
    AgentEnvelope, AgentOutput, AnalysisOutput, ConsumeResult, EnvelopeMetadata, ExploreOutput,
    Finding, GeneralOutput, PlanOutput, PlanStep, ResearchOutput, WriterOutput,
    consume_structured, try_parse_output,
};
pub use cost::{
    AgentCostRecord, AgentCostSummary, CostCheckResult, CostSnapshot, ModelPricing,
    UsageAccumulator, UsageSnapshot,
};
pub use error::{ForgeError, Result, ToolError};
pub use event::{
    AgentEvent, LlmEvent, PathConfirmation, ProjectAnalysis, ProjectCommand, ProjectType, TodoItem,
    TodoStatus, ToolCall, ToolResult, Usage,
};
pub use tool::{ConfirmationLevel, RetryConfig, Tool, ToolDef, ToolExecutionContext, ToolOutput};
pub use trace::{
    AgentTrace, TraceRequest, TraceResponse, TraceRound, TraceToolCall, TraceToolUse, TraceUsage,
};
