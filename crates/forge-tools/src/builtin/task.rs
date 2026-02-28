//! Task tool for launching sub-agents
//!
//! The Task tool allows the main agent to spawn specialized sub-agents
//! for handling complex, multi-step tasks autonomously.

use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, Tool, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r"Launch a sub-agent to handle complex, multi-step tasks autonomously.

Use this tool when:
- The task requires multiple steps or extensive exploration
- You need to search for code patterns across many files
- The task involves complex research or planning
- You want to delegate a self-contained subtask

Available sub-agent types:
- general-purpose: For complex multi-step tasks (coding, file modification, testing)
- explore: Fast project exploration (finding files, searching content)
- plan: Designing implementation strategies
- research: Gathering information from documentation
- writer: Content creation, document writing, reports, emails, proposals
- data-analyst: Data processing, statistical analysis, visualization scripts

The sub-agent will execute autonomously and return a final report.";

/// Model tier for sub-agent execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    /// Use fast model (e.g., claude-3-haiku) for quick tasks
    Fast,
    /// Use default model (inherits from parent or config)
    #[default]
    Default,
    /// Use powerful model (e.g., claude-opus-4) for complex tasks
    Powerful,
}

impl ModelTier {
    /// Parse from string
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "fast" | "haiku" => Some(Self::Fast),
            "default" | "sonnet" => Some(Self::Default),
            "powerful" | "opus" => Some(Self::Powerful),
            _ => None,
        }
    }
}

impl std::fmt::Display for ModelTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fast => write!(f, "fast"),
            Self::Default => write!(f, "default"),
            Self::Powerful => write!(f, "powerful"),
        }
    }
}

/// Sub-agent type for specialized tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubAgentType {
    /// General-purpose agent for complex tasks
    GeneralPurpose,
    /// Fast agent for exploring projects
    Explore,
    /// Planner agent for designing strategies
    Plan,
    /// Agent for researching documentation
    Research,
    /// Content creation and document writing agent
    Writer,
    /// Data analysis and report generation agent
    DataAnalyst,
}

impl SubAgentType {
    /// Get the display name for this agent type
    #[must_use]
    pub const fn display_name(&self) -> &'static str {
        match self {
            Self::GeneralPurpose => "General Purpose",
            Self::Explore => "Explore",
            Self::Plan => "Plan",
            Self::Research => "Research",
            Self::Writer => "Writer",
            Self::DataAnalyst => "Data Analyst",
        }
    }

    /// Get the description for this agent type
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::GeneralPurpose => {
                "General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks"
            }
            Self::Explore => {
                "Fast agent specialized for exploring projects - finding files, searching content, answering questions about structure"
            }
            Self::Plan => {
                "Planner agent for designing implementation plans with step-by-step strategies"
            }
            Self::Research => {
                "Agent for researching documentation, APIs, and gathering information from various sources"
            }
            Self::Writer => {
                "Content creation agent for writing documents, reports, emails, proposals, and other text content"
            }
            Self::DataAnalyst => {
                "Data analysis agent for processing data, statistical analysis, generating visualization scripts and reports"
            }
        }
    }

    /// Parse from string
    #[must_use]
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "general-purpose" | "general" => Some(Self::GeneralPurpose),
            "explore" | "explorer" => Some(Self::Explore),
            "plan" | "planner" => Some(Self::Plan),
            "research" | "researcher" => Some(Self::Research),
            "writer" => Some(Self::Writer),
            "data-analyst" | "analyst" => Some(Self::DataAnalyst),
            _ => None,
        }
    }
}

impl std::fmt::Display for SubAgentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GeneralPurpose => write!(f, "general-purpose"),
            Self::Explore => write!(f, "explore"),
            Self::Plan => write!(f, "plan"),
            Self::Research => write!(f, "research"),
            Self::Writer => write!(f, "writer"),
            Self::DataAnalyst => write!(f, "data-analyst"),
        }
    }
}

/// Task execution status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task is pending
    Pending,
    /// Task is running
    Running,
    /// Task completed successfully
    Completed,
    /// Task failed
    Failed,
    /// Task was cancelled
    Cancelled,
}

