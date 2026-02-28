//! Core agent loop implementation.
//!
//! Contains the `CoreAgent` struct and the main `run_agent_loop` function
//! that orchestrates LLM calls, tool execution, and state management.

use crate::{
    checkpoint::{
        GitCheckpointManager, RuntimeCheckpointStore, RuntimeCheckpointV2, RuntimeToolCallState,
    },
    episodic_memory::{EpisodeRecord, EpisodicMemoryStore},
    executor::ToolExecutor,
    prepare::{
        run_optional_stage, run_pre_tool_stages, AgentLoopStage, PreToolStageOutput,
    },
    reflector::{ErrorKind, RecoveryAction, Reflector},
    tool_dispatch::{
        abort_if_cancelled, check_tool_permission, check_tool_repetition,
        execute_parallel_call_batch, handle_path_confirmation, normalize_json,
        PathConfirmationOutcome, ToolCallBatches, ToolExecutionCoordinator,
        ToolPermissionOutcome, MAX_REJECTED_ENTRIES, REJECTION_TTL,
    },
    verifier::{VerifierDecision, VerifierPipeline},
    AgentConfig, AgentError, ConfirmationHandler, LoopProtectionConfig,
    ReflectionResult, Result,
};
use chrono::Utc;
use forge_domain::{AgentEvent, ToolCall, ToolResult};
use forge_prompt::{PromptContext, PromptManager};
use forge_llm::{
    ChatMessage, ChatRole, ContentBlock, InstrumentedProvider, LlmConfig, LlmProvider,
    MessageContent, RetryConfig,
};
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
pub type AgentEventStream =
    Pin<Box<dyn futures::Stream<Item = Result<AgentEvent>> + Send>>;

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

struct ToolDispatchOutput {
    tool_results: Vec<ToolResult>,
    rollback_hint: Option<String>,
}

#[derive(Default)]
struct RuntimeResumeState {
    applied_tool_call_ids: HashSet<String>,
    side_effect_markers: HashSet<String>,
    pending_approvals: Vec<String>,
}

#[derive(Default)]
struct VerifierStats {
    evaluated: usize,
    warnings: usize,
    blocked: usize,
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
                        || panic_payload.downcast_ref::<String>().map_or_else(
                            || "Agent loop panicked (unknown payload)".to_string(),
                            |s| format!("Agent loop panicked: {s}"),
                        ),
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

        let PreToolStageOutput { full_text, tool_calls } = run_pre_tool_stages(
            iteration,
            &mut messages,
            &provider,
            &tool_defs,
            &llm_config,
            &config,
            &tx,
            &cancellation,
            &mut context_recovery_attempts,
        )
        .await?;

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
        let dispatch_stage = async {
            let mut tool_results: Vec<ToolResult> = Vec::new();
            let mut rollback_hint: Option<String> = None;
            let coordinator = ToolExecutionCoordinator::new(
                &executor,
                &permission_manager,
                &config.working_dir,
                &config,
            );

            // Partition tool calls into parallel-eligible (read-only + auto-allowed) and serial groups.
            let ToolCallBatches { parallel_calls, mut serial_calls } =
                coordinator.partition(&tool_calls);

            // Execute parallel-eligible tools concurrently
            if parallel_calls.len() > 1 {
                let parallel_limit = coordinator.parallel_limit(parallel_calls.len());
                tracing::info!(
                    iteration,
                    stage = AgentLoopStage::ToolDispatch.as_str(),
                    count = parallel_calls.len(),
                    max_inflight = parallel_limit,
                    tools = %parallel_calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", "),
                    "Executing read-only tools in parallel"
                );

                // Send ToolExecuting events for all parallel tools
                for call in &parallel_calls {
                    let _ = tx
                        .send(Ok(AgentEvent::ToolExecuting {
                            id: call.id.clone(),
                            name: call.name.clone(),
                            input: call.input.clone(),
                        }))
                        .await;
                }

                abort_if_cancelled(&cancellation, &tx).await?;
                let parallel_results =
                    execute_parallel_call_batch(&executor, &parallel_calls, parallel_limit).await;

                // Process parallel results: send events and record in reflector
                for (call, result) in &parallel_results {
                    let _ = tx
                        .send(Ok(AgentEvent::ToolResult {
                            id: call.id.clone(),
                            output: result.output.clone(),
                            is_error: result.is_error,
                        }))
                        .await;

                    apply_verifier_decision(
                        &verifier,
                        call,
                        result,
                        true,
                        &tx,
                        &mut verifier_stats,
                    )
                    .await?;

                    reflector.record_result(result, &call.name);

                    if result.is_error && config.reflection.enabled {
                        let analysis = reflector.analyze(result, &call.name);
                        match &analysis.recovery_action {
                            RecoveryAction::Stop { reason } => {
                                let _ = tx
                                    .send(Ok(AgentEvent::Error {
                                        message: format!("Stopping: {reason}"),
                                    }))
                                    .await;
                                return Err(AgentError::PlanningError(reason.clone()));
                            }
                            RecoveryAction::ReportAndContinue { message } => {
                                let _ = tx
                                    .send(Ok(AgentEvent::Error { message: message.clone() }))
                                    .await;
                            }
                            _ => {}
                        }
                    }
                }

                tool_results.extend(parallel_results.into_iter().map(|(_, result)| result));
                abort_if_cancelled(&cancellation, &tx).await?;
            } else {
                // Only 0 or 1 parallel-eligible tools — move them back to serial
                serial_calls.splice(0..0, parallel_calls.into_iter());
            }

