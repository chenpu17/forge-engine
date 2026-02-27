//! Forge Agent — Core Agent Logic
//!
//! This crate implements the core agent loop, tool orchestration,
//! result reflection, and sub-agent delegation.
//!
//! # Architecture
//!
//! The agent follows a gather-act-verify-repeat loop:
//! 1. **Prepare**: Build context and messages
//! 2. **Think**: Call LLM and stream response
//! 3. **Act**: Execute tool calls
//! 4. **Verify**: Check results
//! 5. **Reflect**: Decide whether to continue

pub mod checkpoint;
pub mod context;
pub mod episodic_memory;
pub mod executor;
pub mod mock;
pub mod planner;
pub mod project_analyzer;
pub mod reflector;
pub mod skill;
pub mod skill_context;
pub mod sub_agent;
pub mod verifier;

// The main agent loop is split into focused modules:
mod core_loop;
mod prepare;
mod stream;
mod tool_dispatch;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

// Re-export ConfirmationLevel from forge-tools
pub use forge_tools::ConfirmationLevel;

// Re-export SkillInfo from forge-prompt (used in AgentConfig)
pub use forge_prompt::SkillInfo;

// Re-export CancellationToken for SDK use
pub use tokio_util::sync::CancellationToken;

// Re-exports
pub use core_loop::{AgentEventStream, CoreAgent, HistoryMessage, HistoryRole};
pub use executor::ToolExecutor;
pub use mock::{AutoApproveHandler, MockLlmProvider, MockResponse};
pub use planner::{Planner, TodoItem, TodoStatus};
pub use project_analyzer::{Command, ProjectAnalysis, ProjectAnalyzer, ProjectType};
pub use reflector::{ErrorKind, RecoveryAction, ReflectionResult, Reflector};
pub use skill_context::SkillExecutionContext;
pub use sub_agent::{RealTaskExecutor, SubAgentConfig, SubAgentSecurity};

/// Agent-specific errors
#[derive(Debug, Error)]
pub enum AgentError {
    /// LLM service error
    #[error("LLM error: {0}")]
    LlmError(String),

    /// Tool execution error
    #[error("Tool error: {tool} - {message}")]
    ToolError {
        /// Tool name
        tool: String,
        /// Error message
        message: String,
    },

    /// Tool confirmation was rejected
    #[error("Tool confirmation rejected: {0}")]
    ToolRejected(String),

    /// Context window exceeded
    #[error("Context overflow")]
    ContextOverflow,

    /// Agent was aborted
    #[error("Agent aborted")]
    Aborted,

    /// Planning failed
    #[error("Planning failed: {0}")]
    PlanningError(String),

    /// Maximum iterations exceeded
    #[error("Max iterations exceeded: {0}")]
    MaxIterations(usize),

    /// Timeout exceeded
    #[error("Timeout after {0} seconds")]
    Timeout(u64),

    /// Session error
    #[error("Session error: {0}")]
    SessionError(String),
}

/// Result type for agent operations
pub type Result<T> = std::result::Result<T, AgentError>;

// ---------------------------------------------------------------------------
// Verifier configuration types
// ---------------------------------------------------------------------------

/// Verifier operating mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierMode {
    /// Deterministic checks only.
    #[default]
    Deterministic,
    /// Deterministic checks + optional model-assisted checks.
    Hybrid,
}

/// Verifier enforcement policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifierPolicy {
    /// Block execution on verifier failure.
    FailClosed,
    /// Report warnings but continue.
    #[default]
    WarnOnly,
}

const fn default_true() -> bool {
    true
}

/// Configuration for the verifier pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifierConfig {
    /// Whether verifier checks are enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Verifier mode.
    #[serde(default)]
    pub mode: VerifierMode,
    /// Verifier policy.
    #[serde(default)]
    pub policy: VerifierPolicy,
}

impl Default for VerifierConfig {
    fn default() -> Self {
        Self { enabled: true, mode: VerifierMode::Deterministic, policy: VerifierPolicy::WarnOnly }
    }
}

// ---------------------------------------------------------------------------
// Reflection configuration
// ---------------------------------------------------------------------------

const fn default_max_same_error_count() -> usize {
    3
}

const fn default_max_test_failure_count() -> usize {
    10
}

const fn default_max_consecutive_test_failures() -> usize {
    10
}

/// Configuration for the result reflector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionConfig {
    /// Enable reflection
    pub enabled: bool,
    /// Maximum retries for same error
    pub max_same_error_retries: usize,
    /// Maximum consecutive failures
    pub max_consecutive_failures: usize,
    /// Use LLM for error analysis
    pub use_llm_for_analysis: bool,
    /// Reflection timeout in seconds
    pub reflection_timeout_secs: u64,
    /// Threshold for same error pattern detection (default: 3)
    #[serde(default = "default_max_same_error_count")]
    pub max_same_error_count: usize,
    /// Separate threshold for test failure patterns (higher tolerance, default: 10)
    #[serde(default = "default_max_test_failure_count")]
    pub max_test_failure_count: usize,
    /// Separate threshold for consecutive test failures (higher tolerance, default: 10)
    /// This prevents early termination when running test suites that may have multiple failures
    #[serde(default = "default_max_consecutive_test_failures")]
    pub max_consecutive_test_failures: usize,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_same_error_retries: 2,
            max_consecutive_failures: 5,
            use_llm_for_analysis: false,
            reflection_timeout_secs: 30,
            max_same_error_count: default_max_same_error_count(),
            max_test_failure_count: default_max_test_failure_count(),
            max_consecutive_test_failures: default_max_consecutive_test_failures(),
        }
    }
}

