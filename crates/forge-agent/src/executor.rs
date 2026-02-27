//! Tool executor
//!
//! Handles tool dispatch, parallel execution, and result collection.

use forge_domain::{ToolCall, ToolError, ToolResult};
use forge_tools::metrics::ToolMetrics;
use forge_tools::{ConfirmationLevel, ToolContext, ToolRegistry};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};

/// Maximum size of tool output in bytes (50KB)
/// This prevents any single tool from returning too much data that could
/// cause API payload size limits (e.g., 413 errors)
const MAX_OUTPUT_SIZE: usize = 50 * 1024;

/// Helper to create a path-confirmation `ToolResult` (not in forge-domain).
fn tool_result_needs_path_confirmation(
    tool_call_id: impl Into<String>,
    path: impl Into<String>,
    reason: impl Into<String>,
) -> ToolResult {
    ToolResult {
        tool_call_id: tool_call_id.into(),
        output: String::new(),
        is_error: false,
        path_confirmation: Some(forge_domain::PathConfirmation {
            path: path.into(),
            reason: reason.into(),
        }),
    }
}

/// Tool executor manages tool execution
pub struct ToolExecutor {
    /// Tool registry
    registry: Arc<ToolRegistry>,
    /// Tool context (working directory, etc.)
    context: ToolContext,
    /// Timeout for tool execution
    timeout: Duration,
    /// Maximum output size
    max_output_size: usize,
    /// Paths confirmed by user (allowed to access outside working directory)
    confirmed_paths: Arc<RwLock<HashSet<PathBuf>>>,
    /// Execution metrics (shared, lock-free)
    metrics: Arc<ToolMetrics>,
}

impl ToolExecutor {
    /// Create a new tool executor
    #[must_use]
    pub fn new(registry: Arc<ToolRegistry>, context: ToolContext) -> Self {
        Self {
            registry,
            context,
            timeout: Duration::from_secs(120),
            max_output_size: MAX_OUTPUT_SIZE,
            confirmed_paths: Arc::new(RwLock::new(HashSet::new())),
            metrics: Arc::new(ToolMetrics::new()),
        }
    }

    /// Set execution timeout
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set maximum output size
    #[must_use]
    pub const fn with_max_output_size(mut self, size: usize) -> Self {
        self.max_output_size = size;
        self
    }

    /// Confirm a path for access (add to allowed paths)
    pub async fn confirm_path(&self, path: impl Into<PathBuf>) {
        let path = path.into();
        tracing::info!(path = %path.display(), "Path confirmed by user");
        self.confirmed_paths.write().await.insert(path);
    }

    /// Check if a path has been confirmed
    pub async fn is_path_confirmed(&self, path: &PathBuf) -> bool {
        let confirmed = self.confirmed_paths.read().await;
        confirmed.iter().any(|p| path.starts_with(p) || p == path)
    }

    /// Clear all confirmed paths
    pub async fn clear_confirmed_paths(&self) {
        self.confirmed_paths.write().await.clear();
    }

    /// Truncate output if it exceeds max size
    fn truncate_output(&self, content: String) -> String {
        if content.len() <= self.max_output_size {
            return content;
        }

        let mut truncate_at = self.max_output_size;
        while truncate_at > 0 && !content.is_char_boundary(truncate_at) {
            truncate_at -= 1;
        }

        let truncated = &content[..truncate_at];
        let remaining = content.len() - truncate_at;

        format!("{truncated}\n\n... (output truncated, {remaining} more bytes not shown)")
    }

    /// Safe preview for logs without breaking UTF-8 boundaries.
    fn preview_for_log(content: &str, max_len: usize) -> String {
        if content.len() <= max_len {
            return content.to_string();
        }

        let mut end = max_len.min(content.len());
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }

        format!("{}...", &content[..end])
    }

    /// Execute a single tool call with automatic retry support
    #[tracing::instrument(name = "tool_exec", skip(self), fields(tool = %call.name, call_id = %call.id))]
    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        let start = Instant::now();
        tracing::info!(tool = %call.name, call_id = %call.id, "Executing tool");
        tracing::debug!(
            tool = %call.name, call_id = %call.id,
            params = %serde_json::to_string_pretty(&call.input).unwrap_or_else(|_| call.input.to_string()),
            "Tool call parameters"
        );

        let Some(tool) = self.registry.get(&call.name) else {
            tracing::warn!(tool = %call.name, "Tool not found");
            #[allow(clippy::cast_possible_truncation)]
            self.metrics.record(&call.name, start.elapsed().as_millis() as u64, true);
            return ToolResult::error(&call.id, format!("Tool not found: {}", call.name));
        };

        let retry_config = tool.retry_config();
        let mut attempt = 0;

        // Create context with confirmed paths
        let mut ctx = self.context.clone();
        ctx.confirmed_paths.clone_from(&*self.confirmed_paths.read().await);

        loop {
            let result = tokio::time::timeout(self.timeout, async {
                tool.execute(call.input.clone(), &ctx).await
            })
            .await;

            match result {
                Ok(Ok(output)) => {
                    if let Some(result) = self.handle_output(call, output, &retry_config, &mut attempt, &start, &ctx).await {
                        return result;
                    }
                }
                Ok(Err(e)) => {
                    // Path confirmation — return immediately
                    if let ToolError::PathConfirmationRequired { path, reason } = &e {
                        tracing::info!(tool = %call.name, call_id = %call.id, path = %path, "Path confirmation required");
                        return tool_result_needs_path_confirmation(&call.id, path.clone(), reason.clone());
                    }

                    tracing::warn!(tool = %call.name, call_id = %call.id, elapsed_ms = start.elapsed().as_millis(), error = %e, "Tool execution failed");

                    if retry_config.is_enabled() && attempt < retry_config.max_retries {
                        attempt += 1;
                        let delay_ms = retry_config.delay_for_attempt(attempt - 1);
                        tracing::info!(tool = call.name, attempt, max_retries = retry_config.max_retries, delay_ms, error = %e, "Retrying after error");
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        continue;
                    }
                    #[allow(clippy::cast_possible_truncation)]
                    self.metrics.record(&call.name, start.elapsed().as_millis() as u64, true);
                    return ToolResult::error(&call.id, e.to_string());
                }
                Err(_) => {
                    if retry_config.is_enabled() && attempt < retry_config.max_retries {
                        attempt += 1;
                        let delay_ms = retry_config.delay_for_attempt(attempt - 1);
                        tracing::info!(tool = call.name, attempt, max_retries = retry_config.max_retries, delay_ms, "Retrying after timeout");
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        continue;
                    }
                    tracing::warn!(tool = %call.name, call_id = %call.id, elapsed_ms = start.elapsed().as_millis(), timeout_secs = self.timeout.as_secs(), "Tool execution timed out");
                    #[allow(clippy::cast_possible_truncation)]
                    self.metrics.record(&call.name, start.elapsed().as_millis() as u64, true);
                    return ToolResult::error(
                        &call.id,
                        format!(
                            "Tool execution timed out after {}s{}",
                            self.timeout.as_secs(),
                            if attempt > 0 { format!(" (after {attempt} retries)") } else { String::new() }
                        ),
                    );
                }
            }
        }
    }

    /// Handle successful tool output (may still be a logical error).
    ///
    /// Returns `None` to signal the caller should retry, or `Some(result)` when done.
    async fn handle_output(
        &self,
        call: &ToolCall,
        output: forge_domain::ToolOutput,
        retry_config: &forge_domain::RetryConfig,
        attempt: &mut u32,
        start: &Instant,
        _ctx: &ToolContext,
    ) -> Option<ToolResult> {
        let content = self.truncate_output(output.content);

        if output.is_error {
            if retry_config.is_enabled() && *attempt < retry_config.max_retries {
                *attempt += 1;
                let delay_ms = retry_config.delay_for_attempt(*attempt - 1);
                tracing::info!(
                    tool = call.name, attempt = *attempt,
                    max_retries = retry_config.max_retries, delay_ms,
                    "Retrying tool execution after error"
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                // Signal the outer loop to retry
                return None;
            }
            tracing::warn!(
                tool = %call.name, call_id = %call.id,
                elapsed_ms = start.elapsed().as_millis(),
                output_len = content.len(),
                "Tool execution returned error"
            );
            tracing::debug!(tool = %call.name, error_output = %content, "Tool error output");
            #[allow(clippy::cast_possible_truncation)]
            self.metrics.record(&call.name, start.elapsed().as_millis() as u64, true);
            return Some(ToolResult::error(&call.id, content));
        }

        tracing::info!(
            tool = %call.name, call_id = %call.id,
            elapsed_ms = start.elapsed().as_millis(),
            output_len = content.len(),
            "Tool execution succeeded"
        );
        let preview = Self::preview_for_log(&content, 500);
        tracing::debug!(tool = %call.name, output = %preview, "Tool output (truncated if > 500 chars)");
        #[allow(clippy::cast_possible_truncation)]
        self.metrics.record(&call.name, start.elapsed().as_millis() as u64, false);
        Some(ToolResult::success(&call.id, content))
    }

    /// Best-effort prewarm for a tool call.
    ///
    /// This is intentionally non-fatal and should only be used for optional
    /// preparation when experimental streaming execution is enabled.
    pub async fn prewarm(&self, call: &ToolCall) {
        let Some(tool) = self.registry.get(&call.name) else {
            return;
        };

        let ctx = self.context.clone();
        let timeout = std::cmp::min(self.timeout, Duration::from_secs(10));

        match tokio::time::timeout(timeout, tool.prewarm(call.input.clone(), &ctx)).await {
            Ok(Ok(())) => {
                tracing::debug!(tool = %call.name, call_id = %call.id, "Tool prewarm succeeded");
            }
            Ok(Err(e)) => {
                tracing::debug!(
                    tool = %call.name,
                    call_id = %call.id,
                    error = %e,
                    "Tool prewarm failed (ignored)"
                );
            }
            Err(_) => {
                tracing::debug!(
                    tool = %call.name,
                    call_id = %call.id,
                    "Tool prewarm timed out (ignored)"
                );
            }
        }
    }

    /// Execute multiple tool calls in parallel
    pub async fn execute_parallel(&self, calls: &[ToolCall]) -> Vec<ToolResult> {
        let futures: Vec<_> = calls.iter().map(|call| self.execute(call)).collect();

        futures::future::join_all(futures).await
    }

    /// Execute tool calls in groups based on dependencies
    ///
    /// Groups of readonly tools can run in parallel, while
    /// write operations must run sequentially.
    pub async fn execute_smart(&self, calls: &[ToolCall]) -> Vec<ToolResult> {
        let groups = self.group_by_dependency(calls);
        let mut results = Vec::new();

        for group in groups {
            let group_results = self.execute_parallel(&group).await;
            results.extend(group_results);
        }

        results
    }

    /// Execute with progress reporting
    pub async fn execute_with_progress(
        &self,
        calls: &[ToolCall],
        progress_tx: mpsc::Sender<ToolProgress>,
    ) -> Vec<ToolResult> {
        let mut results = Vec::new();

        for call in calls {
            let _ = progress_tx
                .send(ToolProgress::Started { id: call.id.clone(), name: call.name.clone() })
                .await;

            let result = self.execute(call).await;

            let _ = progress_tx
                .send(ToolProgress::Completed { id: call.id.clone(), is_error: result.is_error })
                .await;

            results.push(result);
        }

        results
    }

    /// Group tool calls by dependency
    fn group_by_dependency(&self, calls: &[ToolCall]) -> Vec<Vec<ToolCall>> {
        let mut groups: Vec<Vec<ToolCall>> = Vec::new();
        let mut current_group: Vec<ToolCall> = Vec::new();
        let mut current_is_readonly = true;

        for call in calls {
            let is_readonly =
                self.registry.get(&call.name).is_some_and(|tool| tool.is_readonly());

            if is_readonly && current_is_readonly {
                current_group.push(call.clone());
            } else {
                if !current_group.is_empty() {
                    groups.push(current_group);
                }
                current_group = vec![call.clone()];
                current_is_readonly = is_readonly;
            }
        }

        if !current_group.is_empty() {
            groups.push(current_group);
        }

        groups
    }

    /// Check if a tool requires confirmation
    #[must_use]
    pub fn requires_confirmation(&self, call: &ToolCall) -> bool {
        self.registry
            .get(&call.name)
            .is_some_and(|tool| tool.confirmation_level(&call.input) != ConfirmationLevel::None)
    }

    /// Get the confirmation level for a tool call
    #[must_use]
    pub fn get_confirmation_level(&self, call: &ToolCall) -> ConfirmationLevel {
        self.registry
            .get(&call.name)
            .map_or(ConfirmationLevel::None, |tool| tool.confirmation_level(&call.input))
    }

    /// Get tool definitions for LLM
    #[must_use]
    pub fn get_tool_definitions(&self) -> Vec<forge_domain::ToolDef> {
        self.registry.all_defs()
    }

    /// Get the tool registry
    #[must_use]
    pub const fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    /// Get the tool context
    #[must_use]
    pub const fn context(&self) -> &ToolContext {
        &self.context
    }

    /// Get the execution metrics
    #[must_use]
    pub const fn metrics(&self) -> &Arc<ToolMetrics> {
        &self.metrics
    }
}

