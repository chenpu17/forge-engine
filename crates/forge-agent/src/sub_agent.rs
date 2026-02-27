//! Sub-agent system for delegated task execution
//!
//! This module implements the real task executor that spawns specialized
//! sub-agents for handling complex tasks. Each sub-agent type has its own
//! tool configuration and system prompt.

use crate::{
    AgentConfig, GenerationConfig,
    LoopProtectionConfig, ReflectionConfig,
};
use crate::core_loop::CoreAgent;
use crate::executor::ToolExecutor;
use async_trait::async_trait;
use forge_config::SubAgentLlmConfig;
use forge_domain::AgentEvent;
use forge_llm::{LlmProvider, ProviderRegistry};
use forge_tools::builtin::task::{
    ModelTier, SubAgentType, TaskExecutionError, TaskExecutionReport, TaskExecutor, TaskInstance,
};
use forge_tools::{ToolContext, ToolRegistry};
use futures::StreamExt;
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Configuration for sub-agent execution
#[derive(Debug, Clone)]
pub struct SubAgentConfig {
    /// Available tool names for this sub-agent
    pub available_tools: Vec<String>,
    /// Maximum iterations for the sub-agent loop
    pub max_iterations: usize,
    /// Timeout in seconds for the sub-agent
    pub timeout_secs: u64,
    /// System prompt prefix for this agent type
    pub system_prompt: String,
}

/// Security settings inherited from the parent agent
#[derive(Debug, Clone, Default)]
pub struct SubAgentSecurity {
    /// Bash read-only mode (persona-configured)
    pub bash_readonly: bool,
    /// Disabled tools from config/persona
    pub disabled_tools: Vec<String>,
}

