//! Forge SDK — entry point for the Forge AI agent engine.
//!
//! Provides [`ForgeSDK`] and [`ForgeSDKBuilder`] for constructing and
//! driving agent sessions from any frontend (CLI, Desktop, Web, etc.).

mod builder;
mod config;
mod error;
mod event;
pub mod extensions;
mod sdk;
mod session;
mod types;

// Primary API
pub use builder::ForgeSDKBuilder;
pub use sdk::ForgeSDK;
pub use config::{
    LlmSettings, ObservabilityConfig, ForgeConfig, SessionSettings, ToolsSettings,
};
pub use error::{ForgeError, Result};
pub use event::{AgentEvent, AgentEventExt, TodoItem, TokenUsage};
pub use session::{SessionId, SessionSummary};
pub use types::{
    CompressionResult, EventDispatchMode, McpConnectionTestResult, McpServerInfo,
    McpServerManageConfig, McpServerStatus, McpStatus, McpToolInfo, McpTransportType,
    MemoryScope, ModelSwitchResult, ProcessOptions, ProxyInfo, RequestId, SessionStatus,
    ToolCategory, ToolInfo,
};

// Re-export useful types from dependencies
pub use forge_agent::{HistoryMessage, HistoryRole};
pub use forge_domain::{ProjectAnalysis, ProjectType};
pub use forge_prompt::{PersonaConfig, PromptContext, PromptManager};
pub use forge_tools::ToolDescriptions;