/// Handler for tool confirmation requests
///
/// Implement this trait to provide custom confirmation handling.
/// The SDK implements this to manage the confirmation flow via channels.
#[async_trait]
pub trait ConfirmationHandler: Send + Sync {
    /// Request confirmation for a tool call
    async fn request_confirmation(
        &self,
        id: &str,
        tool: &str,
        params: &serde_json::Value,
        level: ConfirmationLevel,
    ) -> Result<bool>;

    /// Pre-register a confirmation so it can be resolved before the event
    /// reaches the frontend.
    async fn pre_register(&self, _id: &str) {}

    /// Wait for a previously pre-registered confirmation to be resolved.
    async fn wait_for_confirmation(
        &self,
        id: &str,
        tool: &str,
        params: &serde_json::Value,
        level: ConfirmationLevel,
    ) -> Result<bool> {
        self.request_confirmation(id, tool, params, level).await
    }
}

// ---------------------------------------------------------------------------
// Loop protection configuration
// ---------------------------------------------------------------------------

const fn default_post_completion_iterations() -> usize {
    2
}

/// Loop protection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopProtectionConfig {
    /// Maximum iterations
    pub max_iterations: usize,
    /// Single iteration timeout in seconds
    pub iteration_timeout_secs: u64,
    /// Total execution timeout in seconds
    pub total_timeout_secs: u64,
    /// Detect repeated tool calls
    pub detect_repetition: bool,
    /// Maximum same tool calls allowed (default threshold)
    pub max_same_tool_calls: usize,
    /// Tool-specific call limits (overrides `max_same_tool_calls` for specific tools)
    #[serde(default)]
    pub tool_call_limits: std::collections::HashMap<String, usize>,
    /// Maximum iterations allowed after task completion is detected
    #[serde(default = "default_post_completion_iterations")]
    pub post_completion_iterations: usize,
}

impl Default for LoopProtectionConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            iteration_timeout_secs: 300,
            total_timeout_secs: 3600,
            detect_repetition: true,
            max_same_tool_calls: 5,
            tool_call_limits: std::collections::HashMap::new(),
            post_completion_iterations: default_post_completion_iterations(),
        }
    }
}

// ---------------------------------------------------------------------------
// Generation configuration
// ---------------------------------------------------------------------------

/// Generation configuration for LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationConfig {
    /// Maximum tokens to generate
    pub max_tokens: usize,
    /// Temperature for sampling
    pub temperature: f64,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self { max_tokens: 8192, temperature: 0.7 }
    }
}

// ---------------------------------------------------------------------------
// Experimental agent configuration
// ---------------------------------------------------------------------------

/// Experimental runtime feature flags.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct ExperimentalAgentConfig {
    /// Enable streaming tool input assembly path.
    #[serde(default)]
    pub streaming_tools: bool,
    /// Maximum in-flight count for parallel read-only tool execution.
    #[serde(default)]
    pub streaming_tool_max_inflight: usize,
    /// Enable graph-hybrid stage orchestration path.
    #[serde(default)]
    pub graph_hybrid_runtime: bool,
    /// Enable durable resume v2 checkpoint schema.
    #[serde(default)]
    pub durable_resume_v2: bool,
    /// Enable verifier pipeline integration.
    #[serde(default)]
    pub verifier_pipeline: bool,
    /// Enable episodic memory read/write.
    #[serde(default)]
    pub episodic_memory: bool,
}

// ---------------------------------------------------------------------------
// Agent configuration
// ---------------------------------------------------------------------------

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Model to use
    pub model: String,
    /// Working directory
    pub working_dir: PathBuf,
    /// Project-specific prompt (from CLAUDE.md)
    pub project_prompt: Option<String>,
    /// Loop protection settings
    pub loop_protection: LoopProtectionConfig,
    /// Generation settings
    pub generation: GenerationConfig,
    /// Reflection settings
    pub reflection: ReflectionConfig,
    /// Available skills (model-invocable)
    #[serde(default)]
    pub skills: Vec<SkillInfo>,
    /// Thinking mode configuration
    #[serde(default)]
    pub thinking: Option<forge_config::ThinkingConfig>,
    /// Thinking protocol adaptor
    #[serde(default)]
    pub thinking_adaptor: forge_config::ThinkingAdaptor,
    /// Trust level for tool execution
    #[serde(default)]
    pub trust_level: forge_config::TrustLevelSetting,
    /// Memory index content for user scope
    #[serde(default)]
    pub memory_user_index: Option<String>,
    /// Memory index content for project scope
    #[serde(default)]
    pub memory_project_index: Option<String>,
    /// Permission rules for fine-grained file access control
    #[serde(default)]
    pub permission_rules: Vec<forge_config::PermissionRuleConfig>,
    /// Optional session id used by durable checkpointing.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Verifier configuration
    #[serde(default)]
    pub verifier: VerifierConfig,
    /// Experimental kernel features (all disabled by default)
    #[serde(default)]
    pub experimental: ExperimentalAgentConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-5-20250929".to_string(),
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            project_prompt: None,
            loop_protection: LoopProtectionConfig::default(),
            generation: GenerationConfig::default(),
            reflection: ReflectionConfig::default(),
            skills: Vec::new(),
            thinking: None,
            thinking_adaptor: forge_config::ThinkingAdaptor::Auto,
            trust_level: forge_config::TrustLevelSetting::default(),
            memory_user_index: None,
            memory_project_index: None,
            permission_rules: vec![],
            session_id: None,
            verifier: VerifierConfig::default(),
            experimental: ExperimentalAgentConfig::default(),
        }
    }
}

