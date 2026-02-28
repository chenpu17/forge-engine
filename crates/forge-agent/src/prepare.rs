//! Context preparation for LLM calls.
//!
//! Handles token accounting, context trimming, and pre-tool stage orchestration.

use crate::context;
use crate::stream::process_llm_stream;
use crate::{AgentConfig, AgentError, Result};
use forge_domain::AgentEvent;
use forge_llm::{ChatMessage, LlmConfig, LlmProvider};
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Maximum number of context-overflow recovery attempts before giving up.
const MAX_CONTEXT_RECOVERY_ATTEMPTS: usize = 2;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Output from the pre-tool stages (context prep + model call + stream).
pub struct PreToolStageOutput {
    /// Accumulated full text from the LLM response.
    pub full_text: String,
    /// Parsed tool calls from the LLM response.
    pub tool_calls: Vec<forge_domain::ToolCall>,
}

/// Named stages of the agent loop, used for structured logging.
#[derive(Debug, Clone, Copy)]
pub enum AgentLoopStage {
    PrepareContext,
    CallModel,
    ProcessLlmStream,
    ToolDispatch,
    ReflectAndRecover,
    FinalizeRound,
}

impl AgentLoopStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PrepareContext => "prepare_context",
            Self::CallModel => "call_model",
            Self::ProcessLlmStream => "process_llm_stream",
            Self::ToolDispatch => "tool_dispatch",
            Self::ReflectAndRecover => "reflect_and_recover",
            Self::FinalizeRound => "finalize_round",
        }
    }
}

// ---------------------------------------------------------------------------
// Stage runners
// ---------------------------------------------------------------------------

/// Run a named stage with structured timing logs.
pub async fn run_stage<T, Fut>(
    iteration: usize,
    stage: AgentLoopStage,
    fut: Fut,
) -> T
where
    Fut: Future<Output = T>,
{
    let started = Instant::now();
    tracing::debug!(iteration, stage = stage.as_str(), "Agent stage start");
    let out = fut.await;
    tracing::debug!(
        iteration,
        stage = stage.as_str(),
        elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "Agent stage end"
    );
    out
}

/// Run a stage only when `enabled` is true; otherwise just await the future
/// without the timing wrapper.
pub async fn run_optional_stage<T, Fut>(
    enabled: bool,
    iteration: usize,
    stage: AgentLoopStage,
    fut: Fut,
) -> Result<T>
where
    Fut: Future<Output = Result<T>>,
{
    if enabled {
        run_stage(iteration, stage, fut).await
    } else {
        fut.await
    }
}

// ---------------------------------------------------------------------------
// Context preparation
// ---------------------------------------------------------------------------

/// Prepare context window for the next LLM call.
///
/// This helper emits context warning/compression events and returns the trimmed
/// message list to send to the model.
pub async fn prepare_iteration_messages(
    messages: &[ChatMessage],
    provider: &dyn LlmProvider,
    model: &str,
    tx: &mpsc::Sender<Result<AgentEvent>>,
) -> Vec<ChatMessage> {
    let token_estimates: Vec<usize> =
        messages.iter().map(context::estimate_message_tokens).collect();
    let current_tokens: usize = token_estimates.iter().sum();
    let max_context = provider.context_limit(model);
    let available = context::available_tokens(max_context);

    // Warn when we're above 80% to give UI/agent an early signal before
    // trimming becomes frequent.
    let warning_threshold = available / 5 * 4;
    if current_tokens > warning_threshold {
        let _ = tx
            .send(Ok(AgentEvent::ContextWarning { current_tokens, limit: available }))
            .await;
    }

    let trimmed_messages =
        context::trim_to_fit_with_estimates(messages, &token_estimates, max_context);
    if trimmed_messages.len() < messages.len() {
        let start_idx = token_estimates.len().saturating_sub(trimmed_messages.len());
        let trimmed_tokens: usize = token_estimates[start_idx..].iter().sum();
        let tokens_saved = current_tokens.saturating_sub(trimmed_tokens);

        let _ = tx
            .send(Ok(AgentEvent::ContextCompressed {
                messages_before: messages.len(),
                messages_after: trimmed_messages.len(),
                tokens_saved,
            }))
            .await;
    }

    trimmed_messages
}