/// A task instance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInstance {
    /// Unique task ID
    pub id: String,
    /// Short description of the task
    pub description: String,
    /// The prompt/instructions for the sub-agent
    pub prompt: String,
    /// Type of sub-agent to use
    pub subagent_type: SubAgentType,
    /// Model tier to use (fast/default/powerful)
    pub model_tier: ModelTier,
    /// Maximum turns (iterations) for the sub-agent
    pub max_turns: Option<usize>,
    /// Whether to run in background
    pub run_in_background: bool,
    /// Nesting depth in the sub-agent call chain (0 = spawned by main agent)
    pub nesting_depth: usize,
    /// Current status
    pub status: TaskStatus,
    /// Result from the sub-agent (when completed)
    pub result: Option<String>,
    /// Error message (when failed)
    pub error: Option<String>,
    /// Created timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Completed timestamp
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl TaskInstance {
    /// Create a new task instance
    #[must_use]
    pub fn new(description: String, prompt: String, subagent_type: SubAgentType) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            description,
            prompt,
            subagent_type,
            model_tier: ModelTier::Default,
            max_turns: None,
            run_in_background: false,
            nesting_depth: 0,
            status: TaskStatus::Pending,
            result: None,
            error: None,
            created_at: chrono::Utc::now(),
            completed_at: None,
        }
    }

    /// Create a new task instance with all options
    #[must_use]
    pub fn with_options(
        description: String,
        prompt: String,
        subagent_type: SubAgentType,
        model_tier: ModelTier,
        max_turns: Option<usize>,
        run_in_background: bool,
        nesting_depth: usize,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            description,
            prompt,
            subagent_type,
            model_tier,
            max_turns,
            run_in_background,
            nesting_depth,
            status: TaskStatus::Pending,
            result: None,
            error: None,
            created_at: chrono::Utc::now(),
            completed_at: None,
        }
    }

    /// Mark task as running
    pub const fn start(&mut self) {
        self.status = TaskStatus::Running;
    }

    /// Mark task as completed with result
    pub fn complete(&mut self, result: String) {
        self.status = TaskStatus::Completed;
        self.result = Some(result);
        self.completed_at = Some(chrono::Utc::now());
    }

    /// Mark task as failed with error
    pub fn fail(&mut self, error: String) {
        self.status = TaskStatus::Failed;
        self.error = Some(error);
        self.completed_at = Some(chrono::Utc::now());
    }

    /// Mark task as cancelled
    pub fn cancel(&mut self) {
        self.status = TaskStatus::Cancelled;
        self.completed_at = Some(chrono::Utc::now());
    }
}

/// State for tracking active tasks
#[derive(Debug, Default)]
pub struct TaskState {
    /// Active tasks by ID
    pub tasks: std::collections::HashMap<String, TaskInstance>,
}

impl TaskState {
    /// Create new task state
    #[must_use]
    pub fn new() -> Self {
        Self { tasks: std::collections::HashMap::new() }
    }

    /// Add a new task
    pub fn add_task(&mut self, task: TaskInstance) -> String {
        let id = task.id.clone();
        self.tasks.insert(id.clone(), task);
        id
    }

    /// Get a task by ID
    #[must_use]
    pub fn get_task(&self, id: &str) -> Option<&TaskInstance> {
        self.tasks.get(id)
    }

    /// Get a mutable task by ID
    pub fn get_task_mut(&mut self, id: &str) -> Option<&mut TaskInstance> {
        self.tasks.get_mut(id)
    }

    /// Get all running tasks
    #[must_use]
    pub fn running_tasks(&self) -> Vec<&TaskInstance> {
        self.tasks.values().filter(|t| t.status == TaskStatus::Running).collect()
    }

    /// Get all tasks
    #[must_use]
    pub fn all_tasks(&self) -> Vec<&TaskInstance> {
        self.tasks.values().collect()
    }

    /// Remove completed/failed tasks older than the given duration
    #[allow(clippy::cast_possible_wrap)]
    pub fn cleanup_old_tasks(&mut self, max_age: std::time::Duration) {
        let now = chrono::Utc::now();
        self.tasks.retain(|_, task| {
            task.completed_at.is_none_or(|completed_at| {
                let age = now.signed_duration_since(completed_at);
                age.num_seconds() < max_age.as_secs() as i64
            })
        });
    }
}

