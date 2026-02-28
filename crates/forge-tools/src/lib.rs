//! Forge Tools — Tool System
//!
//! This crate provides the tool abstraction layer, built-in tools,
//! and script plugin support.
//!
//! Core types (`Tool`, `ToolOutput`, `ToolDef`, `ConfirmationLevel`,
//! `RetryConfig`, `ToolExecutionContext`, `ToolError`) live in `forge_domain`.
//! This crate provides the concrete `ToolContext`, `ToolRegistry`, and
//! all built-in tool implementations.

pub mod background;
pub mod builtin;
pub mod description;
pub mod hardcoded_safety;
pub mod metrics;
pub mod params;
pub mod path_utils;
pub mod permission_policy;
pub mod platform;
pub mod plugin;
pub mod security;
pub mod schema;
pub mod shell_path;
pub mod trust_permission;

// Re-export background task types
pub use background::{
    BackgroundTaskManager, BackgroundTaskStatus, BackgroundTaskSummary, BackgroundTaskType,
    TaskOutputResult,
};

// Re-export tool description loader
pub use description::ToolDescriptions;

// Re-export parameter extraction utilities
pub use params::{
    optional_bool, optional_f64, optional_i64, optional_str, optional_u64, optional_usize,
    required_str, string_array,
};

// Re-export shell path extraction
pub use shell_path::{extract_paths_from_command, extract_redirect_targets};

// Re-export domain types for convenience
pub use forge_domain::{
    ConfirmationLevel, RetryConfig, Tool, ToolDef, ToolError, ToolExecutionContext, ToolOutput,
};

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// ToolContext — concrete execution context
// ---------------------------------------------------------------------------

/// Tool execution context.
///
/// Carries runtime state needed by tool implementations. Implements
/// [`forge_domain::ToolExecutionContext`] so it can be passed through the
/// domain `Tool::execute` interface.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Current working directory.
    pub working_dir: std::path::PathBuf,
    /// Environment variables.
    pub env: HashMap<String, String>,
    /// Timeout in seconds.
    pub timeout_secs: u64,
    /// Paths confirmed by user (allowed even if outside working directory).
    pub confirmed_paths: std::collections::HashSet<std::path::PathBuf>,
    /// Bash read-only mode (blocks write operations in bash).
    pub bash_readonly: bool,
    /// Plan mode active flag (blocks write operations when true).
    /// Uses `Arc<AtomicBool>` for real-time checking within a single `process()` call.
    pub plan_mode_flag: Arc<AtomicBool>,
    /// Current sub-agent nesting depth (0 = main agent).
    pub subagent_nesting_depth: usize,
    /// Optional LSP manager for code intelligence tools.
    pub lsp_manager: Option<Arc<forge_lsp::LspManager>>,
}

impl ToolContext {
    /// Check if plan mode is currently active.
    #[inline]
    #[must_use]
    pub fn is_plan_mode_active(&self) -> bool {
        self.plan_mode_flag.load(Ordering::Acquire)
    }

    /// Set plan mode state.
    pub fn set_plan_mode_active(&self, active: bool) {
        self.plan_mode_flag.store(active, Ordering::Release);
    }

    /// Get the plan mode flag for sharing with SDK.
    #[must_use]
    pub fn plan_mode_flag(&self) -> Arc<AtomicBool> {
        self.plan_mode_flag.clone()
    }

    /// Create a new `ToolContext` with a shared plan mode flag.
    #[must_use]
    pub fn with_plan_mode_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.plan_mode_flag = flag;
        self
    }
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            working_dir: std::env::current_dir().unwrap_or_default(),
            env: std::env::vars().collect(),
            timeout_secs: 120,
            confirmed_paths: std::collections::HashSet::new(),
            bash_readonly: false,
            plan_mode_flag: Arc::new(AtomicBool::new(false)),
            subagent_nesting_depth: 0,
            lsp_manager: None,
        }
    }
}

// Implement the domain trait so `ToolContext` can be used with `Tool::execute`.
impl ToolExecutionContext for ToolContext {
    fn working_dir(&self) -> &Path {
        &self.working_dir
    }

    fn bash_readonly(&self) -> bool {
        self.bash_readonly
    }

    fn plan_mode_active(&self) -> bool {
        self.is_plan_mode_active()
    }

    fn subagent_nesting_depth(&self) -> usize {
        self.subagent_nesting_depth
    }

    fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    fn confirmed_paths(&self) -> &std::collections::HashSet<std::path::PathBuf> {
        &self.confirmed_paths
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ---------------------------------------------------------------------------
// ToolRegistry
// ---------------------------------------------------------------------------

/// Tool registry — stores named tool instances.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Create a new empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    /// Register a tool.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Get a tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Get all tool definitions.
    #[must_use]
    pub fn all_defs(&self) -> Vec<ToolDef> {
        self.tools.values().map(|t| t.to_def()).collect()
    }

    /// List all tool names.
    #[must_use]
    pub fn list_names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }

    /// List all registered tools.
    #[must_use]
    pub fn list_all(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.values().cloned().collect()
    }

    /// Unregister a tool by name.
    ///
    /// Returns `true` if the tool was found and removed.
    pub fn unregister(&mut self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }
}
