//! Core agent loop implementation.
//!
//! Contains the `CoreAgent` struct and the main `run_agent_loop` function
//! that orchestrates LLM calls, tool execution, and state management.

use crate::{
    checkpoint::{
        build_context_fingerprint, format_runtime_tool_states, persist_runtime_checkpoint,
        GitCheckpointManager, RuntimeCheckpointStore, RuntimeResumeState,
    },
    cost_tracker::CostTracker,
    episodic_memory::EpisodicMemoryStore,
    executor::ToolExecutor,
    prepare::{run_optional_stage, run_pre_tool_stages, AgentLoopStage, PreToolStageOutput},
    reflector::{check_loop_protection, Reflector},
    tool_dispatch::{
        check_tool_repetition, dispatch_and_execute_tools, DispatchState, ToolDispatchOutput,
        MAX_REJECTED_ENTRIES, REJECTION_TTL,
    },
    trace_recorder::TraceRecorder,
    verifier::{VerifierPipeline, VerifierStats},
    AgentConfig, AgentError, ConfirmationHandler, Result,
};
use forge_domain::{AgentEvent, CostCheckResult};
use forge_llm::{
    ChatMessage, ChatRole, ContentBlock, InstrumentedProvider, LlmConfig, LlmProvider,
    MessageContent, RetryConfig,
};
use forge_prompt::{PromptContext, PromptManager};
use forge_tools::trust_permission::{PermissionConfig, TrustAwarePermissionManager, TrustLevel};
use futures::FutureExt;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Convert config-level trust setting to runtime trust level.
const fn trust_level_from_setting(setting: forge_config::TrustLevelSetting) -> TrustLevel {
    match setting {
        forge_config::TrustLevelSetting::Cautious => TrustLevel::Cautious,
        forge_config::TrustLevelSetting::Development => TrustLevel::Development,
        forge_config::TrustLevelSetting::Trusted => TrustLevel::Trusted,
        forge_config::TrustLevelSetting::Yolo => TrustLevel::Yolo,
    }
}

/// Stream of agent events.
pub type AgentEventStream = Pin<Box<dyn futures::Stream<Item = Result<AgentEvent>> + Send>>;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A message in the conversation history
#[derive(Debug, Clone)]
pub struct HistoryMessage {
    /// Message role
    pub role: HistoryRole,
    /// Message content
    pub content: String,
}

/// Role for history messages
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryRole {
    /// User message
    User,
    /// Assistant message
    Assistant,
    /// System message (e.g., compression summaries)
    System,
}

impl HistoryMessage {
    /// Create a user message
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: HistoryRole::User, content: content.into() }
    }

    /// Create an assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: HistoryRole::Assistant, content: content.into() }
    }

    /// Create a system message (for compression summaries, etc.)
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: HistoryRole::System, content: content.into() }
    }
}

// ---------------------------------------------------------------------------
// CoreAgent
// ---------------------------------------------------------------------------

/// Core Agent struct
pub struct CoreAgent {
    /// LLM provider
    provider: Arc<dyn LlmProvider>,
    /// Tool executor
    executor: Arc<ToolExecutor>,
    /// Agent configuration
    config: AgentConfig,
    /// Prompt manager
    prompt_manager: Arc<PromptManager>,
    /// Confirmation handler (optional)
    confirmation_handler: Option<Arc<dyn ConfirmationHandler>>,
    /// Permission manager for confirmation caching (Once level)
    permission_manager: Arc<Mutex<TrustAwarePermissionManager>>,
    /// Cancellation token (shared so `abort()` can cancel the current run)
    cancellation: Arc<Mutex<CancellationToken>>,
    /// Running state
    is_running: Arc<AtomicBool>,
}