            // Best-effort prewarm for serial calls when streaming experiment is enabled.
            coordinator.prewarm_serial_calls(&serial_calls).await;

            // Execute remaining tools serially (need confirmation, write access, or other special handling)
            for call in &serial_calls {
                let is_readonly =
                    executor.registry().get(&call.name).is_some_and(|t| t.is_readonly());
                let side_effect_marker = build_side_effect_marker(call);

                // Idempotent replay guard for write-capable operations.
                if !is_readonly
                    && (runtime_state.applied_tool_call_ids.contains(&call.id)
                        || runtime_state.side_effect_markers.contains(&side_effect_marker))
                {
                    let skipped = ToolResult::success(
                        &call.id,
                        "Skipped already-applied side-effect call during durable resume",
                    );
                    let _ = tx
                        .send(Ok(AgentEvent::ToolResult {
                            id: call.id.clone(),
                            output: skipped.output.clone(),
                            is_error: skipped.is_error,
                        }))
                        .await;
                    tool_results.push(skipped);
                    continue;
                }

                runtime_state.pending_approvals = vec![call.id.clone()];
                persist_runtime_checkpoint(
                    runtime_checkpoint_store.as_ref(),
                    config.session_id.as_deref(),
                    iteration,
                    "awaiting_confirmation",
                    &messages,
                    &runtime_state,
                    &format_runtime_tool_states(std::slice::from_ref(call), "pending_confirmation"),
                    rollback_hint.as_deref(),
                )
                .await?;

                // Check permission and handle confirmation flow
                match check_tool_permission(
                    call,
                    &executor,
                    &permission_manager,
                    confirmation_handler.as_ref(),
                    &mut rejected_tools,
                    &config.working_dir,
                    &tx,
                )
                .await
                {
                    ToolPermissionOutcome::Skip(result) => {
                        runtime_state.pending_approvals.clear();
                        tool_results.push(result);
                        continue;
                    }
                    ToolPermissionOutcome::Proceed => {
                        runtime_state.pending_approvals.clear();
                    }
                }

                // Create git checkpoint before the first write operation
                if !checkpoint_manager.has_checkpoint() && !is_readonly {
                    if let Ok(Some(cp)) = checkpoint_manager.create().await {
                        let head_sha = cp.head_sha.clone();
                        reflector.set_has_checkpoint(true);
                        let _ = tx.send(Ok(AgentEvent::CheckpointCreated { head_sha })).await;
                    }
                }

                // Save interrupt point before side-effects.
                if !is_readonly {
                    persist_runtime_checkpoint(
                        runtime_checkpoint_store.as_ref(),
                        config.session_id.as_deref(),
                        iteration,
                        "before_side_effect",
                        &messages,
                        &runtime_state,
                        &format_runtime_tool_states(std::slice::from_ref(call), "ready"),
                        rollback_hint.as_deref(),
                    )
                    .await?;
                }

                // Send executing event (with input for UI display)
                let _ = tx
                    .send(Ok(AgentEvent::ToolExecuting {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        input: call.input.clone(),
                    }))
                    .await;

                // Execute tool with cancellation barrier
                abort_if_cancelled(&cancellation, &tx).await?;
                let result = executor.execute(call).await;

                // Handle path confirmation if needed
                let result = match handle_path_confirmation(
                    call,
                    result,
                    &executor,
                    confirmation_handler.as_ref(),
                    &mut rejected_tools,
                    &tx,
                    &cancellation,
                )
                .await?
                {
                    PathConfirmationOutcome::Continue(r) => r,
                    PathConfirmationOutcome::Skip(r) => {
                        tool_results.push(r);
                        continue;
                    }
                };

                if !is_readonly {
                    runtime_state.applied_tool_call_ids.insert(call.id.clone());
                    runtime_state.side_effect_markers.insert(side_effect_marker);
                    persist_runtime_checkpoint(
                        runtime_checkpoint_store.as_ref(),
                        config.session_id.as_deref(),
                        iteration,
                        "after_side_effect",
                        &messages,
                        &runtime_state,
                        &format_runtime_tool_states(std::slice::from_ref(call), "done"),
                        rollback_hint.as_deref(),
                    )
                    .await?;
                }

                // Send result event
                let _ = tx
                    .send(Ok(AgentEvent::ToolResult {
                        id: call.id.clone(),
                        output: result.output.clone(),
                        is_error: result.is_error,
                    }))
                    .await;

                apply_verifier_decision(
                    &verifier,
                    call,
                    &result,
                    is_readonly,
                    &tx,
                    &mut verifier_stats,
                )
                .await?;

                // Parse plan mode markers ONLY from plan mode tools
                if !result.is_error
                    && (call.name == "enter_plan_mode" || call.name == "exit_plan_mode")
                {
                    match parse_plan_mode_marker(&result.output) {
                        PlanModeMarker::Enter(plan_file) => {
                            tracing::info!(
                                plan_file = ?plan_file,
                                "Plan mode marker detected: entering plan mode"
                            );
                            let _ = tx.send(Ok(AgentEvent::PlanModeEntered { plan_file })).await;
                        }
                        PlanModeMarker::Exit { saved } => {
                            tracing::info!(
                                saved = saved,
                                "Plan mode marker detected: exiting plan mode"
                            );
                            let _ = tx
                                .send(Ok(AgentEvent::PlanModeExited {
                                    saved,
                                    plan_file: None,
                                }))
                                .await;
                        }
                        PlanModeMarker::None => {}
                    }
                }

                // Detect task completion: tests passing is a strong signal
                if task_completed_at_iteration.is_none()
                    && !result.is_error
                    && call.name == "bash"
                    && is_test_command_success(&call.input, &result.output)
                {
                    task_completed_at_iteration = Some(iteration);
                    checkpoint_manager.clear();
                    tracing::info!(iteration = iteration, "Task completion detected: tests passed");
                }

                // Record result in reflector for error tracking
                reflector.record_result(&result, &call.name);

                if result.is_error && config.reflection.enabled {
                    let analysis = reflector.analyze(&result, &call.name);
                    let signature =
                        build_episode_signature(&call.name, analysis.error_kind, &result.output);
                    let mut episodic_strategy: Option<String> = None;
                    if let Some(store) = episodic_store.as_ref() {
                        if let Some(record) =
                            store.find_latest(&signature, &context_fingerprint).await?
                        {
                            episodic_strategy = Some(record.strategy.clone());
                            let _ = tx
                                .send(Ok(AgentEvent::Recovery {
                                    action: "Using episodic memory hint".to_string(),
                                    suggestion: Some(record.strategy),
                                }))
                                .await;
                        }
                    }

                    if let Some(strategy) = analysis.suggestion.clone().or(episodic_strategy) {
                        pending_episode = Some((signature.clone(), strategy));
                    }

                    if matches!(analysis.recovery_action, RecoveryAction::Rollback { .. }) {
                        persist_runtime_checkpoint(
                            runtime_checkpoint_store.as_ref(),
                            config.session_id.as_deref(),
                            iteration,
                            "before_rollback",
                            &messages,
                            &runtime_state,
                            &format_runtime_tool_states(std::slice::from_ref(call), "rollback"),
                            rollback_hint.as_deref(),
                        )
                        .await?;
                    }

                    let reflect_fut = handle_error_recovery(
                        &analysis,
                        &call.name,
                        &mut reflector,
                        &mut checkpoint_manager,
                        &tx,
                    );
                    let recover = run_optional_stage(
                        config.experimental.graph_hybrid_runtime,
                        iteration,
                        AgentLoopStage::ReflectAndRecover,
                        reflect_fut,
                    )
                    .await?;
                    if let Some(hint) = recover {
                        rollback_hint = Some(hint);
                    }
                } else if !result.is_error {
                    maybe_append_episode_success(
                        episodic_store.as_ref(),
                        &mut pending_episode,
                        &context_fingerprint,
                        result.output.len() / 4,
                    )
                    .await;
                }

                tool_results.push(result);

                // Honor cancellation between calls after the just-finished tool has been
                // durably observed.
                abort_if_cancelled(&cancellation, &tx).await?;
            }