/// Progress event for tool execution
#[derive(Debug, Clone)]
pub enum ToolProgress {
    /// Tool execution started
    Started {
        /// Tool call ID
        id: String,
        /// Tool name
        name: String,
    },
    /// Tool execution completed
    Completed {
        /// Tool call ID
        id: String,
        /// Whether execution resulted in error
        is_error: bool,
    },
}

/// Checks if tool results contain any errors
#[must_use]
pub fn has_errors(results: &[ToolResult]) -> bool {
    results.iter().any(|r| r.is_error)
}

/// Get only error results
#[must_use]
pub fn get_errors(results: &[ToolResult]) -> Vec<&ToolResult> {
    results.iter().filter(|r| r.is_error).collect()
}

/// Detect repeated tool calls
pub struct RepetitionDetector {
    /// Track tool calls by signature
    call_counts: HashMap<String, usize>,
    /// Maximum allowed repetitions
    max_repetitions: usize,
}

impl RepetitionDetector {
    /// Create a new detector
    #[must_use]
    pub fn new(max_repetitions: usize) -> Self {
        Self { call_counts: HashMap::new(), max_repetitions }
    }

    /// Check if a call is a repetition
    ///
    /// Returns Some(count) if this is a problematic repetition
    pub fn check(&mut self, call: &ToolCall) -> Option<usize> {
        // Create a signature from name + input
        let signature = format!("{}:{}", call.name, call.input);
        let count = self.call_counts.entry(signature).or_insert(0);
        *count += 1;

        if *count > self.max_repetitions {
            Some(*count)
        } else {
            None
        }
    }