/// Callback for executing sub-agent tasks
///
/// This trait allows the `TaskTool` to delegate actual sub-agent execution
/// to the agent layer, avoiding circular dependencies.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    /// Execute a task with a sub-agent
    async fn execute_task(&self, task: &TaskInstance) -> std::result::Result<String, String>;

    /// Execute task and return structured metrics when available.
    async fn execute_task_report(
        &self,
        task: &TaskInstance,
    ) -> std::result::Result<TaskExecutionReport, TaskExecutionError> {
        self.execute_task(task)
            .await
            .map(TaskExecutionReport::from_output)
            .map_err(TaskExecutionError::from)
    }

    /// Execute task with cancellation support.
    ///
    /// The default implementation keeps backward compatibility and ignores the
    /// cancellation token.
    async fn execute_task_report_with_cancel(
        &self,
        task: &TaskInstance,
        _cancellation: CancellationToken,
    ) -> std::result::Result<TaskExecutionReport, TaskExecutionError> {
        self.execute_task_report(task).await
    }
}

/// Structured execution report returned by a sub-agent run.
#[derive(Debug, Clone)]
pub struct TaskExecutionReport {
    /// Final textual output synthesized by the sub-agent.
    pub output: String,
    /// Total token usage observed while executing the task.
    pub tokens_used: usize,
    /// Number of tool calls issued by the sub-agent.
    pub tool_calls: usize,
}

impl TaskExecutionReport {
    /// Build a report from plain output when metrics are unavailable.
    #[must_use]
    pub const fn from_output(output: String) -> Self {
        Self { output, tokens_used: 0, tool_calls: 0 }
    }
}

/// Structured execution error with partial metrics.
#[derive(Debug, Clone)]
pub struct TaskExecutionError {
    /// User-facing error text.
    pub message: String,
    /// Total token usage observed before failure.
    pub tokens_used: usize,
    /// Number of tool calls issued before failure.
    pub tool_calls: usize,
}

impl TaskExecutionError {
    /// Create an execution error with optional partial metrics.
    #[must_use]
    pub const fn new(message: String, tokens_used: usize, tool_calls: usize) -> Self {
        Self { message, tokens_used, tool_calls }
    }
}

impl From<String> for TaskExecutionError {
    fn from(value: String) -> Self {
        Self::new(value, 0, 0)
    }
}

/// Default task executor that returns a placeholder
/// (Real implementation should be provided by forge-agent)
pub struct MockTaskExecutor;

#[async_trait]
impl TaskExecutor for MockTaskExecutor {
    async fn execute_task(&self, task: &TaskInstance) -> std::result::Result<String, String> {
        // In real implementation, this would spawn a sub-agent
        // For now, return a placeholder response
        Ok(format!(
            "[Task '{}' would be executed by {} sub-agent]\n\nPrompt: {}",
            task.description,
            task.subagent_type.display_name(),
            task.prompt
        ))
    }
}

/// Task tool for launching sub-agents
pub struct TaskTool {
    /// Shared task state
    state: Arc<RwLock<TaskState>>,
    /// Task executor (provided by agent layer)
    executor: Arc<dyn TaskExecutor>,
    /// Background task manager (optional, for background execution)
    background_manager: Option<Arc<crate::background::BackgroundTaskManager>>,
    /// Working directory for background tasks
    working_dir: std::path::PathBuf,
    /// Maximum concurrent subagent tasks
    max_concurrent_subagents: usize,
}

impl TaskTool {
    /// Create a new `TaskTool` with the given executor
    #[must_use]
    pub fn new(executor: Arc<dyn TaskExecutor>) -> Self {
        Self {
            state: Arc::new(RwLock::new(TaskState::new())),
            executor,
            background_manager: None,
            working_dir: std::env::current_dir().unwrap_or_default(),
            max_concurrent_subagents: 5,
        }
    }

    /// Create a new `TaskTool` with mock executor (for testing)
    #[must_use]
    pub fn with_mock_executor() -> Self {
        Self::new(Arc::new(MockTaskExecutor))
    }

    /// Create a new `TaskTool` with shared state
    #[must_use]
    pub fn with_state(state: Arc<RwLock<TaskState>>, executor: Arc<dyn TaskExecutor>) -> Self {
        Self {
            state,
            executor,
            background_manager: None,
            working_dir: std::env::current_dir().unwrap_or_default(),
            max_concurrent_subagents: 5,
        }
    }

    /// Create a new `TaskTool` with full configuration
    #[must_use]
    pub fn with_full_config(
        state: Arc<RwLock<TaskState>>,
        executor: Arc<dyn TaskExecutor>,
        background_manager: Option<Arc<crate::background::BackgroundTaskManager>>,
        working_dir: std::path::PathBuf,
        max_concurrent_subagents: usize,
    ) -> Self {
        Self { state, executor, background_manager, working_dir, max_concurrent_subagents }
    }