            Ok(ToolDispatchOutput { tool_results, rollback_hint })
        };

        let ToolDispatchOutput { tool_results, mut rollback_hint } = run_optional_stage(
            config.experimental.graph_hybrid_runtime,
            iteration,
            AgentLoopStage::ToolDispatch,
            dispatch_stage,
        )
        .await?;

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
// Helper functions
// ---------------------------------------------------------------------------

fn format_runtime_tool_states(calls: &[ToolCall], status: &str) -> Vec<RuntimeToolCallState> {
    calls
        .iter()
        .map(|call| RuntimeToolCallState {
            id: call.id.clone(),
            name: call.name.clone(),
            status: status.to_string(),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn persist_runtime_checkpoint(
    store: Option<&RuntimeCheckpointStore>,
    session_id: Option<&str>,
    round: usize,
    stage: &str,
    messages: &[ChatMessage],
    runtime_state: &RuntimeResumeState,
    tool_call_states: &[RuntimeToolCallState],
    rollback_hint: Option<&str>,
) -> Result<()> {
    let (Some(store), Some(session_id)) = (store, session_id) else {
        return Ok(());
    };

    let checkpoint = RuntimeCheckpointV2 {
        version: 2,
        session_id: session_id.to_string(),
        round,
        stage: stage.to_string(),
        messages: messages.to_vec(),
        tool_call_states: tool_call_states.to_vec(),
        pending_approvals: runtime_state.pending_approvals.clone(),
        rollback_hint: rollback_hint.map(str::to_string),
        applied_tool_call_ids: runtime_state.applied_tool_call_ids.iter().cloned().collect(),
        side_effect_markers: runtime_state.side_effect_markers.iter().cloned().collect(),
        updated_at: Utc::now(),
    };
    store.save(&checkpoint).await
}

fn build_side_effect_marker(call: &ToolCall) -> String {
    format!("{}:{}", call.name, normalize_json(&call.input))
}

fn build_context_fingerprint(config: &AgentConfig) -> String {
    format!("{}|{}", config.working_dir.display(), config.model)
}

fn build_episode_signature(tool_name: &str, error_kind: Option<ErrorKind>, output: &str) -> String {
    let kind = error_kind.unwrap_or(ErrorKind::Unknown);
    let head = output.lines().next().unwrap_or("").trim();
    format!("{kind:?}:{tool_name}:{head}")
}

async fn maybe_append_episode_success(
    store: Option<&EpisodicMemoryStore>,
    pending_episode: &mut Option<(String, String)>,
    context_fingerprint: &str,
    tokens_used: usize,
) {
    let Some(store) = store else {
        return;
    };
    let Some((signature, strategy)) = pending_episode.take() else {
        return;
    };

    let record = EpisodeRecord {
        signature,
        context_fingerprint: context_fingerprint.to_string(),
        strategy,
        success: true,
        tokens_used,
        created_at: Utc::now(),
    };
    if let Err(e) = store.append_success(record).await {
        tracing::warn!(error = %e, "Failed to append episodic memory");
    }
}

async fn apply_verifier_decision(
    verifier: &VerifierPipeline,
    call: &ToolCall,
    result: &ToolResult,
    is_readonly: bool,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    stats: &mut VerifierStats,
) -> Result<()> {
    stats.evaluated += 1;
    let decision = verifier.evaluate(call, result, is_readonly);
    match decision {
        VerifierDecision::Pass => return Ok(()),
        VerifierDecision::Warn { message } => {
            stats.warnings += 1;
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: "Verifier warning".to_string(),
                    suggestion: Some(message),
                }))
                .await;
        }
        VerifierDecision::Fail { reason } => {
            stats.blocked += 1;
            let _ = tx.send(Ok(AgentEvent::Error { message: reason.clone() })).await;
            return Err(AgentError::PlanningError(reason));
        }
    }

    Ok(())
}