    /// Reset the detector
    pub fn reset(&mut self) {
        self.call_counts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_repetition_detector() {
        let mut detector = RepetitionDetector::new(2);

        let call = ToolCall {
            id: "1".to_string(),
            name: "read".to_string(),
            input: json!({"path": "/test.txt"}),
        };

        assert!(detector.check(&call).is_none());
        assert!(detector.check(&call).is_none());
        assert!(detector.check(&call).is_some()); // Third call triggers
    }

    #[test]
    fn test_has_errors() {
        let results = vec![ToolResult::success("1", "ok"), ToolResult::error("2", "failed")];

        assert!(has_errors(&results));

        let results = vec![ToolResult::success("1", "ok")];
        assert!(!has_errors(&results));
    }

    #[test]
    fn test_get_errors() {
        let results = vec![
            ToolResult::success("1", "ok"),
            ToolResult::error("2", "failed"),
            ToolResult::success("3", "ok"),
        ];

        let errors = get_errors(&results);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].tool_call_id, "2");
    }

    #[test]
    fn test_tool_call_name_not_empty() {
        let call = ToolCall {
            id: "call_123".to_string(),
            name: "glob".to_string(),
            input: json!({"pattern": "**/*.rs"}),
        };

        assert!(!call.name.is_empty(), "Tool name should not be empty");
        assert_eq!(call.name, "glob");
    }

    #[test]
    fn test_tool_call_empty_name_detection() {
        let call = ToolCall {
            id: "call_123".to_string(),
            name: String::new(),
            input: json!({}),
        };

        assert!(call.name.is_empty(), "Empty name should be detectable");
    }

    #[test]
    fn test_truncate_output_small() {
        let registry = Arc::new(ToolRegistry::new());
        let context = ToolContext::default();
        let executor = ToolExecutor::new(registry, context).with_max_output_size(100);

        let content = "hello world".to_string();
        let result = executor.truncate_output(content.clone());
        assert_eq!(result, content);
    }

    #[test]
    fn test_truncate_output_large() {
        let registry = Arc::new(ToolRegistry::new());
        let context = ToolContext::default();
        let executor = ToolExecutor::new(registry, context).with_max_output_size(50);

        let content = "a".repeat(100);
        let result = executor.truncate_output(content);

        assert!(result.len() < 100 + 100);
        assert!(result.contains("output truncated"));
        assert!(result.contains("50 more bytes"));
    }

    #[test]
    fn test_truncate_output_utf8_safety() {
        let registry = Arc::new(ToolRegistry::new());
        let context = ToolContext::default();
        let executor = ToolExecutor::new(registry, context).with_max_output_size(10);

        // Chinese characters are multi-byte
        let content = "你好世界测试".to_string(); // 18 bytes (3 bytes per char)
        let result = executor.truncate_output(content);

        // Should not panic and should produce valid UTF-8
        assert!(result.is_ascii() || result.chars().all(|c| c.len_utf8() > 0));
        assert!(result.contains("truncated"));
    }

    #[test]
    fn test_max_output_size_constant() {
        // Verify the constant is set to a reasonable value (50KB)
        assert_eq!(MAX_OUTPUT_SIZE, 50 * 1024);
    }
}