impl CoreAgent {
    /// Create a new agent
    #[must_use]
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        executor: Arc<ToolExecutor>,
        config: AgentConfig,
    ) -> Self {
        Self::with_prompt_manager(provider, executor, config, Arc::new(PromptManager::new()))
    }

    /// Create a new agent with a custom prompt manager
    #[must_use]
    pub fn with_prompt_manager(
        provider: Arc<dyn LlmProvider>,
        executor: Arc<ToolExecutor>,
        config: AgentConfig,
        prompt_manager: Arc<PromptManager>,
    ) -> Self {
        // Create trust-aware permission manager with project root and trust level
        let mut permission_manager = TrustAwarePermissionManager::new(PermissionConfig::default());
        permission_manager.set_project_root(config.working_dir.clone());
        permission_manager.set_trust_level(trust_level_from_setting(config.trust_level));
        if !config.permission_rules.is_empty() {
            permission_manager.set_permission_rules(config.permission_rules.clone());
        }

        Self {
            provider,
            executor,
            config,
            prompt_manager,
            confirmation_handler: None,
            permission_manager: Arc::new(Mutex::new(permission_manager)),
            cancellation: Arc::new(Mutex::new(CancellationToken::new())),
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set the confirmation handler
    ///
    /// The confirmation handler is called when a tool requires user confirmation
    /// before execution. If not set, tools requiring confirmation will be skipped.
    #[must_use]
    pub fn with_confirmation_handler(mut self, handler: Arc<dyn ConfirmationHandler>) -> Self {
        self.confirmation_handler = Some(handler);
        self
    }

    /// Get the cancellation token for external abort control
    ///
    /// Returns a clone of the shared cancellation token mutex. The SDK can use this
    /// to cancel the running agent from outside.
    #[must_use]
    pub fn cancellation_token(&self) -> Arc<Mutex<CancellationToken>> {
        self.cancellation.clone()
    }

    /// Process a user query
    ///
    /// # Errors
    ///
    /// Returns an error if the agent is already running or if the agent loop fails.
    pub fn process(&self, query: &str) -> Result<AgentEventStream> {
        self.process_with_history(query, &[])
    }

    /// Process a user query with conversation history
    ///
    /// The history parameter contains previous conversation turns that should be
    /// included as context for the current query.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent is already running or if the agent loop fails.
    pub fn process_with_history(
        &self,
        query: &str,
        history: &[HistoryMessage],
    ) -> Result<AgentEventStream> {
        // Check if already running
        if self.is_running.swap(true, Ordering::SeqCst) {
            return Err(AgentError::SessionError("Agent is already running".to_string()));
        }

        // Create new cancellation token for this run and store it in the shared mutex
        let cancellation = CancellationToken::new();
        {
            let mut guard = self.cancellation.lock();
            *guard = cancellation.clone();
        }
        let is_running = self.is_running.clone();

        // Create event channel
        let (tx, rx) = mpsc::channel::<Result<AgentEvent>>(100);

        // Clone data for the spawned task
        let provider = self.provider.clone();
        let executor = self.executor.clone();
        let config = self.config.clone();
        let prompt_manager = self.prompt_manager.clone();
        let confirmation_handler = self.confirmation_handler.clone();
        let permission_manager = self.permission_manager.clone();
        let query = query.to_string();
        let history = history.to_vec();
        // Spawn the agent loop
        tokio::spawn(async move {
            let tx_panic = tx.clone();
            let fut = async {
                let result = Box::pin(run_agent_loop(
                    provider,
                    executor,
                    config,
                    prompt_manager,
                    confirmation_handler,
                    permission_manager,
                    query,
                    history,
                    tx.clone(),
                    cancellation,
                ))
                .await;

                // Handle any final error - propagate original error type
                if let Err(e) = result {
                    let _ = tx.send(Err(e)).await;
                }
            };

            // Catch panics in the agent loop to ensure a terminal event is always sent.
            // Without this, a panic silently drops `tx`, ending the stream without a
            // Done/Error/Cancelled event.
            match Box::pin(std::panic::AssertUnwindSafe(fut).catch_unwind()).await {
                Ok(()) => {}
                Err(panic_payload) => {
                    let msg = panic_payload.downcast_ref::<&str>().map_or_else(
                        || {
                            panic_payload.downcast_ref::<String>().map_or_else(
                                || "Agent loop panicked (unknown payload)".to_string(),
                                |s| format!("Agent loop panicked: {s}"),
                            )
                        },
                        |s| format!("Agent loop panicked: {s}"),
                    );
                    tracing::error!("{}", msg);
                    let _ = tx_panic.send(Ok(AgentEvent::Error { message: msg })).await;
                }
            }

            // Mark as not running
            is_running.store(false, Ordering::SeqCst);
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    /// Abort the current execution
    pub fn abort(&self) {
        // Cancel the current running token
        self.cancellation.lock().cancel();
    }

    /// Check if the agent is currently running
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// run_agent_loop — initialization
// ---------------------------------------------------------------------------

/// Run the agent loop
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
#[tracing::instrument(name = "agent_loop", skip_all, fields(query_len = initial_query.len()))]
async fn run_agent_loop(
    provider: Arc<dyn LlmProvider>,
    executor: Arc<ToolExecutor>,
    config: AgentConfig,
    prompt_manager: Arc<PromptManager>,
    confirmation_handler: Option<Arc<dyn ConfirmationHandler>>,
    permission_manager: Arc<Mutex<TrustAwarePermissionManager>>,
    initial_query: String,
    history: Vec<HistoryMessage>,
    tx: mpsc::Sender<Result<AgentEvent>>,
    cancellation: CancellationToken,
) -> Result<()> {
    // Wrap provider with retry/timeout middleware
    let iteration_timeout =
        std::time::Duration::from_secs(config.loop_protection.iteration_timeout_secs);
    let (retry_tx, mut retry_rx) = tokio::sync::mpsc::unbounded_channel();
    let provider: Arc<dyn LlmProvider> = Arc::new(
        InstrumentedProvider::new(provider)
            .with_retry(RetryConfig {
                max_retries: 10,
                initial_delay: std::time::Duration::from_millis(500),
                max_delay: std::time::Duration::from_secs(30),
                backoff_factor: 2.0,
            })
            .with_timeout(iteration_timeout)
            .with_retry_notifications(retry_tx),
    );

    // Spawn a lightweight task to forward retry notifications as AgentEvents
    let retry_event_tx = tx.clone();
    tokio::spawn(async move {
        while let Some(notif) = retry_rx.recv().await {
            let _ = retry_event_tx
                .send(Ok(AgentEvent::Retrying {
                    attempt: notif.attempt,
                    max_attempts: notif.max_attempts,
                    error: notif.error,
                }))
                .await;
        }
    });

    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut iteration = 0;
    let start_time = Instant::now();
    let mut repetition_tracker: HashMap<String, usize> = HashMap::new();
    let mut reflector = Reflector::new();
    let mut checkpoint_manager = GitCheckpointManager::new(&config.working_dir);
    let runtime_checkpoint_store = config
        .experimental
        .durable_resume_v2
        .then(|| RuntimeCheckpointStore::new(&config.working_dir));
    let mut runtime_state = RuntimeResumeState::default();
    let mut resumed_from_checkpoint = false;
    let mut resume_stage_hint: Option<String> = None;
    let verifier =
        VerifierPipeline::new(config.verifier.clone(), config.experimental.verifier_pipeline);
    let mut verifier_stats = VerifierStats::default();
    let episodic_store =
        config.experimental.episodic_memory.then(|| EpisodicMemoryStore::new(&config.working_dir));
    let context_fingerprint = build_context_fingerprint(&config);
    let mut pending_episode: Option<(String, String)> = None;
    let runtime_mode =
        if config.experimental.graph_hybrid_runtime { "graph_hybrid" } else { "linear" };
    // Track rejected tool calls to avoid repeated confirmation requests.
    // Values include an Instant for TTL-based expiry (5 minutes).
    let mut rejected_tools: HashMap<String, (String, Instant)> = HashMap::new();

    // Track context overflow recovery attempts
    let mut context_recovery_attempts = 0;
    // Track task completion state for graceful convergence
    let mut task_completed_at_iteration: Option<usize> = None;

    // --- Cost tracking ---
    let cost_config = forge_config::CostConfig::default();
    let cost_tracker = Arc::new(CostTracker::from_config(&cost_config));
    let agent_id = config
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    cost_tracker.register_agent(&agent_id, "main", &config.model, None);
    let mut budget_exceeded = false;

    // --- Trace recording ---
    let mut trace_recorder = TraceRecorder::new(
        config.session_id.as_deref(),
        "main",
        &config.model,
        config.experimental.trace_recording,
    );

    // Get tool definitions and names for prompt context
    let tool_defs: Vec<forge_llm::ToolDef> = executor
        .get_tool_definitions()
        .into_iter()
        .map(|t| forge_llm::ToolDef {
            name: t.name,
            description: t.description,
            parameters: t.parameters,
        })
        .collect();
    let tool_names: Vec<String> = tool_defs.iter().map(|t| t.name.clone()).collect();

    // Build system prompt using PromptManager
    let prompt_ctx = PromptContext::builder()
        .working_dir(&config.working_dir)
        .model(&config.model)
        .project_prompt(config.project_prompt.clone())
        .tools(tool_names)
        .skills(config.skills.clone())
        .memory_user_index(config.memory_user_index.clone())
        .memory_project_index(config.memory_project_index.clone())
        .build();
    let system_prompt = prompt_manager.build_system_prompt(&prompt_ctx);

    // LLM config
    let llm_config = LlmConfig {
        model: config.model.clone(),
        max_tokens: config.generation.max_tokens,
        temperature: config.generation.temperature,
        system_prompt: Some(system_prompt),
        system_blocks: None,
        enable_cache: true,
        thinking: config.thinking.clone(),
        thinking_adaptor: config.thinking_adaptor,
        stream_timeout_secs: LlmConfig::default_stream_timeout_secs(),
        response_schema: None,
    };

    if let (Some(store), Some(session_id)) =
        (runtime_checkpoint_store.as_ref(), config.session_id.as_deref())
    {
        if let Some(cp) = store.load(session_id).await? {
            resumed_from_checkpoint = true;
            iteration = cp.round;
            resume_stage_hint = Some(cp.stage.clone());
            messages = cp.messages;
            runtime_state.applied_tool_call_ids =
                cp.applied_tool_call_ids.into_iter().collect::<HashSet<_>>();
            runtime_state.side_effect_markers =
                cp.side_effect_markers.into_iter().collect::<HashSet<_>>();
            runtime_state.pending_approvals = cp.pending_approvals;
            tracing::info!(
                session_id = %session_id,
                round = iteration,
                stage = %cp.stage,
                applied_side_effects = runtime_state.applied_tool_call_ids.len(),
                "Resumed agent loop from durable checkpoint"
            );
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: "Resumed from durable checkpoint".to_string(),
                    suggestion: Some(format!("Resumed at round {} stage {}", iteration, cp.stage)),
                }))
                .await;
        }
    }

    if !resumed_from_checkpoint || messages.is_empty() {
        // Add conversation history first
        for msg in history {
            let role = match msg.role {
                HistoryRole::User | HistoryRole::System => ChatRole::User,
                HistoryRole::Assistant => ChatRole::Assistant,
            };
            messages.push(ChatMessage { role, content: MessageContent::Text(msg.content.clone()) });
        }

        // Add initial user message (the current query)
        messages.push(ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Text(initial_query.clone()),
        });
    }

    // ---------------------------------------------------------------------------
    // Main loop
    // ---------------------------------------------------------------------------

    loop {
        iteration += 1;

        // Check loop protection
        check_loop_protection(&config.loop_protection, iteration, start_time, &cancellation)?;

        // Check for graceful convergence after task completion
        if let Some(completed_at) = task_completed_at_iteration {
            let iterations_since_completion = iteration.saturating_sub(completed_at);
            if iterations_since_completion > config.loop_protection.post_completion_iterations {
                tracing::info!(
                    iteration = iteration,
                    completed_at = completed_at,
                    post_completion_iterations = config.loop_protection.post_completion_iterations,
                    "Gracefully stopping: task completed and post-completion iterations exceeded"
                );
                let _ = tx
                    .send(Ok(AgentEvent::Done {
                        summary: Some("Task completed successfully. Stopping to avoid unnecessary iterations.".to_string()),
                    }))
                    .await;
                if let (Some(store), Some(session_id)) =
                    (runtime_checkpoint_store.as_ref(), config.session_id.as_deref())
                {
                    store.clear(session_id).await?;
                }
                return Ok(());
            }
        }

        // Send thinking start event
        let _ = tx.send(Ok(AgentEvent::ThinkingStart)).await;

        // Begin trace round (if recording)
        let messages_json = serde_json::to_string(&messages).unwrap_or_default();
        trace_recorder.begin_round(
            iteration,
            &messages_json,
            messages.len(),
            tool_defs.len(),
            &config.model,
            config.generation.max_tokens,
            config.generation.temperature,
        );

        let trace_ref = if trace_recorder.is_enabled() {
            Some(&mut trace_recorder)
        } else {
            None
        };

        let PreToolStageOutput { full_text, tool_calls, usage } = run_pre_tool_stages(
            iteration,
            &mut messages,
            &provider,
            &tool_defs,
            &llm_config,
            &config,
            &tx,
            &cancellation,
            &mut context_recovery_attempts,
            trace_ref,
        )
        .await?;

        // --- Cost tracking: record usage and check budget ---
        if let Some(ref u) = usage {
            let check = cost_tracker.record_usage(&agent_id, u, &config.model);
            // Always emit CostUpdate when cost tracking is enabled
            if cost_tracker.is_enabled() {
                let cost = cost_tracker.agent_cost(&agent_id).unwrap_or(0.0);
                let _ = tx
                    .send(Ok(AgentEvent::CostUpdate {
                        agent_id: agent_id.clone(),
                        input_tokens: u.input_tokens,
                        output_tokens: u.output_tokens,
                        estimated_cost_usd: cost,
                        budget_limit_usd: None,
                    }))
                    .await;
            }
            match check {
                CostCheckResult::Warning { current_usd, limit_usd, percentage } => {
                    let _ = tx
                        .send(Ok(AgentEvent::CostWarning {
                            agent_id: agent_id.clone(),
                            current_usd,
                            limit_usd,
                            percentage,
                        }))
                        .await;
                }
                CostCheckResult::BudgetExceeded { current_usd, limit_usd } => {
                    let _ = tx
                        .send(Ok(AgentEvent::BudgetExceeded {
                            agent_id: agent_id.clone(),
                            current_usd,
                            limit_usd,
                        }))
                        .await;
                    budget_exceeded = true;
                }
                CostCheckResult::Ok => {}
            }
        }

        tracing::debug!(
            iteration,
            text_len = full_text.len(),
            tool_call_count = tool_calls.len(),
            "LLM stream processing complete"
        );

        persist_runtime_checkpoint(
            runtime_checkpoint_store.as_ref(),
            config.session_id.as_deref(),
            iteration,
            AgentLoopStage::ProcessLlmStream.as_str(),
            &messages,
            &runtime_state,
            &format_runtime_tool_states(&tool_calls, "ready"),
            None,
        )
        .await?;

        // Add assistant message to history
        if !full_text.is_empty() || !tool_calls.is_empty() {
            // Build content blocks for assistant message
            let content = if tool_calls.is_empty() {
                MessageContent::Text(full_text.clone())
            } else {
                // Include both text and tool use blocks
                let mut blocks = Vec::new();
                if !full_text.is_empty() {
                    blocks.push(ContentBlock::Text { text: full_text.clone() });
                }
                for call in &tool_calls {
                    blocks.push(ContentBlock::ToolUse {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        input: call.input.clone(),
                    });
                }
                MessageContent::Blocks(blocks)
            };

            messages.push(ChatMessage { role: ChatRole::Assistant, content });
        }

        // If no tool calls, check if this is a valid completion or an empty response error
        if tool_calls.is_empty() {
            // Empty response on first iteration is likely a model error
            if full_text.is_empty() && iteration == 1 {
                let msg = "LLM returned empty response (no text, no tool calls). This may indicate a model compatibility issue or API error.".to_string();
                tracing::warn!("{}", msg);
                let _ = tx.send(Ok(AgentEvent::Error { message: msg.clone() })).await;
                return Err(AgentError::LlmError(msg));
            }

            // Normal completion with text response
            checkpoint_manager.clear();
            if let (Some(store), Some(session_id)) =
                (runtime_checkpoint_store.as_ref(), config.session_id.as_deref())
            {
                store.clear(session_id).await?;
            }

            // End trace round and finalize
            trace_recorder.end_round();
            if trace_recorder.is_enabled() {
                let trace_id = trace_recorder.trace_id().to_string();
                if let Err(e) = trace_recorder.finalize().await {
                    tracing::warn!("Failed to finalize trace: {e}");
                } else {
                    let _ = tx.send(Ok(AgentEvent::TraceRecorded { trace_id })).await;
                }
            }

            tracing::info!(
                iteration,
                text_len = full_text.len(),
                runtime_mode,
                resumed_from_checkpoint,
                resume_stage = ?resume_stage_hint,
                e2e_latency_ms = u64::try_from(start_time.elapsed().as_millis()).unwrap_or(u64::MAX),
                verifier_evaluated = verifier_stats.evaluated,
                verifier_warnings = verifier_stats.warnings,
                verifier_blocked = verifier_stats.blocked,
                "Agent loop completed: sending Done event"
            );
            let _ = tx
                .send(Ok(AgentEvent::Done {
                    summary: if full_text.is_empty() { None } else { Some(full_text) },
                }))
                .await;
            return Ok(());
        }

        // Check for repeated tool calls
        if config.loop_protection.detect_repetition {
            check_tool_repetition(
                &tool_calls,
                &mut repetition_tracker,
                &config.loop_protection,
                &tx,
            )
            .await?;
        }

        // Execute tool calls
        let ToolDispatchOutput { tool_results, mut rollback_hint } = {
            let mut ds = DispatchState {
                runtime_state: &mut runtime_state,
                reflector: &mut reflector,
                checkpoint_manager: &mut checkpoint_manager,
                verifier_stats: &mut verifier_stats,
                rejected_tools: &mut rejected_tools,
                pending_episode: &mut pending_episode,
                task_completed_at_iteration: &mut task_completed_at_iteration,
            };
            run_optional_stage(
                config.experimental.graph_hybrid_runtime,
                iteration,
                AgentLoopStage::ToolDispatch,
                dispatch_and_execute_tools(
                    &tool_calls,
                    &executor,
                    &permission_manager,
                    &config,
                    &verifier,
                    confirmation_handler.as_ref(),
                    runtime_checkpoint_store.as_ref(),
                    episodic_store.as_ref(),
                    &context_fingerprint,
                    &messages,
                    iteration,
                    &tx,
                    &cancellation,
                    &mut ds,
                ),
            )
            .await?
        };

        // Record tool calls to trace (after dispatch)
        if trace_recorder.is_enabled() {
            for (call, result) in tool_calls.iter().zip(tool_results.iter()) {
                // Duration is not tracked at this level; use 0 as placeholder.
                // The ToolExecutor tracks per-tool metrics separately.
                trace_recorder.record_tool_call(call, result, 0);
            }
        }

        // End trace round
        trace_recorder.end_round();

        // Check budget exceeded — graceful shutdown after finishing current tool calls
        if budget_exceeded {
            tracing::info!("Budget exceeded — gracefully stopping agent loop");
            trace_recorder.end_round();
            if trace_recorder.is_enabled() {
                let trace_id = trace_recorder.trace_id().to_string();
                if let Err(e) = trace_recorder.finalize().await {
                    tracing::warn!("Failed to finalize trace: {e}");
                } else {
                    let _ = tx.send(Ok(AgentEvent::TraceRecorded { trace_id })).await;
                }
            }
            let _ = tx
                .send(Ok(AgentEvent::Done {
                    summary: Some("Agent stopped: budget exceeded.".to_string()),
                }))
                .await;
            return Ok(());
        }

        let finalize_stage = async {
            // Sweep expired entries from rejected_tools to prevent unbounded growth
            if rejected_tools.len() > MAX_REJECTED_ENTRIES {
                rejected_tools.retain(|_, (_, rejected_at)| rejected_at.elapsed() < REJECTION_TTL);
            }

            let mut result_blocks: Vec<ContentBlock> = tool_results
                .iter()
                .map(|result| ContentBlock::ToolResult {
                    tool_use_id: result.tool_call_id.clone(),
                    content: result.output.clone(),
                    is_error: result.is_error,
                })
                .collect();

            // Append rollback hint as a Text block to avoid consecutive User messages
            if let Some(hint) = rollback_hint.take() {
                result_blocks.push(ContentBlock::Text { text: hint });
            }
            Ok::<Vec<ContentBlock>, AgentError>(result_blocks)
        };
        let result_blocks = run_optional_stage(
            config.experimental.graph_hybrid_runtime,
            iteration,
            AgentLoopStage::FinalizeRound,
            finalize_stage,
        )
        .await?;

        messages.push(ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Blocks(result_blocks),
        });

        persist_runtime_checkpoint(
            runtime_checkpoint_store.as_ref(),
            config.session_id.as_deref(),
            iteration,
            AgentLoopStage::FinalizeRound.as_str(),
            &messages,
            &runtime_state,
            &format_runtime_tool_states(&tool_calls, "done"),
            None,
        )
        .await?;

        // Continue loop with tool results
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use forge_prompt::PromptManager;

    #[test]
    fn test_prompt_manager_build_system_prompt() {
        let manager = PromptManager::new();
        let ctx = PromptContext::builder().working_dir("/test/path").model("test-model").build();
        let prompt = manager.build_system_prompt(&ctx);
        assert!(prompt.contains("Forge"));
        assert!(prompt.contains("Working directory"));
    }
}
