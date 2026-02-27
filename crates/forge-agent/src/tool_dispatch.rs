//! Tool execution dispatch and coordination.
//!
//! Handles tool call partitioning (parallel vs serial), permission checking,
//! path confirmation flow, and repetition detection.

use crate::executor::ToolExecutor;
use crate::{AgentConfig, AgentError, ConfirmationHandler, ConfirmationLevel, LoopProtectionConfig, Result};
use forge_domain::{AgentEvent, ToolCall, ToolResult};
use forge_tools::trust_level::PermissionCheckResult;
use forge_tools::trust_permission::TrustAwarePermissionManager;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

/// TTL for rejected tool entries
pub const REJECTION_TTL: std::time::Duration = std::time::Duration::from_secs(300);
/// Maximum number of rejected tool entries to track.
pub const MAX_REJECTED_ENTRIES: usize = 200;

/// Check cancellation and emit event if cancelled.
pub async fn abort_if_cancelled(
    cancellation: &CancellationToken,
    tx: &mpsc::Sender<Result<AgentEvent>>,
) -> Result<()> {
    if cancellation.is_cancelled() {
        let _ = tx.send(Ok(AgentEvent::Cancelled)).await;
        return Err(AgentError::Aborted);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ToolCallBatches
// ---------------------------------------------------------------------------

/// Tool calls split by execution strategy.
pub struct ToolCallBatches {
    /// Read-only tools eligible for parallel execution.
    pub parallel_calls: Vec<ToolCall>,
    /// Tools that must be executed serially.
    pub serial_calls: Vec<ToolCall>,
}

// ---------------------------------------------------------------------------
// ToolExecutionCoordinator
// ---------------------------------------------------------------------------

/// Coordinates tool execution with permission checks and batching.
pub struct ToolExecutionCoordinator<'a> {
    /// Tool executor reference.
    pub executor: &'a Arc<ToolExecutor>,
    /// Permission manager reference.
    pub permission_manager: &'a Arc<Mutex<TrustAwarePermissionManager>>,
    /// Working directory path.
    pub working_dir: &'a std::path::Path,
    /// Whether streaming tools experiment is enabled.
    pub streaming_tools_enabled: bool,
    /// Maximum number of parallel in-flight tool calls.
    pub parallel_max_inflight: usize,
}

impl<'a> ToolExecutionCoordinator<'a> {
    /// Create a new coordinator.
    pub const fn new(
        executor: &'a Arc<ToolExecutor>,
        permission_manager: &'a Arc<Mutex<TrustAwarePermissionManager>>,
        working_dir: &'a std::path::Path,
        config: &'a AgentConfig,
    ) -> Self {
        Self {
            executor,
            permission_manager,
            working_dir,
            streaming_tools_enabled: config.experimental.streaming_tools,
            parallel_max_inflight: config.experimental.streaming_tool_max_inflight,
        }
    }

    /// Partition tool calls into parallel and serial batches.
    pub fn partition(&self, tool_calls: &[ToolCall]) -> ToolCallBatches {
        partition_tool_calls(
            tool_calls,
            self.executor.as_ref(),
            self.permission_manager,
            self.working_dir,
        )
    }

    /// Compute the effective parallel concurrency limit.
    pub fn parallel_limit(&self, call_count: usize) -> usize {
        if self.parallel_max_inflight == 0 {
            call_count.max(1)
        } else {
            self.parallel_max_inflight.max(1).min(call_count.max(1))
        }
    }

    /// Best-effort prewarm for serial calls when streaming is enabled.
    pub async fn prewarm_serial_calls(&self, calls: &[ToolCall]) {
        if !self.streaming_tools_enabled || calls.is_empty() {
            return;
        }

        for call in calls {
            // Prewarm is best-effort and does not affect execution semantics.
            self.executor.prewarm(call).await;
        }
    }
}

// ---------------------------------------------------------------------------
// execute_parallel_call_batch
// ---------------------------------------------------------------------------

/// Execute a batch of tool calls in parallel with a concurrency limit.
pub async fn execute_parallel_call_batch(
    executor: &Arc<ToolExecutor>,
    calls: &[ToolCall],
    max_inflight: usize,
) -> Vec<(ToolCall, ToolResult)> {
    let semaphore = Arc::new(Semaphore::new(max_inflight.max(1)));
    let futures: Vec<_> = calls
        .iter()
        .map(|call| {
            let call = call.clone();
            let executor_ref = Arc::clone(executor);
            let sem = Arc::clone(&semaphore);
            async move {
                let permit = sem.acquire_owned().await;
                if let Ok(_permit) = permit {
                    let result = executor_ref.execute(&call).await;
                    (call, result)
                } else {
                    let result = ToolResult::error(
                        &call.id,
                        "Parallel execution semaphore closed unexpectedly",
                    );
                    (call, result)
                }
            }
        })
        .collect();

    futures::future::join_all(futures).await
}

// ---------------------------------------------------------------------------
// is_plan_mode_transition_tool
// ---------------------------------------------------------------------------

/// Check if a tool is a plan mode transition tool.
pub fn is_plan_mode_transition_tool(name: &str) -> bool {
    name == "enter_plan_mode" || name == "exit_plan_mode"
}

// ---------------------------------------------------------------------------
// partition_tool_calls
// ---------------------------------------------------------------------------

/// Split tool calls into a parallel-safe read-only batch and a serial batch.
pub fn partition_tool_calls(
    tool_calls: &[ToolCall],
    executor: &ToolExecutor,
    permission_manager: &Mutex<TrustAwarePermissionManager>,
    working_dir: &std::path::Path,
) -> ToolCallBatches {
    let mut parallel_calls: Vec<ToolCall> = Vec::new();
    let mut serial_calls: Vec<ToolCall> = Vec::new();

    for call in tool_calls {
        let is_readonly =
            executor.registry().get(&call.name).is_some_and(|t| t.is_readonly());

        let is_auto_allowed = if is_readonly {
            let confirmation_level = executor.get_confirmation_level(call);
            let permission_check = {
                let pm = permission_manager.lock();
                pm.check(&call.name, &call.input, confirmation_level, working_dir)
            };
            matches!(permission_check, PermissionCheckResult::Allowed)
        } else {
            false
        };

        // Keep plan-mode transition tools serialized to preserve strict state transitions.
        if is_readonly && is_auto_allowed && !is_plan_mode_transition_tool(&call.name) {
            parallel_calls.push(call.clone());
        } else {
            serial_calls.push(call.clone());
        }
    }

    ToolCallBatches { parallel_calls, serial_calls }
}

// ---------------------------------------------------------------------------
// PathConfirmationOutcome
// ---------------------------------------------------------------------------

/// Outcome of path confirmation flow.
pub enum PathConfirmationOutcome {
    /// Use this result and continue post-processing
    Continue(ToolResult),
    /// Skip to next tool with this result
    Skip(ToolResult),
}

// ---------------------------------------------------------------------------
// handle_path_confirmation
// ---------------------------------------------------------------------------

/// Handle path confirmation when a tool requires access outside the working directory.
///
/// If the result contains a `path_confirmation` request, this function handles the
/// user confirmation flow and potentially re-executes the tool.
pub async fn handle_path_confirmation(
    call: &ToolCall,
    result: ToolResult,
    executor: &ToolExecutor,
    confirmation_handler: Option<&Arc<dyn ConfirmationHandler>>,
    rejected_tools: &mut HashMap<String, (String, Instant)>,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    cancellation: &CancellationToken,
) -> Result<PathConfirmationOutcome> {
    let path_conf = match result.path_confirmation {
        Some(ref pc) => pc.clone(),
        None => return Ok(PathConfirmationOutcome::Continue(result)),
    };

    tracing::info!(
        tool = %call.name, path = %path_conf.path,
        "Path confirmation required for tool"
    );

    // Check if previously rejected (with TTL)
    let path_rejection_key = format!("path:{}", path_conf.path);
    if let Some((reason, rejected_at)) = rejected_tools.get(&path_rejection_key) {
        if rejected_at.elapsed() < REJECTION_TTL {
            let err_result = ToolResult::error(
                &call.id,
                format!(
                    "Path access to '{}' was previously rejected. Reason: {}. \
                     Please choose a different path or ask the user for guidance.",
                    path_conf.path, reason
                ),
            );
            let _ = tx
                .send(Ok(AgentEvent::ToolResult {
                    id: call.id.clone(),
                    output: err_result.output.clone(),
                    is_error: err_result.is_error,
                }))
                .await;
            return Ok(PathConfirmationOutcome::Skip(err_result));
        }
        rejected_tools.remove(&path_rejection_key);
    }

    // Pre-register and send path confirmation event
    if let Some(handler) = confirmation_handler {
        handler.pre_register(&call.id).await;
    }
    let _ = tx
        .send(Ok(AgentEvent::PathConfirmationRequired {
            id: call.id.clone(),
            path: path_conf.path.clone(),
            reason: path_conf.reason.clone(),
        }))
        .await;

    // Wait for user confirmation
    let allowed = if let Some(handler) = confirmation_handler {
        match handler
            .wait_for_confirmation(
                &call.id,
                &call.name,
                &serde_json::json!({ "path": path_conf.path, "reason": path_conf.reason }),
                ConfirmationLevel::Once,
            )
            .await
        {
            Ok(allowed) => allowed,
            Err(e) => {
                tracing::warn!(
                    tool = %call.name, error = %e,
                    "Path confirmation handler error, treating as rejection"
                );
                false
            }
        }
    } else {
        tracing::warn!(tool = %call.name, "Path confirmation required but no handler configured, denying");
        false
    };

    if allowed {
        let path = std::path::PathBuf::from(&path_conf.path);
        executor.confirm_path(path).await;
        tracing::info!(tool = %call.name, path = %path_conf.path, "Path confirmed, retrying tool");
        abort_if_cancelled(cancellation, tx).await?;
        let new_result = executor.execute(call).await;
        Ok(PathConfirmationOutcome::Continue(new_result))
    } else {
        let path_rejection_key = format!("path:{}", path_conf.path);
        rejected_tools
            .insert(path_rejection_key, ("User denied path access".to_string(), Instant::now()));
        let err_result = ToolResult::error(
            &call.id,
            format!(
                "Path access denied by user: {}. DO NOT request access to this path again.",
                path_conf.path
            ),
        );
        Ok(PathConfirmationOutcome::Continue(err_result))
    }
}

// ---------------------------------------------------------------------------
// ToolPermissionOutcome
// ---------------------------------------------------------------------------

/// Outcome of tool permission check and confirmation flow.
pub enum ToolPermissionOutcome {
    /// Tool is allowed to execute
    Proceed,
    /// Tool was denied/rejected — skip with this result
    Skip(ToolResult),
}

// ---------------------------------------------------------------------------
// check_tool_permission
// ---------------------------------------------------------------------------

/// Check tool permission and handle confirmation flow.
///
/// Returns `Proceed` if the tool should be executed, or `Skip(result)` if it was
/// denied/rejected/blocked.
#[allow(clippy::too_many_lines)]
pub async fn check_tool_permission(
    call: &ToolCall,
    executor: &ToolExecutor,
    permission_manager: &Mutex<TrustAwarePermissionManager>,
    confirmation_handler: Option<&Arc<dyn ConfirmationHandler>>,
    rejected_tools: &mut HashMap<String, (String, Instant)>,
    working_dir: &std::path::Path,
    tx: &mpsc::Sender<Result<AgentEvent>>,
) -> ToolPermissionOutcome {
    let confirmation_level = executor.get_confirmation_level(call);

    let permission_check = {
        let pm = permission_manager.lock();
        pm.check(&call.name, &call.input, confirmation_level, working_dir)
    };

    // Handle immediate deny/block cases
    match permission_check {
        PermissionCheckResult::Allowed => return ToolPermissionOutcome::Proceed,
        PermissionCheckResult::Denied { reason } => {
            let result = ToolResult::error(&call.id, format!("Tool denied: {reason}"));
            let _ = tx
                .send(Ok(AgentEvent::ToolResult {
                    id: call.id.clone(),
                    output: result.output.clone(),
                    is_error: result.is_error,
                }))
                .await;
            return ToolPermissionOutcome::Skip(result);
        }
        PermissionCheckResult::HardBlocked { reason } => {
            let result = ToolResult::error(
                &call.id,
                format!("Operation blocked by safety layer: {reason}"),
            );
            let _ = tx
                .send(Ok(AgentEvent::ToolResult {
                    id: call.id.clone(),
                    output: result.output.clone(),
                    is_error: result.is_error,
                }))
                .await;
            return ToolPermissionOutcome::Skip(result);
        }
        PermissionCheckResult::NeedsConfirmation { .. } => {
            // Fall through to confirmation flow below
        }
    }

    // Check if previously rejected (with TTL expiry)
    let rejection_signature = format!("{}:{}", call.name, normalize_json(&call.input));
    if let Some((reason, rejected_at)) = rejected_tools.get(&rejection_signature) {
        if rejected_at.elapsed() < REJECTION_TTL {
            let result = ToolResult::error(
                &call.id,
                format!(
                    "Tool '{}' was previously rejected in this session. Reason: {}. \
                     Please try a different approach or ask the user for guidance.",
                    call.name, reason
                ),
            );
            let _ = tx
                .send(Ok(AgentEvent::ToolResult {
                    id: call.id.clone(),
                    output: result.output.clone(),
                    is_error: result.is_error,
                }))
                .await;
            return ToolPermissionOutcome::Skip(result);
        }
        rejected_tools.remove(&rejection_signature);
    }

    // Pre-register and send confirmation event
    if let Some(handler) = confirmation_handler {
        handler.pre_register(&call.id).await;
    }

    let _ = tx
        .send(Ok(AgentEvent::ConfirmationRequired {
            id: call.id.clone(),
            tool: call.name.clone(),
            params: call.input.clone(),
            level: confirmation_level,
        }))
        .await;

    // Wait for user confirmation
    let allowed = if let Some(handler) = confirmation_handler {
        match handler
            .wait_for_confirmation(&call.id, &call.name, &call.input, confirmation_level)
            .await
        {
            Ok(allowed) => allowed,
            Err(e) => {
                tracing::warn!(
                    tool = %call.name, error = %e,
                    "Confirmation handler error, treating as rejection"
                );
                false
            }
        }
    } else {
        tracing::warn!(
            tool = %call.name, level = ?confirmation_level,
            "Tool requires confirmation but no handler configured, skipping"
        );
        false
    };

    if allowed {
        let mut pm = permission_manager.lock();
        pm.record_confirmation(&call.name, &call.input);
        ToolPermissionOutcome::Proceed
    } else {
        let rejection_signature = format!("{}:{}", call.name, normalize_json(&call.input));
        rejected_tools.insert(rejection_signature, ("User declined".to_string(), Instant::now()));

        let result = ToolResult::error(
            &call.id,
            format!(
                "Tool '{}' was rejected by user. DO NOT call this tool again with the same parameters. \
                 Try a different approach or ask the user for guidance.",
                call.name
            ),
        );
        let _ = tx
            .send(Ok(AgentEvent::ToolResult {
                id: call.id.clone(),
                output: result.output.clone(),
                is_error: result.is_error,
            }))
            .await;
        ToolPermissionOutcome::Skip(result)
    }
}

// ---------------------------------------------------------------------------
// check_tool_repetition
// ---------------------------------------------------------------------------

/// Check for repeated tool calls and cap tracker size.
///
/// Returns an error if any tool call exceeds its repetition limit.
pub async fn check_tool_repetition(
    tool_calls: &[ToolCall],
    tracker: &mut HashMap<String, usize>,
    config: &LoopProtectionConfig,
    tx: &mpsc::Sender<Result<AgentEvent>>,
) -> Result<()> {
    const MAX_TRACKER_ENTRIES: usize = 500;

    for call in tool_calls {
        let signature = format!("{}:{}", call.name, normalize_json(&call.input));
        let count = tracker.entry(signature).or_insert(0);
        *count += 1;

        let limit =
            config.tool_call_limits.get(&call.name).copied().unwrap_or(config.max_same_tool_calls);

        if *count > limit {
            let msg = format!(
                "Repeated tool call detected: {} (called {} times, limit {})",
                call.name, count, limit
            );
            let _ = tx.send(Ok(AgentEvent::Error { message: msg.clone() })).await;
            return Err(AgentError::MaxIterations(*count));
        }
    }

    // Cap tracker size to prevent unbounded growth in long sessions
    if tracker.len() > MAX_TRACKER_ENTRIES {
        let mut entries: Vec<(String, usize)> = tracker.drain().collect();
        entries.sort_by_key(|(_, count)| *count);
        let keep_from = entries.len() / 2;
        tracker.extend(entries.into_iter().skip(keep_from));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// normalize_json
// ---------------------------------------------------------------------------

/// Normalize a JSON value for consistent signature generation.
/// Sorts object keys to ensure deterministic output regardless of JSON serialization order.
pub fn normalize_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            let pairs: Vec<String> = keys
                .iter()
                .filter_map(|k| map.get(*k).map(|v| format!("{}:{}", k, normalize_json(v))))
                .collect();
            format!("{{{}}}", pairs.join(","))
        }
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(normalize_json).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}