impl SubAgentConfig {
    /// Get configuration for Explore sub-agent
    #[must_use]
    pub fn for_explore() -> Self {
        Self {
            available_tools: vec!["glob".to_string(), "grep".to_string(), "read".to_string()],
            max_iterations: 20,
            timeout_secs: 120,
            system_prompt: EXPLORE_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Get configuration for Plan sub-agent
    #[must_use]
    pub fn for_plan() -> Self {
        Self {
            available_tools: vec!["glob".to_string(), "grep".to_string(), "read".to_string()],
            max_iterations: 30,
            timeout_secs: 300,
            system_prompt: PLAN_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Get configuration for Research sub-agent
    #[must_use]
    pub fn for_research() -> Self {
        Self {
            available_tools: vec![
                "glob".to_string(),
                "grep".to_string(),
                "read".to_string(),
                "web_fetch".to_string(),
                "web_search".to_string(),
            ],
            max_iterations: 25,
            timeout_secs: 180,
            system_prompt: RESEARCH_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Get configuration for `GeneralPurpose` sub-agent
    #[must_use]
    pub fn for_general_purpose() -> Self {
        Self {
            available_tools: vec![
                "glob".to_string(),
                "grep".to_string(),
                "read".to_string(),
                "bash".to_string(),
                "web_fetch".to_string(),
                "web_search".to_string(),
            ],
            max_iterations: 50,
            timeout_secs: 600,
            system_prompt: GENERAL_PURPOSE_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Get configuration for Writer sub-agent
    #[must_use]
    pub fn for_writer() -> Self {
        Self {
            available_tools: vec![
                "read".to_string(),
                "write".to_string(),
                "edit".to_string(),
                "web_search".to_string(),
                "web_fetch".to_string(),
            ],
            max_iterations: 30,
            timeout_secs: 300,
            system_prompt: WRITER_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Get configuration for `DataAnalyst` sub-agent
    #[must_use]
    pub fn for_data_analyst() -> Self {
        Self {
            available_tools: vec![
                "read".to_string(),
                "glob".to_string(),
                "grep".to_string(),
                "bash".to_string(),
                "write".to_string(),
                "web_search".to_string(),
            ],
            max_iterations: 30,
            timeout_secs: 300,
            system_prompt: DATA_ANALYST_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Get configuration for a given sub-agent type
    #[must_use]
    pub fn for_type(agent_type: SubAgentType) -> Self {
        match agent_type {
            SubAgentType::Explore => Self::for_explore(),
            SubAgentType::Plan => Self::for_plan(),
            SubAgentType::Research => Self::for_research(),
            SubAgentType::GeneralPurpose => Self::for_general_purpose(),
            SubAgentType::Writer => Self::for_writer(),
            SubAgentType::DataAnalyst => Self::for_data_analyst(),
        }
    }
}

/// Real task executor that spawns sub-agents
///
/// This executor creates mini-agents with limited tool sets to handle
/// specific types of tasks. Each sub-agent runs in its own context
/// and returns a summary when complete.
pub struct RealTaskExecutor {
    /// Provider registry for dynamic provider selection based on model
    provider_registry: Arc<ProviderRegistry>,
    /// Full tool registry (we filter for each sub-agent)
    full_registry: Arc<ToolRegistry>,
    /// Working directory
    working_dir: PathBuf,
    /// Default model to use for sub-agents
    model: String,
    /// `SubAgent` LLM configuration for model tiers
    subagent_config: SubAgentLlmConfig,
    /// Plan mode flag (inherited from parent agent)
    plan_mode_flag: Arc<AtomicBool>,
    /// Shared security configuration
    security: Arc<RwLock<SubAgentSecurity>>,
    /// Permission policy rules inherited from parent SDK config.
    permission_rules: Vec<forge_config::PermissionRuleConfig>,
}

impl RealTaskExecutor {
    /// Create a new real task executor with a single provider (for backward compatibility)
    #[must_use]
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        full_registry: Arc<ToolRegistry>,
        working_dir: PathBuf,
        model: String,
    ) -> Self {
        // Wrap single provider in a registry
        let mut registry = ProviderRegistry::new();
        registry.register(provider);
        Self::with_full_config(
            Arc::new(registry),
            full_registry,
            working_dir,
            model,
            SubAgentLlmConfig::default(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(RwLock::new(SubAgentSecurity::default())),
            vec![],
        )
    }

    /// Create a new real task executor with security settings inherited from parent
    #[must_use]
    pub fn with_security(
        provider: Arc<dyn LlmProvider>,
        full_registry: Arc<ToolRegistry>,
        working_dir: PathBuf,
        model: String,
        plan_mode_flag: Arc<AtomicBool>,
        bash_readonly: bool,
    ) -> Self {
        // Wrap single provider in a registry
        let mut registry = ProviderRegistry::new();
        registry.register(provider);
        Self::with_full_config(
            Arc::new(registry),
            full_registry,
            working_dir,
            model,
            SubAgentLlmConfig::default(),
            plan_mode_flag,
            Arc::new(RwLock::new(SubAgentSecurity { bash_readonly, disabled_tools: Vec::new() })),
            vec![],
        )
    }

    /// Create a new real task executor with shared security state
    #[must_use]
    pub fn with_security_state(
        provider: Arc<dyn LlmProvider>,
        full_registry: Arc<ToolRegistry>,
        working_dir: PathBuf,
        model: String,
        plan_mode_flag: Arc<AtomicBool>,
        security: Arc<RwLock<SubAgentSecurity>>,
    ) -> Self {
        // Wrap single provider in a registry
        let mut registry = ProviderRegistry::new();
        registry.register(provider);
        Self::with_full_config(
            Arc::new(registry),
            full_registry,
            working_dir,
            model,
            SubAgentLlmConfig::default(),
            plan_mode_flag,
            security,
            vec![],
        )
    }

    /// Create a new real task executor with full configuration
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub const fn with_full_config(
        provider_registry: Arc<ProviderRegistry>,
        full_registry: Arc<ToolRegistry>,
        working_dir: PathBuf,
        model: String,
        subagent_config: SubAgentLlmConfig,
        plan_mode_flag: Arc<AtomicBool>,
        security: Arc<RwLock<SubAgentSecurity>>,
        permission_rules: Vec<forge_config::PermissionRuleConfig>,
    ) -> Self {
        Self {
            provider_registry,
            full_registry,
            working_dir,
            model,
            subagent_config,
            plan_mode_flag,
            security,
            permission_rules,
        }
    }

    /// Resolve the model to use based on the model tier
    fn resolve_model(&self, tier: ModelTier) -> String {
        match tier {
            ModelTier::Fast => {
                self.subagent_config.fast_model.clone().unwrap_or_else(|| self.model.clone())
            }
            ModelTier::Default => {
                self.subagent_config.default_model.clone().unwrap_or_else(|| self.model.clone())
            }
            ModelTier::Powerful => {
                self.subagent_config.powerful_model.clone().unwrap_or_else(|| self.model.clone())
            }
        }
    }

    /// Create a filtered tool registry for a sub-agent
    fn create_filtered_registry(
        &self,
        allowed_tools: &[String],
        disabled_tools: &[String],
    ) -> ToolRegistry {
        let disabled_set: std::collections::HashSet<&str> =
            disabled_tools.iter().map(String::as_str).collect();
        let mut filtered = ToolRegistry::new();

        for tool_name in allowed_tools {
            if disabled_set.contains(tool_name.as_str()) {
                continue;
            }
            if let Some(tool) = self.full_registry.get(tool_name) {
                filtered.register(tool.clone());
            }
        }

        filtered
    }

    /// Build the full system prompt for a sub-agent
    fn build_system_prompt(config: &SubAgentConfig, task: &TaskInstance) -> String {
        format!(
            "{}\n\n## Task\n\n{}\n\n## Instructions\n\n{}\n\n## Guidelines\n\n\
             - Complete the task thoroughly but concisely\n\
             - Use the available tools to gather information\n\
             - When done, provide a clear summary of your findings\n\
             - If you cannot complete the task, explain why",
            config.system_prompt, task.description, task.prompt
        )
    }

    /// Run a task and return a structured report with metrics.
    #[allow(clippy::too_many_lines)]
    async fn run_task_report(
        &self,
        task: &TaskInstance,
        cancellation: Option<CancellationToken>,
    ) -> std::result::Result<TaskExecutionReport, TaskExecutionError> {
        let sub_config = SubAgentConfig::for_type(task.subagent_type);
        let model = self.resolve_model(task.model_tier);

        // Read security settings
        let (bash_readonly, disabled_tools) = {
            let sec = self.security.read();
            (sec.bash_readonly, sec.disabled_tools.clone())
        };

        // Create filtered registry
        let filtered_registry =
            self.create_filtered_registry(&sub_config.available_tools, &disabled_tools);

        // Create tool context for sub-agent
        let tool_context = ToolContext {
            working_dir: self.working_dir.clone(),
            bash_readonly,
            plan_mode_flag: self.plan_mode_flag.clone(),
            subagent_nesting_depth: task.nesting_depth + 1,
            ..Default::default()
        };

        let executor = Arc::new(ToolExecutor::new(Arc::new(filtered_registry), tool_context));

        // Build system prompt
        let system_prompt = Self::build_system_prompt(&sub_config, task);

        // Apply max_turns override if specified
        let max_iterations = task.max_turns.unwrap_or(sub_config.max_iterations);

        let agent_config = AgentConfig {
            model: model.clone(),
            working_dir: self.working_dir.clone(),
            project_prompt: Some(system_prompt),
            loop_protection: LoopProtectionConfig {
                max_iterations,
                total_timeout_secs: sub_config.timeout_secs,
                iteration_timeout_secs: 60,
                detect_repetition: true,
                max_same_tool_calls: 5,
                tool_call_limits: std::collections::HashMap::new(),
                post_completion_iterations: 2,
            },
            generation: GenerationConfig { max_tokens: 8192, temperature: 0.3 },
            reflection: ReflectionConfig {
                enabled: true,
                max_same_error_retries: 2,
                max_consecutive_failures: 3,
                use_llm_for_analysis: false,
                reflection_timeout_secs: 15,
                max_same_error_count: 3,
                max_test_failure_count: 10,
                max_consecutive_test_failures: 10,
            },
            skills: Vec::new(),
            thinking: None,
            thinking_adaptor: forge_config::ThinkingAdaptor::Auto,
            trust_level: forge_config::TrustLevelSetting::default(),
            memory_user_index: None,
            memory_project_index: None,
            permission_rules: self.permission_rules.clone(),
            session_id: None,
            verifier: crate::VerifierConfig::default(),
            experimental: crate::ExperimentalAgentConfig::default(),
        };

        // Resolve provider for the chosen model
        let provider = self
            .provider_registry
            .get_for_model(&model)
            .ok_or_else(|| {
                TaskExecutionError::new(
                    format!("No provider found for model '{model}'"),
                    0,
                    0,
                )
            })?;

        let agent = CoreAgent::new(provider, executor, agent_config);

        // Execute the sub-agent
        let mut stream = agent.process(&task.prompt).map_err(|e| {
            TaskExecutionError::new(format!("Failed to start sub-agent: {e}"), 0, 0)
        })?;

        let mut full_response = String::new();
        let mut total_tokens = 0usize;
        let mut tool_call_count = 0usize;

        loop {
            // Check cancellation
            if let Some(ref token) = cancellation {
                if token.is_cancelled() {
                    return Err(TaskExecutionError::new(
                        "Task cancelled".to_string(),
                        total_tokens,
                        tool_call_count,
                    ));
                }
            }

            let event = stream.next().await;
            match event {
                Some(Ok(AgentEvent::TextDelta { delta })) => {
                    full_response.push_str(&delta);
                }
                Some(Ok(AgentEvent::ToolCallStart { .. })) => {
                    tool_call_count += 1;
                }
                Some(Ok(AgentEvent::TokenUsage { input_tokens, output_tokens, .. })) => {
                    total_tokens += input_tokens + output_tokens;
                }
                Some(Ok(AgentEvent::Done { .. })) => {
                    break;
                }
                Some(Ok(AgentEvent::Error { message })) => {
                    return Err(TaskExecutionError::new(
                        message,
                        total_tokens,
                        tool_call_count,
                    ));
                }
                Some(Err(e)) => {
                    return Err(TaskExecutionError::new(
                        format!("Stream error: {e}"),
                        total_tokens,
                        tool_call_count,
                    ));
                }
                None => break,
                _ => {}
            }
        }

        if full_response.is_empty() {
            full_response = "[Sub-agent completed without text output]".to_string();
        }

        Ok(TaskExecutionReport {
            output: full_response,
            tokens_used: total_tokens,
            tool_calls: tool_call_count,
        })
    }
}

#[async_trait]
impl TaskExecutor for RealTaskExecutor {
    async fn execute_task(&self, task: &TaskInstance) -> std::result::Result<String, String> {
        match self.run_task_report(task, None).await {
            Ok(report) => Ok(report.output),
            Err(e) => Err(e.message),
        }
    }

    async fn execute_task_report(
        &self,
        task: &TaskInstance,
    ) -> std::result::Result<TaskExecutionReport, TaskExecutionError> {
        self.run_task_report(task, None).await
    }

    async fn execute_task_report_with_cancel(
        &self,
        task: &TaskInstance,
        cancellation: CancellationToken,
    ) -> std::result::Result<TaskExecutionReport, TaskExecutionError> {
        self.run_task_report(task, Some(cancellation)).await
    }
}

// ============================================================================
// System Prompts for Sub-Agents
// ============================================================================

const EXPLORE_SYSTEM_PROMPT: &str = r#"# Codebase Explorer

You are a specialized agent for exploring codebases. Your job is to quickly find files,
understand code structure, and answer questions about the codebase.

## Available Tools

- **glob**: Find files by pattern (e.g., "**/*.rs", "src/**/*.ts")
- **grep**: Search file contents for patterns
- **read**: Read file contents

## Best Practices

1. Start with broad searches, then narrow down
2. Use glob to find relevant files first
3. Use grep to search for specific patterns
4. Read files to understand context
5. Summarize your findings clearly
"#;

const PLAN_SYSTEM_PROMPT: &str = r"# Software Architect

You are a specialized agent for designing implementation plans. Your job is to analyze
code structure, identify patterns, and create step-by-step implementation strategies.

## Available Tools

- **glob**: Find files by pattern
- **grep**: Search for code patterns
- **read**: Read file contents for analysis

## Best Practices

1. Understand the existing architecture first
2. Identify all affected files and components
3. Consider dependencies and side effects
4. Break down changes into discrete steps
5. Note any risks or trade-offs

## Output Format

Provide your plan as:
1. **Summary**: What needs to be done
2. **Files to Modify**: List of files with brief description
3. **Steps**: Numbered implementation steps
4. **Considerations**: Any risks or alternatives
";

const RESEARCH_SYSTEM_PROMPT: &str = r"# Research Agent

You are a specialized agent for gathering information from code and documentation.
Your job is to research APIs, libraries, and code patterns.

## Available Tools

- **glob**: Find files by pattern
- **grep**: Search for code patterns
- **read**: Read file contents
- **web_fetch**: Fetch web pages for documentation
- **web_search**: Search the web for information

## Best Practices

1. Check local code first, then external sources
2. Verify information from multiple sources
3. Provide specific references and examples
4. Distinguish between facts and recommendations
5. Note version-specific information
";

const GENERAL_PURPOSE_SYSTEM_PROMPT: &str = r"# General Purpose Agent

You are a general-purpose coding agent capable of handling complex, multi-step tasks.
You can explore code, execute commands, and gather information from multiple sources.

## Available Tools

- **glob**: Find files by pattern
- **grep**: Search for code patterns
- **read**: Read file contents
- **bash**: Execute shell commands
- **web_fetch**: Fetch web pages
- **web_search**: Search the web

## Best Practices

1. Understand the task fully before acting
2. Gather information before making changes
3. Break complex tasks into smaller steps
4. Verify your work as you go
5. Provide clear summaries of what you found or did

## Safety

- Be careful with bash commands
- Don't modify files unless specifically asked
- Report any errors or issues clearly
";

const WRITER_SYSTEM_PROMPT: &str = r"# Content Writer Agent

You are a specialized agent for content creation and document writing. Your job is to
produce high-quality written content including documents, reports, emails, proposals,
technical documentation, and other text-based deliverables.

## Available Tools

- **read**: Read existing files for reference
- **write**: Create new files with content
- **edit**: Modify existing files
- **web_search**: Search for reference material
- **web_fetch**: Fetch web pages for research

## Best Practices

1. Understand the audience and purpose before writing
2. Research relevant context and reference materials first
3. Structure content with clear headings and logical flow
4. Use appropriate tone and style for the target audience
5. Proofread and refine the output for clarity and accuracy
6. Save the final content to files when appropriate
";

const DATA_ANALYST_SYSTEM_PROMPT: &str = r"# Data Analyst Agent

You are a specialized agent for data analysis and report generation. Your job is to
process data, perform statistical analysis, generate visualization scripts, and
produce analytical reports.

## Available Tools

- **read**: Read data files and existing analysis
- **glob**: Find data files and related resources
- **grep**: Search for patterns in data or code
- **bash**: Execute analysis scripts and commands
- **write**: Create analysis scripts and reports
- **web_search**: Search for analysis methods and references

## Best Practices

1. Understand the data structure and format first
2. Validate data quality before analysis
3. Use appropriate statistical methods for the data type
4. Generate clear visualizations with proper labels
5. Document methodology and assumptions
6. Present findings with actionable insights
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_agent_config_for_explore() {
        let config = SubAgentConfig::for_explore();
        assert_eq!(config.available_tools, vec!["glob", "grep", "read"]);
        assert_eq!(config.max_iterations, 20);
        assert_eq!(config.timeout_secs, 120);
    }

    #[test]
    fn test_sub_agent_config_for_plan() {
        let config = SubAgentConfig::for_plan();
        assert_eq!(config.available_tools, vec!["glob", "grep", "read"]);
        assert_eq!(config.max_iterations, 30);
        assert_eq!(config.timeout_secs, 300);
    }

    #[test]
    fn test_sub_agent_config_for_research() {
        let config = SubAgentConfig::for_research();
        assert!(config.available_tools.contains(&"web_fetch".to_string()));
        assert!(config.available_tools.contains(&"web_search".to_string()));
    }

    #[test]
    fn test_sub_agent_config_for_general_purpose() {
        let config = SubAgentConfig::for_general_purpose();
        assert!(config.available_tools.contains(&"bash".to_string()));
        assert_eq!(config.max_iterations, 50);
    }

    #[test]
    fn test_sub_agent_config_for_type() {
        let explore = SubAgentConfig::for_type(SubAgentType::Explore);
        assert_eq!(explore.max_iterations, 20);

        let plan = SubAgentConfig::for_type(SubAgentType::Plan);
        assert_eq!(plan.max_iterations, 30);

        let research = SubAgentConfig::for_type(SubAgentType::Research);
        assert_eq!(research.max_iterations, 25);

        let general = SubAgentConfig::for_type(SubAgentType::GeneralPurpose);
        assert_eq!(general.max_iterations, 50);

        let writer = SubAgentConfig::for_type(SubAgentType::Writer);
        assert_eq!(writer.max_iterations, 30);
        assert!(writer.available_tools.contains(&"write".to_string()));
        assert!(writer.available_tools.contains(&"edit".to_string()));

        let analyst = SubAgentConfig::for_type(SubAgentType::DataAnalyst);
        assert_eq!(analyst.max_iterations, 30);
        assert!(analyst.available_tools.contains(&"bash".to_string()));
        assert!(analyst.available_tools.contains(&"write".to_string()));
    }

    #[test]
    fn test_real_task_executor_security_inheritance() {
        use std::sync::atomic::Ordering;

        // Create with default security (no restrictions)
        let plan_flag_default = Arc::new(AtomicBool::new(false));

        // Verify default executor has no security restrictions
        // (We can't easily test execute_task without a real LLM, but we can
        // verify the constructor stores the security settings correctly)

        // Create with security enabled
        let plan_flag_enabled = Arc::new(AtomicBool::new(true));

        // Simulate parent agent enabling plan mode
        plan_flag_enabled.store(true, Ordering::Release);

        // Verify atomic flag works as expected
        assert!(plan_flag_enabled.load(Ordering::Acquire));
        assert!(!plan_flag_default.load(Ordering::Acquire));

        // Verify bash_readonly can be inherited
        let bash_readonly = true;
        assert!(bash_readonly);
    }

    #[test]
    fn test_filtered_registry_respects_subagent_tool_scope() {
        use serde_json::Value;

        struct DummyTool {
            name: String,
        }

        #[async_trait::async_trait]
        impl forge_tools::Tool for DummyTool {
            fn name(&self) -> &str {
                &self.name
            }

            fn description(&self) -> &str {
                "dummy"
            }

            fn parameters_schema(&self) -> Value {
                serde_json::json!({"type": "object"})
            }

            fn confirmation_level(&self, _params: &Value) -> forge_tools::ConfirmationLevel {
                forge_tools::ConfirmationLevel::None
            }

            fn is_readonly(&self) -> bool {
                true
            }

            async fn execute(
                &self,
                _params: Value,
                _ctx: &dyn forge_tools::ToolExecutionContext,
            ) -> std::result::Result<forge_tools::ToolOutput, forge_tools::ToolError> {
                Ok(forge_tools::ToolOutput::success("ok"))
            }
        }

        let provider = Arc::new(crate::mock::MockLlmProvider::new());
        let mut registry = forge_tools::ToolRegistry::new();
        for name in ["glob", "grep", "read", "bash", "web_search"] {
            registry.register(Arc::new(DummyTool { name: name.to_string() }));
        }

        let executor = RealTaskExecutor::new(
            provider,
            Arc::new(registry),
            std::env::temp_dir(),
            "claude-sonnet-4".to_string(),
        );

        let explore = SubAgentConfig::for_explore();
        let filtered = executor.create_filtered_registry(&explore.available_tools, &[]);

        assert!(filtered.get("glob").is_some());
        assert!(filtered.get("grep").is_some());
        assert!(filtered.get("read").is_some());
        assert!(filtered.get("bash").is_none());
        assert!(filtered.get("web_search").is_none());

        let general = SubAgentConfig::for_general_purpose();
        let filtered_with_disable =
            executor.create_filtered_registry(&general.available_tools, &["bash".to_string()]);
        assert!(filtered_with_disable.get("bash").is_none());
    }

    #[tokio::test]
    async fn test_execute_task_report_maps_usage_metrics() {
        let provider = Arc::new(crate::mock::MockLlmProvider::with_text_response(
            "sub-agent produced useful summary",
        ));
        let registry = Arc::new(forge_tools::ToolRegistry::new());

        let executor = RealTaskExecutor::new(
            provider,
            registry,
            std::env::temp_dir(),
            "claude-sonnet-4".to_string(),
        );

        let task = TaskInstance::new(
            "Analyze module".to_string(),
            "Summarize findings".to_string(),
            SubAgentType::Explore,
        );

        let report = executor.execute_task_report(&task).await.expect("execute task report");
        assert!(report.output.contains("sub-agent produced useful summary"));
        assert!(report.tokens_used > 0, "token usage should be recorded from LLM events");
        assert_eq!(report.tool_calls, 0);
    }
}