// ---------------------------------------------------------------------------
// Pre-tool stage orchestration
// ---------------------------------------------------------------------------

/// Run the three pre-tool stages: context prep, model call, and stream processing.
///
/// On context-overflow errors the function attempts tiered compression up to
/// [`MAX_CONTEXT_RECOVERY_ATTEMPTS`] times before propagating the error.
#[allow(clippy::too_many_arguments)]
pub async fn run_pre_tool_stages(
    iteration: usize,
    messages: &mut Vec<ChatMessage>,
    provider: &Arc<dyn LlmProvider>,
    tool_defs: &[forge_llm::ToolDef],
    llm_config: &LlmConfig,
    config: &AgentConfig,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    cancellation: &CancellationToken,
    context_recovery_attempts: &mut usize,
) -> Result<PreToolStageOutput> {
    // Stage 1: token accounting + context trim
    let trimmed_messages = run_stage(
        iteration,
        AgentLoopStage::PrepareContext,
        prepare_iteration_messages(messages, provider.as_ref(), &config.model, tx),
    )
    .await;

    // Stage 2: model call (with context-overflow recovery)
    let stream = run_stage(iteration, AgentLoopStage::CallModel, async {
        loop {
            let result =
                provider.chat_stream(&trimmed_messages, tool_defs.to_vec(), llm_config).await;
            match result {
                Ok(stream) => {
                    *context_recovery_attempts = 0;
                    break Ok(stream);
                }
                Err(e) => {
                    let error_str = e.to_string();
                    if is_context_overflow_error(&error_str)
                        && *context_recovery_attempts < MAX_CONTEXT_RECOVERY_ATTEMPTS
                    {
                        *context_recovery_attempts += 1;
                        tracing::warn!(
                            attempt = *context_recovery_attempts,
                            "Context overflow detected, attempting smart compression"
                        );
                        let _ = tx
                            .send(Ok(AgentEvent::ContextRecoveryAttempt {
                                message: format!(
                                    "Context overflow detected (attempt {}/{}), compressing context...",
                                    *context_recovery_attempts, MAX_CONTEXT_RECOVERY_ATTEMPTS
                                ),
                            }))
                            .await;

                        let max_context = provider.context_limit(&config.model);
                        let (compressed, tier) =
                            context::tiered_compress(messages, max_context, provider, config).await;
                        tracing::info!(
                            tier,
                            before = messages.len(),
                            after = compressed.len(),
                            "Context overflow recovery via tiered compression"
                        );
                        *messages = compressed;
                        continue;
                    }

                    let _ = tx
                        .send(Ok(AgentEvent::Error { message: format!("LLM error: {e}") }))
                        .await;

                    if matches!(e, forge_llm::LlmError::Timeout(_)) {
                        break Err(AgentError::Timeout(
                            config.loop_protection.iteration_timeout_secs,
                        ));
                    }
                    break Err(AgentError::LlmError(error_str));
                }
            }
        }
    })
    .await?;

    // Stage 3: stream event processing
    let (full_text, tool_calls) = run_stage(
        iteration,
        AgentLoopStage::ProcessLlmStream,
        process_llm_stream(stream, tx, cancellation, config.experimental.streaming_tools),
    )
    .await?;

    Ok(PreToolStageOutput { full_text, tool_calls })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Detect whether an LLM error string indicates a context-window overflow.
fn is_context_overflow_error(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.contains("context length")
        || lower.contains("context_length")
        || lower.contains("token limit")
        || lower.contains("too many tokens")
        || lower.contains("maximum context")
        || lower.contains("exceeds the model")
        || lower.contains("input is too long")
        || lower.contains("prompt is too long")
}

// ---------------------------------------------------------------------------
// Plan mode marker
// ---------------------------------------------------------------------------

/// Plan mode marker parsing result
#[derive(Debug)]
pub(crate) enum PlanModeMarker {
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
pub(crate) fn parse_plan_mode_marker(output: &str) -> PlanModeMarker {
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

/// Check if a bash command is a test command that succeeded
pub(crate) fn is_test_command_success(input: &serde_json::Value, output: &str) -> bool {
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::context;
    use forge_llm::{ChatMessage, ChatRole, MessageContent};

    #[test]
    fn test_context_estimate_tokens() {
        let message = ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Text("Hello, this is a test message.".to_string()),
        };
        let tokens = context::estimate_message_tokens(&message);
        assert!(tokens > 0);
        assert!(tokens < 100); // Reasonable for short message
    }

    #[test]
    fn test_context_available_tokens() {
        let available = context::available_tokens(200_000);
        // Should be max - system_reserved - response_reserved
        assert_eq!(available, 200_000 - 4_000 - 4_096);
    }

    #[test]
    fn test_context_trim_to_fit() {
        // Create messages that would exceed a small context
        let messages: Vec<ChatMessage> = (0..100)
            .map(|i| ChatMessage {
                role: ChatRole::User,
                content: MessageContent::Text(format!(
                    "This is message number {} with some content.",
                    i
                )),
            })
            .collect();

        // Use a small context limit to force trimming
        let small_context = 1000;
        let trimmed = context::trim_to_fit(&messages, small_context);

        assert!(!trimmed.is_empty());
        assert!(trimmed.len() < messages.len());
        // Last message should be preserved - check via Debug format
        let last_content = format!("{:?}", trimmed.last().expect("non-empty").content);
        assert!(last_content.contains("99"));
    }

    #[test]
    fn test_context_trim_preserves_all_when_fits() {
        let messages: Vec<ChatMessage> = (0..3)
            .map(|i| ChatMessage {
                role: ChatRole::User,
                content: MessageContent::Text(format!("Short {}", i)),
            })
            .collect();

        // Use large context - should keep all messages
        let trimmed = context::trim_to_fit(&messages, 200_000);
        assert_eq!(trimmed.len(), messages.len());
    }

    #[test]
    fn test_is_context_overflow_error() {
        use super::is_context_overflow_error;

        assert!(is_context_overflow_error("context length exceeded"));
        assert!(is_context_overflow_error("context_length_exceeded"));
        assert!(is_context_overflow_error("too many tokens in request"));
        assert!(is_context_overflow_error("input is too long"));
        assert!(!is_context_overflow_error("rate limit exceeded"));
        assert!(!is_context_overflow_error("authentication failed"));
    }

    #[test]
    fn test_parse_plan_mode_marker_enter() {
        use super::{parse_plan_mode_marker, PlanModeMarker};
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
        use super::{parse_plan_mode_marker, PlanModeMarker};
        let output = "Entering plan mode.\n\n__PLAN_MODE_ENTER__:";
        match parse_plan_mode_marker(output) {
            PlanModeMarker::Enter(None) => {}
            _ => panic!("Expected Enter marker with empty path"),
        }
    }

    #[test]
    fn test_parse_plan_mode_marker_exit_saved() {
        use super::{parse_plan_mode_marker, PlanModeMarker};
        let output = "Exiting plan mode.\n\n__PLAN_MODE_EXIT__:saved";
        match parse_plan_mode_marker(output) {
            PlanModeMarker::Exit { saved: true } => {}
            _ => panic!("Expected Exit marker with saved=true"),
        }
    }

    #[test]
    fn test_parse_plan_mode_marker_exit_not_saved() {
        use super::{parse_plan_mode_marker, PlanModeMarker};
        let output = "Exiting plan mode.\n\n__PLAN_MODE_EXIT__:not_saved";
        match parse_plan_mode_marker(output) {
            PlanModeMarker::Exit { saved: false } => {}
            _ => panic!("Expected Exit marker with saved=false"),
        }
    }

    #[test]
    fn test_parse_plan_mode_marker_none() {
        use super::{parse_plan_mode_marker, PlanModeMarker};
        let output = "Just some regular tool output without any markers";
        match parse_plan_mode_marker(output) {
            PlanModeMarker::None => {}
            _ => panic!("Expected no marker"),
        }
    }
}