async fn handle_error_recovery(
    analysis: &ReflectionResult,
    _tool_name: &str,
    reflector: &mut Reflector,
    checkpoint_manager: &mut GitCheckpointManager,
    tx: &mpsc::Sender<Result<AgentEvent>>,
) -> Result<Option<String>> {
    match &analysis.recovery_action {
        RecoveryAction::Retry { delay, max_retries } => {
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: format!(
                        "Retrying after {}ms (max {} retries)",
                        delay.as_millis(),
                        max_retries
                    ),
                    suggestion: analysis.suggestion.clone(),
                }))
                .await;
        }
        RecoveryAction::TryAlternative { hint } => {
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: "Trying alternative approach".to_string(),
                    suggestion: Some(hint.clone()),
                }))
                .await;
        }
        RecoveryAction::ReportAndContinue { message } => {
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: "Continuing despite error".to_string(),
                    suggestion: Some(message.clone()),
                }))
                .await;
        }
        RecoveryAction::TryCompression { hint } => {
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: "Attempting context compression".to_string(),
                    suggestion: Some(hint.clone()),
                }))
                .await;
        }
        RecoveryAction::Rollback { reason } => match checkpoint_manager.rollback().await {
            Ok(report) => {
                let _ = tx
                    .send(Ok(AgentEvent::RolledBack {
                        reason: reason.clone(),
                        files_restored: report.files_count,
                    }))
                    .await;

                reflector.reset_counters();
                reflector.record_rollback();

                return Ok(Some(format!(
                    "[System] The working tree was rolled back because: {reason}. \
                         All file changes have been reverted. \
                         Please try a completely different approach."
                )));
            }
            Err(e) => {
                tracing::error!("Rollback failed: {}", e);
                let _ = tx
                    .send(Ok(AgentEvent::Error { message: format!("Stopping: {reason}") }))
                    .await;
                return Err(AgentError::PlanningError(reason.clone()));
            }
        },
        RecoveryAction::Stop { reason } => {
            let _ =
                tx.send(Ok(AgentEvent::Error { message: format!("Stopping: {reason}") })).await;
            return Err(AgentError::PlanningError(reason.clone()));
        }
        RecoveryAction::Skip => {}
    }

    Ok(None)
}

