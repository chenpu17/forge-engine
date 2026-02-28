//! Core tool traits and types.

use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Tool execution context (minimal trait — concrete ToolContext in forge-tools)
// ---------------------------------------------------------------------------

/// Minimal execution context passed to tools.
///
/// `forge-tools` provides the concrete `ToolContext` that implements this trait
/// with additional fields (sandbox, background manager, optional extensions, etc.).
///
/// The `'static` bound enables downcasting via [`as_any()`](Self::as_any)
/// for tools that need access to concrete context fields.
pub trait ToolExecutionContext: Send + Sync + 'static {
    /// Current working directory.
    fn working_dir(&self) -> &Path;

    /// Whether bash is in read-only mode.
    fn bash_readonly(&self) -> bool;

    /// Whether plan mode is currently active.
    fn plan_mode_active(&self) -> bool;

    /// Current sub-agent nesting depth.
    fn subagent_nesting_depth(&self) -> usize;

    /// Timeout in seconds for tool execution.
    fn timeout_secs(&self) -> u64;

    /// Downcast to `&dyn Any` for concrete type access.
    ///
    /// Tools that need access to concrete context fields (e.g. extension managers)
    /// can downcast via this method.
    fn as_any(&self) -> &dyn std::any::Any;
}

// ---------------------------------------------------------------------------
// Tool trait
// ---------------------------------------------------------------------------

/// Core tool interface.
///
/// All built-in tools, script plugins, and MCP wrappers implement this trait.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (unique identifier).
    fn name(&self) -> &str;

    /// Tool description (shown to LLM).
    fn description(&self) -> &str;

    /// Parameters JSON Schema.
    fn parameters_schema(&self) -> Value;

    /// Execute the tool with the given parameters and context.
    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, crate::error::ToolError>;

    /// Optional prewarm hook for lightweight initialization.
    async fn prewarm(
        &self,
        _params: Value,
        _ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<(), crate::error::ToolError> {
        Ok(())
    }

    /// Confirmation level required for this tool call.
    fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
        ConfirmationLevel::None
    }

    /// Retry configuration for this tool.
    fn retry_config(&self) -> RetryConfig {
        RetryConfig::NONE
    }

    /// Whether this tool is read-only (does not modify any state).
    fn is_readonly(&self) -> bool {
        false
    }

    /// Whether this tool requires network access.
    fn requires_network(&self) -> bool {
        false
    }

    /// Convert to tool definition for LLM.
    fn to_def(&self) -> ToolDef {
        ToolDef {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool output
// ---------------------------------------------------------------------------

/// Tool execution output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    /// Output content.
    pub content: String,
    /// Whether the execution resulted in an error.
    pub is_error: bool,
    /// Optional structured data.
    pub data: Option<Value>,
}

impl ToolOutput {
    /// Create a successful output.
    #[must_use]
    pub fn success(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false, data: None }
    }

    /// Create an error output.
    #[must_use]
    pub fn error(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true, data: None }
    }
}

// ---------------------------------------------------------------------------
// Tool definition (sent to LLM)
// ---------------------------------------------------------------------------

/// Tool definition for LLM function calling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// Parameters JSON Schema.
    pub parameters: Value,
}

// ---------------------------------------------------------------------------
// Confirmation level
// ---------------------------------------------------------------------------

/// Confirmation level for tool execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfirmationLevel {
    /// No confirmation needed — safe operations.
    #[default]
    None,
    /// Confirm once, then allow similar commands automatically.
    Once,
    /// Always require confirmation.
    Always,
    /// Dangerous operation — require extra warning.
    Dangerous,
}

// ---------------------------------------------------------------------------
// Retry configuration
// ---------------------------------------------------------------------------

/// Retry configuration for tools.
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 means no retries).
    pub max_retries: u32,
    /// Initial delay between retries in milliseconds.
    pub initial_delay_ms: u64,
    /// Whether to use exponential backoff.
    pub exponential_backoff: bool,
}

impl RetryConfig {
    /// No retries (default for local tools).
    pub const NONE: Self = Self {
        max_retries: 0,
        initial_delay_ms: 0,
        exponential_backoff: false,
    };

    /// Standard network retry config (3 retries with exponential backoff).
    pub const NETWORK: Self = Self {
        max_retries: 3,
        initial_delay_ms: 1000,
        exponential_backoff: true,
    };

    /// Check if retries are enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.max_retries > 0
    }

    /// Calculate delay for a given attempt (0-indexed).
    #[must_use]
    pub const fn delay_for_attempt(&self, attempt: u32) -> u64 {
        if self.exponential_backoff {
            self.initial_delay_ms * (1 << attempt)
        } else {
            self.initial_delay_ms
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self::NONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_confirmation_level_default() {
        assert_eq!(ConfirmationLevel::default(), ConfirmationLevel::None);
    }

    #[test]
    fn test_confirmation_level_serde() {
        let json = serde_json::to_string(&ConfirmationLevel::Dangerous).expect("serialize");
        let parsed: ConfirmationLevel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, ConfirmationLevel::Dangerous);
    }

    #[test]
    fn test_retry_config_none() {
        let cfg = RetryConfig::NONE;
        assert!(!cfg.is_enabled());
        assert_eq!(cfg.delay_for_attempt(0), 0);
    }

    #[test]
    fn test_retry_config_network() {
        let cfg = RetryConfig::NETWORK;
        assert!(cfg.is_enabled());
        assert_eq!(cfg.delay_for_attempt(0), 1000);
        assert_eq!(cfg.delay_for_attempt(1), 2000);
        assert_eq!(cfg.delay_for_attempt(2), 4000);
    }

    #[test]
    fn test_tool_output_serde() {
        let out = ToolOutput {
            content: "hello".to_string(),
            is_error: false,
            data: Some(serde_json::json!({"key": "value"})),
        };
        let json = serde_json::to_string(&out).expect("serialize");
        let parsed: ToolOutput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.content, "hello");
        assert!(!parsed.is_error);
        assert!(parsed.data.is_some());
    }

    #[test]
    fn test_tool_def_serde() {
        let def = ToolDef {
            name: "test".to_string(),
            description: "A test tool".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let json = serde_json::to_string(&def).expect("serialize");
        let parsed: ToolDef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.name, "test");
    }
}