    /// Get the shared state
    #[must_use]
    pub fn state(&self) -> Arc<RwLock<TaskState>> {
        self.state.clone()
    }

    /// Handle resume of a previous task
    #[allow(clippy::too_many_lines)]
    async fn handle_resume(
        &self,
        resume_id: &str,
        _params: &Value,
    ) -> std::result::Result<ToolOutput, ToolError> {
        // Look up the task in state
        let task_opt = {
            let state = self.state.read().await;
            state.get_task(resume_id).cloned()
        };

        let Some(task) = task_opt else {
            return Ok(ToolOutput::error(format!(
                "Task not found: {resume_id}. Cannot resume a task that doesn't exist."
            )));
        };

        // Check if task is still running
        if task.status == TaskStatus::Running {
            return Ok(ToolOutput::error(format!(
                "Task {resume_id} is still running. Cannot resume a running task."
            )));
        }

        // Check if task was completed successfully
        if task.status == TaskStatus::Completed {
            if let Some(ref result) = task.result {
                return Ok(ToolOutput::success(format!(
                    "## Resumed Task: {}\n\n### Previous Result\n\n{result}\n\n---\nAgent ID: {resume_id}",
                    task.description
                )));
            }
        }

        // Task failed or was cancelled - re-execute with same parameters
        let mut new_task = TaskInstance::with_options(
            task.description.clone(),
            task.prompt.clone(),
            task.subagent_type,
            task.model_tier,
            task.max_turns,
            task.run_in_background,
            task.nesting_depth, // Preserve original nesting depth
        );
        new_task.id = resume_id.to_string(); // Keep the same ID
        new_task.start();

        // Update state
        {
            let mut state = self.state.write().await;
            state.tasks.insert(resume_id.to_string(), new_task.clone());
        }

        // Preserve background execution semantics if the original task was background
        if task.run_in_background {
            if let Some(ref bg_manager) = self.background_manager {
                let executor = self.executor.clone();
                let task_for_bg = new_task.clone();
                let state = self.state.clone();
                let task_id_for_bg = resume_id.to_string();
                let description = new_task.description.clone();

                let executor_future = async move {
                    let result = executor.execute_task(&task_for_bg).await;

                    // Update task state when done
                    {
                        let mut state_guard = state.write().await;
                        if let Some(t) = state_guard.get_task_mut(&task_id_for_bg) {
                            match &result {
                                Ok(output) => t.complete(output.clone()),
                                Err(error) => t.fail(error.clone()),
                            }
                        }
                    }

                    result
                };

                match bg_manager
                    .spawn_subagent(
                        resume_id.to_string(),
                        &description,
                        &new_task.prompt,
                        &self.working_dir,
                        self.max_concurrent_subagents,
                        executor_future,
                    )
                    .await
                {
                    Ok(_) => {
                        return Ok(ToolOutput::success(format!(
                            "Resumed background agent: {description}\nAgent ID: {resume_id}\n\n\
                             Use task_output tool to check progress."
                        )));
                    }
                    Err(e) => {
                        // Update task state to failed
                        {
                            let mut state = self.state.write().await;
                            if let Some(t) = state.get_task_mut(resume_id) {
                                t.fail(e.clone());
                            }
                        }
                        return Ok(ToolOutput::error(format!(
                            "Failed to resume background agent: {e}"
                        )));
                    }
                }
            }
            // No background manager available, fall back to sync with warning
            tracing::warn!(
                "Background execution requested for resume but no background manager available"
            );
        }

        // Execute the task synchronously
        let result = self.executor.execute_task(&new_task).await;

        // Update task state
        {
            let mut state = self.state.write().await;
            if let Some(t) = state.get_task_mut(resume_id) {
                match &result {
                    Ok(output) => t.complete(output.clone()),
                    Err(error) => t.fail(error.clone()),
                }
            }
        }

        // Return result
        match result {
            Ok(output) => Ok(ToolOutput::success(format!(
                "## Resumed Task: {}\n\n### Sub-agent Report\n\n{output}\n\n---\nAgent ID: {resume_id}",
                new_task.description
            ))),
            Err(error) => Ok(ToolOutput::error(format!(
                "Resumed task '{}' failed: {error}\n\nAgent ID: {resume_id}",
                new_task.description
            ))),
        }
    }
}