/// Check loop protection constraints
fn check_loop_protection(
    config: &LoopProtectionConfig,
    iteration: usize,
    start_time: Instant,
    cancellation: &CancellationToken,
) -> Result<()> {
    // Check cancellation
    if cancellation.is_cancelled() {
        return Err(AgentError::Aborted);
    }

    // Check max iterations
    if iteration > config.max_iterations {
        return Err(AgentError::MaxIterations(config.max_iterations));
    }

    // Check total timeout
    if start_time.elapsed().as_secs() > config.total_timeout_secs {
        return Err(AgentError::Timeout(config.total_timeout_secs));
    }

    Ok(())
}

/// Check if a bash command is a test command that succeeded
fn is_test_command_success(input: &serde_json::Value, output: &str) -> bool {
    // Extract command from input
    let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");

    // Check if it's a test command
    let is_test_command = command.contains("npm test")
        || command.contains("npm run test")
        || command.contains("yarn test")
        || command.contains("pnpm test")
        || command.contains("cargo test")
        || command.contains("pytest")
        || command.contains("go test")
        || command.contains("jest")
        || command.contains("vitest")
        || command.contains("node --test");

    if !is_test_command {
        return false;
    }

    // Check if output indicates success
    let output_lower = output.to_lowercase();

    let has_success_indicator = output.contains("Exit code: 0")
        || output.contains("exit code: 0")
        || output_lower.contains("all tests passed")
        || output_lower.contains("tests passed")
        || (output_lower.contains("# pass") && !output_lower.contains("# fail 1"));

    let has_failure_indicator = output.contains("Exit code: 1")
        || output.contains("exit code: 1")
        || output_lower.contains("failed")
        || output_lower.contains("failure")
        || output_lower.contains("error:")
        || output_lower.contains("# fail 1")
        || output_lower.contains("not ok");

    has_success_indicator && !has_failure_indicator
}