#[async_trait]
#[allow(clippy::too_many_lines, clippy::unnecessary_literal_bound)]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("task", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A short (3-5 word) description of the task"
                },
                "prompt": {
                    "type": "string",
                    "description": "Detailed instructions for the sub-agent to perform"
                },
                "subagent_type": {
                    "type": "string",
                    "enum": ["general-purpose", "explore", "plan", "research", "writer", "data-analyst"],
                    "description": "The type of specialized sub-agent to use"
                },
                "model": {
                    "type": "string",
                    "enum": ["fast", "default", "powerful"],
                    "description": "Model tier: fast (haiku-class), default (sonnet-class), powerful (opus-class)"
                },
                "max_turns": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Maximum agent turns before stopping (overrides default for agent type)"
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Run agent in background, returns task_id immediately"
                },
                "resume": {
                    "type": "string",
                    "description": "Agent ID to resume from previous execution. When provided, description/prompt/subagent_type are not required."
                }
            },
            "oneOf": [
                {
                    "required": ["resume"]
                },
                {
                    "required": ["description", "prompt", "subagent_type"]
                }
            ]
        })
    }

    fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
        ConfirmationLevel::None
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        // Check for resume parameter first
        if let Some(resume_id) = params.get("resume").and_then(|v| v.as_str()) {
            return self.handle_resume(resume_id, &params).await;
        }

        let description = crate::required_str(&params, "description")?.to_string();
        let prompt = crate::required_str(&params, "prompt")?.to_string();
        let subagent_type_str = crate::required_str(&params, "subagent_type")?;

        let subagent_type = SubAgentType::from_str(subagent_type_str).ok_or_else(|| {
            ToolError::InvalidParams(format!(
                "Invalid subagent_type '{subagent_type_str}'. Valid types: general-purpose, explore, plan, research, writer, data-analyst"
            ))
        })?;

        // Parse optional model tier
        let model_tier = params
            .get("model")
            .and_then(|v| v.as_str())
            .map_or(ModelTier::Default, |model_str| {
                ModelTier::from_str(model_str).unwrap_or(ModelTier::Default)
            });

        // Parse optional max_turns
        #[allow(clippy::cast_possible_truncation)]
        let max_turns = params.get("max_turns").and_then(Value::as_u64).map(|v| v as usize);

        // Parse optional run_in_background
        let run_in_background =
            params.get("run_in_background").and_then(Value::as_bool).unwrap_or(false);

        // Get current nesting depth from context
        let nesting_depth = ctx.subagent_nesting_depth();

        // Create task instance with all options including nesting depth
        let mut task = TaskInstance::with_options(
            description.clone(),
            prompt,
            subagent_type,
            model_tier,
            max_turns,
            run_in_background,
            nesting_depth,
        );
        task.start();

        // Add to state
        let task_id = {
            let mut state = self.state.write().await;
            state.add_task(task.clone())
        };

        // Handle background execution
        if run_in_background {
            if let Some(ref bg_manager) = self.background_manager {
                let executor = self.executor.clone();
                let task_for_bg = task.clone();
                let state = self.state.clone();
                let task_id_for_bg = task_id.clone();

                let executor_future = async move {
                    let result = executor.execute_task(&task_for_bg).await;

                    {
                        let mut state_guard = state.write().await;
                        if let Some(t) = state_guard.get_task_mut(&task_id_for_bg) {
                            match &result {
                                Ok(output) => t.complete(output.clone()),
                                Err(error) => t.fail(error.clone()),
                            }
                        }
                    }

                    result
                };

                match bg_manager
                    .spawn_subagent(
                        task_id.clone(),
                        &description,
                        &task.prompt,
                        &self.working_dir,
                        self.max_concurrent_subagents,
                        executor_future,
                    )
                    .await
                {
                    Ok(_) => {
                        return Ok(ToolOutput::success(format!(
                            "Started background agent: {description}\nAgent ID: {task_id}\n\n\
                             Use task_output tool to check progress."
                        )));
                    }
                    Err(e) => {
                        {
                            let mut state = self.state.write().await;
                            if let Some(t) = state.get_task_mut(&task_id) {
                                t.fail(e.clone());
                            }
                        }
                        return Ok(ToolOutput::error(format!(
                            "Failed to start background agent: {e}"
                        )));
                    }
                }
            }
            tracing::warn!(
                "Background execution requested but no background manager available"
            );
        }

        // Execute the task synchronously
        let result = self.executor.execute_task(&task).await;

        // Update task state
        {
            let mut state = self.state.write().await;
            if let Some(task) = state.get_task_mut(&task_id) {
                match &result {
                    Ok(output) => task.complete(output.clone()),
                    Err(error) => task.fail(error.clone()),
                }
            }
        }

        // Return result with metadata
        match result {
            Ok(output) => Ok(ToolOutput::success(format!(
                "## Task: {description}\n\n### Sub-agent Report\n\n{output}\n\n---\nAgent ID: {task_id}"
            ))),
            Err(error) => Ok(ToolOutput::error(format!(
                "Task '{description}' failed: {error}\n\nAgent ID: {task_id}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    #[test]
    fn test_subagent_type_from_str() {
        assert_eq!(SubAgentType::from_str("general-purpose"), Some(SubAgentType::GeneralPurpose));
        assert_eq!(SubAgentType::from_str("explore"), Some(SubAgentType::Explore));
        assert_eq!(SubAgentType::from_str("plan"), Some(SubAgentType::Plan));
        assert_eq!(SubAgentType::from_str("research"), Some(SubAgentType::Research));
        assert_eq!(SubAgentType::from_str("writer"), Some(SubAgentType::Writer));
        assert_eq!(SubAgentType::from_str("data-analyst"), Some(SubAgentType::DataAnalyst));
        assert_eq!(SubAgentType::from_str("analyst"), Some(SubAgentType::DataAnalyst));
        assert_eq!(SubAgentType::from_str("invalid"), None);
    }

    #[test]
    fn test_subagent_type_display() {
        assert_eq!(SubAgentType::GeneralPurpose.to_string(), "general-purpose");
        assert_eq!(SubAgentType::Explore.to_string(), "explore");
        assert_eq!(SubAgentType::Plan.to_string(), "plan");
        assert_eq!(SubAgentType::Research.to_string(), "research");
        assert_eq!(SubAgentType::Writer.to_string(), "writer");
        assert_eq!(SubAgentType::DataAnalyst.to_string(), "data-analyst");
    }

    #[test]
    fn test_task_instance_lifecycle() {
        let mut task = TaskInstance::new(
            "Test task".to_string(),
            "Do something".to_string(),
            SubAgentType::Explore,
        );

        assert_eq!(task.status, TaskStatus::Pending);

        task.start();
        assert_eq!(task.status, TaskStatus::Running);

        task.complete("Done!".to_string());
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task.result, Some("Done!".to_string()));
        assert!(task.completed_at.is_some());
    }

    #[test]
    fn test_task_instance_failure() {
        let mut task = TaskInstance::new(
            "Test task".to_string(),
            "Do something".to_string(),
            SubAgentType::Plan,
        );

        task.start();
        task.fail("Something went wrong".to_string());

        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(task.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_task_state() {
        let mut state = TaskState::new();

        let task1 =
            TaskInstance::new("Task 1".to_string(), "Prompt 1".to_string(), SubAgentType::Explore);
        let task2 =
            TaskInstance::new("Task 2".to_string(), "Prompt 2".to_string(), SubAgentType::Plan);

        let id1 = state.add_task(task1);
        let id2 = state.add_task(task2);

        assert!(state.get_task(&id1).is_some());
        assert!(state.get_task(&id2).is_some());
        assert_eq!(state.all_tasks().len(), 2);
    }

    #[test]
    fn test_task_state_running_tasks() {
        let mut state = TaskState::new();

        let mut task1 =
            TaskInstance::new("Task 1".to_string(), "Prompt 1".to_string(), SubAgentType::Explore);
        task1.start();

        let task2 =
            TaskInstance::new("Task 2".to_string(), "Prompt 2".to_string(), SubAgentType::Plan);

        state.add_task(task1);
        state.add_task(task2);

        let running = state.running_tasks();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].description, "Task 1");
    }

    #[tokio::test]
    async fn test_task_tool_execute() {
        let tool = TaskTool::with_mock_executor();

        let params = json!({
            "description": "Find all Rust files",
            "prompt": "Search for all .rs files in the project",
            "subagent_type": "explore"
        });

        let ctx = ToolContext::default();
        let result = tool.execute(params, &ctx).await.expect("execute should succeed");

        assert!(!result.is_error);
        assert!(result.content.contains("Find all Rust files"));
        assert!(result.content.contains("Explore"));
    }

    #[tokio::test]
    async fn test_task_tool_invalid_params() {
        let tool = TaskTool::with_mock_executor();
        let ctx = ToolContext::default();

        // Missing description
        let params = json!({
            "prompt": "Do something",
            "subagent_type": "explore"
        });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());

        // Invalid subagent_type
        let params = json!({
            "description": "Test",
            "prompt": "Do something",
            "subagent_type": "invalid"
        });
        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_task_tool_state_tracking() {
        let tool = TaskTool::with_mock_executor();

        let params = json!({
            "description": "Test task",
            "prompt": "Do something",
            "subagent_type": "general-purpose"
        });

        let ctx = ToolContext::default();
        let _ = tool.execute(params, &ctx).await.expect("execute should succeed");

        let state = tool.state.read().await;
        assert_eq!(state.all_tasks().len(), 1);

        let task = state.all_tasks()[0];
        assert_eq!(task.status, TaskStatus::Completed);
    }

    #[test]
    fn test_model_tier_from_str() {
        assert_eq!(ModelTier::from_str("fast"), Some(ModelTier::Fast));
        assert_eq!(ModelTier::from_str("haiku"), Some(ModelTier::Fast));
        assert_eq!(ModelTier::from_str("default"), Some(ModelTier::Default));
        assert_eq!(ModelTier::from_str("sonnet"), Some(ModelTier::Default));
        assert_eq!(ModelTier::from_str("powerful"), Some(ModelTier::Powerful));
        assert_eq!(ModelTier::from_str("opus"), Some(ModelTier::Powerful));
        assert_eq!(ModelTier::from_str("invalid"), None);
    }

    #[test]
    fn test_model_tier_display() {
        assert_eq!(ModelTier::Fast.to_string(), "fast");
        assert_eq!(ModelTier::Default.to_string(), "default");
        assert_eq!(ModelTier::Powerful.to_string(), "powerful");
    }

    #[test]
    fn test_task_instance_with_options() {
        let task = TaskInstance::with_options(
            "Test task".to_string(),
            "Do something".to_string(),
            SubAgentType::Explore,
            ModelTier::Fast,
            Some(10),
            true,
            0,
        );

        assert_eq!(task.description, "Test task");
        assert_eq!(task.prompt, "Do something");
        assert_eq!(task.subagent_type, SubAgentType::Explore);
        assert_eq!(task.model_tier, ModelTier::Fast);
        assert_eq!(task.max_turns, Some(10));
        assert!(task.run_in_background);
        assert_eq!(task.nesting_depth, 0);
        assert_eq!(task.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn test_task_tool_with_model_tier() {
        let tool = TaskTool::with_mock_executor();

        let params = json!({
            "description": "Fast task",
            "prompt": "Do something quickly",
            "subagent_type": "explore",
            "model": "fast"
        });

        let ctx = ToolContext::default();
        let result = tool.execute(params, &ctx).await.expect("execute should succeed");

        assert!(!result.is_error);

        let state = tool.state.read().await;
        let task = state.all_tasks()[0];
        assert_eq!(task.model_tier, ModelTier::Fast);
    }

    #[tokio::test]
    async fn test_task_tool_with_max_turns() {
        let tool = TaskTool::with_mock_executor();

        let params = json!({
            "description": "Limited task",
            "prompt": "Do something with limits",
            "subagent_type": "explore",
            "max_turns": 5
        });

        let ctx = ToolContext::default();
        let result = tool.execute(params, &ctx).await.expect("execute should succeed");

        assert!(!result.is_error);

        let state = tool.state.read().await;
        let task = state.all_tasks()[0];
        assert_eq!(task.max_turns, Some(5));
    }

    #[tokio::test]
    async fn test_task_inherits_nesting_depth_from_context() {
        let tool = TaskTool::with_mock_executor();

        // Simulate calling from a sub-agent (nesting_depth = 1)
        let mut ctx = ToolContext::default();
        ctx.subagent_nesting_depth = 1;

        let params = json!({
            "description": "Nested task",
            "prompt": "Do something",
            "subagent_type": "explore"
        });

        let _ = tool.execute(params, &ctx).await.expect("execute should succeed");

        let state = tool.state.read().await;
        let task = state.all_tasks()[0];
        assert_eq!(task.nesting_depth, 1);
    }
}