// ---------------------------------------------------------------------------
// Plan mode marker
// ---------------------------------------------------------------------------

/// Plan mode marker parsing result
#[derive(Debug)]
enum PlanModeMarker {
    /// Entered plan mode with optional plan file path
    Enter(Option<String>),
    /// Exited plan mode (saved: bool)
    Exit { saved: bool },
    /// No marker found
    None,
}

/// The tools emit markers in this format:
/// - `__PLAN_MODE_ENTER__:{plan_file_path}` when entering plan mode
/// - `__PLAN_MODE_EXIT__:saved` or `__PLAN_MODE_EXIT__:not_saved` when exiting
fn parse_plan_mode_marker(output: &str) -> PlanModeMarker {
    // Check for enter marker
    if let Some(pos) = output.find("__PLAN_MODE_ENTER__:") {
        let start = pos + "__PLAN_MODE_ENTER__:".len();
        let rest = &output[start..];
        let path = rest.lines().next().map(|s| s.trim().to_string());
        return PlanModeMarker::Enter(path.filter(|s| !s.is_empty()));
    }

    // Check for exit marker
    if output.contains("__PLAN_MODE_EXIT__:saved") {
        return PlanModeMarker::Exit { saved: true };
    }
    if output.contains("__PLAN_MODE_EXIT__:not_saved") {
        return PlanModeMarker::Exit { saved: false };
    }

    PlanModeMarker::None
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

    #[test]
    fn test_check_loop_protection_ok() {
        let config = LoopProtectionConfig::default();
        let result = check_loop_protection(&config, 1, Instant::now(), &CancellationToken::new());
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_loop_protection_max_iterations() {
        let config = LoopProtectionConfig { max_iterations: 5, ..Default::default() };
        let result = check_loop_protection(&config, 10, Instant::now(), &CancellationToken::new());
        assert!(matches!(result, Err(AgentError::MaxIterations(_))));
    }

    #[test]
    fn test_check_loop_protection_cancelled() {
        let config = LoopProtectionConfig::default();
        let token = CancellationToken::new();
        token.cancel();
        let result = check_loop_protection(&config, 1, Instant::now(), &token);
        assert!(matches!(result, Err(AgentError::Aborted)));
    }

    #[test]
    fn test_parse_plan_mode_marker_enter() {
        let output = "Entering plan mode.\n\n__PLAN_MODE_ENTER__:/path/to/plan.md";
        match parse_plan_mode_marker(output) {
            PlanModeMarker::Enter(Some(path)) => {
                assert_eq!(path, "/path/to/plan.md");
            }
            _ => panic!("Expected Enter marker with path"),
        }
    }

    #[test]
    fn test_parse_plan_mode_marker_enter_empty_path() {
        let output = "Entering plan mode.\n\n__PLAN_MODE_ENTER__:";
        match parse_plan_mode_marker(output) {
            PlanModeMarker::Enter(None) => {}
            _ => panic!("Expected Enter marker with empty path"),
        }
    }

    #[test]
    fn test_parse_plan_mode_marker_exit_saved() {
        let output = "Exiting plan mode.\n\n__PLAN_MODE_EXIT__:saved";
        match parse_plan_mode_marker(output) {
            PlanModeMarker::Exit { saved: true } => {}
            _ => panic!("Expected Exit marker with saved=true"),
        }
    }

    #[test]
    fn test_parse_plan_mode_marker_exit_not_saved() {
        let output = "Exiting plan mode.\n\n__PLAN_MODE_EXIT__:not_saved";
        match parse_plan_mode_marker(output) {
            PlanModeMarker::Exit { saved: false } => {}
            _ => panic!("Expected Exit marker with saved=false"),
        }
    }

    #[test]
    fn test_parse_plan_mode_marker_none() {
        let output = "Just some regular tool output without any markers";
        match parse_plan_mode_marker(output) {
            PlanModeMarker::None => {}
            _ => panic!("Expected no marker"),
        }
    }
}