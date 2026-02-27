//! Batch Tool - Execute multiple tools in parallel
//!
//! This tool allows executing multiple tool calls in a single request,
//! reducing the number of LLM round-trips needed for multi-step operations.

use crate::description::ToolDescriptions;
use crate::{ConfirmationLevel, Tool, ToolError, ToolExecutionContext, ToolOutput, ToolRegistry};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock, RwLock};

/// Maximum number of tool calls allowed in a single batch
const MAX_BATCH_SIZE: usize = 10;

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r"Execute multiple tools in parallel within a single request.

Use this tool when you need to perform multiple independent operations that don't depend on each other's results. This reduces round-trips and improves efficiency.

IMPORTANT GUIDELINES:
- Maximum 10 tool calls per batch
- Tools are executed in parallel, so they must be independent
- Do NOT use batch for operations that depend on previous results
- Some tools are not allowed in batch: batch, task, ask_user

GOOD USE CASES:
- Reading multiple files at once
- Running multiple grep searches
- Checking multiple file patterns with glob

BAD USE CASES:
- Operations where one depends on another's result
- Interactive tools that need user input
- Nested batch calls";

/// Tools that are not allowed in batch operations
const DISALLOWED_TOOLS: &[&str] = &["batch", "task", "ask_user"];

/// Batch tool for parallel execution of multiple tools
///
/// Uses a lazy registry reference that can be set after construction
/// to avoid circular dependency issues during initialization.
pub struct BatchTool {
    /// Lazy reference to the tool registry
    registry: RwLock<Option<Arc<ToolRegistry>>>,
}

impl BatchTool {
    /// Create a new batch tool (registry must be set later via `set_registry`)
    #[must_use]
    pub const fn new() -> Self {
        Self { registry: RwLock::new(None) }
    }

    /// Set the tool registry reference
    ///
    /// This should be called after the registry is fully initialized.
    ///
    /// # Panics
    ///
    /// Panics if the registry lock is poisoned.
    pub fn set_registry(&self, registry: Arc<ToolRegistry>) {
        #[allow(clippy::expect_used)]
        let mut guard = self.registry.write().expect("registry lock poisoned");
        *guard = Some(registry);
    }

    /// Get the tool registry
    fn get_registry(&self) -> Option<Arc<ToolRegistry>> {
        #[allow(clippy::expect_used)]
        self.registry.read().expect("registry lock poisoned").clone()
    }
}

impl Default for BatchTool {
    fn default() -> Self {
        Self::new()
    }
}

/// A single tool call in a batch
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchToolCall {
    /// Name of the tool to execute
    tool: String,
    /// Parameters for the tool
    parameters: Value,
}

/// Result of a single tool call in a batch
#[derive(Debug, Clone, Serialize)]
struct BatchCallResult {
    /// Tool name
    tool: String,
    /// Whether the call succeeded
    success: bool,
    /// Output (if successful)
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
    /// Error message (if failed)
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl Tool for BatchTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "batch"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("batch", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tool_calls": {
                    "type": "array",
                    "description": "Array of tool calls to execute in parallel",
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": {
                                "type": "string",
                                "description": "The name of the tool to execute"
                            },
                            "parameters": {
                                "type": "object",
                                "description": "Parameters for the tool"
                            }
                        },
                        "required": ["tool", "parameters"]
                    },
                    "minItems": 1,
                    "maxItems": 10
                }
            },
            "required": ["tool_calls"]
        })
    }

    #[allow(clippy::option_if_let_else)]
    fn confirmation_level(&self, params: &Value) -> ConfirmationLevel {
        let Some(registry) = self.get_registry() else {
            return ConfirmationLevel::None;
        };

        // Check if any tool in the batch requires confirmation
        if let Some(calls) = params.get("tool_calls").and_then(|v| v.as_array()) {
            let mut max_level = ConfirmationLevel::None;

            for call in calls {
                if let Some(tool_name) = call.get("tool").and_then(|v| v.as_str()) {
                    if let Some(tool) = registry.get(tool_name) {
                        let tool_params = call.get("parameters").cloned().unwrap_or_else(|| json!({}));
                        let level = tool.confirmation_level(&tool_params);

                        // Track the highest confirmation level
                        #[allow(clippy::match_same_arms)]
                        {
                            max_level = match (&max_level, &level) {
                                (_, ConfirmationLevel::Dangerous) | (ConfirmationLevel::Dangerous, _) => ConfirmationLevel::Dangerous,
                                (_, ConfirmationLevel::Always) | (ConfirmationLevel::Always, _) => ConfirmationLevel::Always,
                                (_, ConfirmationLevel::Once) | (ConfirmationLevel::Once, _) => ConfirmationLevel::Once,
                                _ => ConfirmationLevel::None,
                            };
                        }
                    }
                }
            }

            max_level
        } else {
            ConfirmationLevel::None
        }
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        // Get registry
        let registry = self.get_registry().ok_or_else(|| {
            ToolError::ExecutionFailed("Batch tool not initialized: registry not set".to_string())
        })?;

        // Log received parameters for debugging
        tracing::debug!(
            "Batch tool received params: {}",
            serde_json::to_string_pretty(&params).unwrap_or_else(|_| params.to_string())
        );

        // Try to parse tool_calls from various possible formats
        let tool_calls_value = params
            .get("tool_calls")
            .or_else(|| params.get("calls"))
            .or_else(|| params.get("tools"));

        let calls: Vec<BatchToolCall> = if let Some(v) = tool_calls_value {
            serde_json::from_value(v.clone()).map_err(|e| {
                tracing::warn!("Failed to parse tool_calls: {}. Received: {}", e, v);
                ToolError::InvalidParams(format!(
                    "Invalid 'tool_calls' format. Expected array of {{\"tool\": \"name\", \"parameters\": {{}}}}. Parse error: {}. Received: {}",
                    e,
                    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
                ))
            })?
        } else {
            tracing::warn!("Missing 'tool_calls' in params: {}", params);
            return Err(ToolError::InvalidParams(format!(
                "Missing 'tool_calls' array. Expected format: {{\"tool_calls\": [{{\"tool\": \"name\", \"parameters\": {{}}}}]}}. Received: {}",
                serde_json::to_string_pretty(&params).unwrap_or_else(|_| params.to_string())
            )));
        };

        if calls.is_empty() {
            return Err(ToolError::InvalidParams("At least one tool call is required".to_string()));
        }

        // Limit batch size
        let (calls_to_execute, discarded) = if calls.len() > MAX_BATCH_SIZE {
            (calls[..MAX_BATCH_SIZE].to_vec(), calls[MAX_BATCH_SIZE..].to_vec())
        } else {
            (calls, vec![])
        };

        // Validate all tools before execution
        for call in &calls_to_execute {
            // Check if tool is disallowed
            if DISALLOWED_TOOLS.contains(&call.tool.as_str()) {
                return Err(ToolError::InvalidParams(format!(
                    "Tool '{}' is not allowed in batch. Disallowed tools: {}",
                    call.tool,
                    DISALLOWED_TOOLS.join(", ")
                )));
            }

            // Check if tool exists
            if registry.get(&call.tool).is_none() {
                let available: Vec<_> = registry
                    .list_names()
                    .into_iter()
                    .filter(|n| !DISALLOWED_TOOLS.contains(n))
                    .collect();
                return Err(ToolError::InvalidParams(format!(
                    "Tool '{}' not found. Available tools: {}",
                    call.tool,
                    available.join(", ")
                )));
            }
        }

        // Execute all tools in parallel
        let futures: Vec<_> = calls_to_execute
            .iter()
            .filter_map(|call| {
                let tool = registry.get(&call.tool)?;
                let params = call.parameters.clone();
                let tool_name = call.tool.clone();

                Some(async move {
                    match tool.execute(params, ctx).await {
                        Ok(output) => BatchCallResult {
                            tool: tool_name,
                            success: !output.is_error,
                            output: Some(output.content),
                            error: None,
                        },
                        Err(e) => BatchCallResult {
                            tool: tool_name,
                            success: false,
                            output: None,
                            error: Some(e.to_string()),
                        },
                    }
                })
            })
            .collect();

        let mut results: Vec<BatchCallResult> = futures::future::join_all(futures).await;

        // Add discarded calls as errors
        for call in discarded {
            results.push(BatchCallResult {
                tool: call.tool,
                success: false,
                output: None,
                error: Some(format!("Exceeded maximum batch size of {MAX_BATCH_SIZE}")),
            });
        }

        // Build output
        let successful = results.iter().filter(|r| r.success).count();
        let failed = results.len() - successful;

        let mut output_lines = vec![
            "## Batch Execution Results".to_string(),
            String::new(),
            format!("**Summary**: {}/{} tools executed successfully", successful, results.len()),
            String::new(),
        ];

        for (i, result) in results.iter().enumerate() {
            let status = if result.success { "OK" } else { "FAILED" };
            output_lines.push(format!("### {}. {} [{}]", i + 1, result.tool, status));

            if let Some(ref output) = result.output {
                // Truncate long outputs
                let truncated = if output.len() > 2000 {
                    let mut end = 2000.min(output.len());
                    while end > 0 && !output.is_char_boundary(end) {
                        end -= 1;
                    }
                    format!(
                        "{}...\n[Output truncated, {} bytes total]",
                        &output[..end],
                        output.len()
                    )
                } else {
                    output.clone()
                };
                output_lines.push(format!("```\n{truncated}\n```"));
            }

            if let Some(ref error) = result.error {
                output_lines.push(format!("**Error**: {error}"));
            }

            output_lines.push(String::new());
        }

        if failed == 0 {
            output_lines.push("All tools executed successfully. Consider using batch for similar parallel operations.".to_string());
        }

        let output_content = output_lines.join("\n");

        let data = json!({
            "total": results.len(),
            "successful": successful,
            "failed": failed,
            "results": results,
        });

        Ok(ToolOutput {
            content: output_content,
            is_error: failed > 0 && successful == 0,
            data: Some(data),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;
    use std::time::Duration;

    // ==================== Mock Tool for Testing ====================

    /// A simple mock tool for testing batch execution
    struct MockTool {
        name: String,
        output: String,
        should_fail: bool,
        delay_ms: u64,
    }

    impl MockTool {
        fn new(name: &str, output: &str) -> Self {
            Self {
                name: name.to_string(),
                output: output.to_string(),
                should_fail: false,
                delay_ms: 0,
            }
        }

        fn failing(name: &str, error: &str) -> Self {
            Self {
                name: name.to_string(),
                output: error.to_string(),
                should_fail: true,
                delay_ms: 0,
            }
        }

        fn with_delay(mut self, ms: u64) -> Self {
            self.delay_ms = ms;
            self
        }
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "Mock tool for testing"
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {}
            })
        }

        async fn execute(
            &self,
            _params: Value,
            _ctx: &dyn ToolExecutionContext,
        ) -> std::result::Result<ToolOutput, ToolError> {
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }

            if self.should_fail {
                Err(ToolError::ExecutionFailed(self.output.clone()))
            } else {
                Ok(ToolOutput::success(&self.output))
            }
        }
    }

    /// Mock tool that requires confirmation
    struct ConfirmingMockTool {
        name: String,
        level: ConfirmationLevel,
    }

    impl ConfirmingMockTool {
        fn new(name: &str, level: ConfirmationLevel) -> Self {
            Self { name: name.to_string(), level }
        }
    }

    #[async_trait]
    impl Tool for ConfirmingMockTool {
        fn name(&self) -> &str {
            &self.name
        }

        fn description(&self) -> &str {
            "Mock tool with confirmation"
        }

        fn parameters_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {}
            })
        }

        fn confirmation_level(&self, _params: &Value) -> ConfirmationLevel {
            self.level
        }

        async fn execute(
            &self,
            _params: Value,
            _ctx: &dyn ToolExecutionContext,
        ) -> std::result::Result<ToolOutput, ToolError> {
            Ok(ToolOutput::success("confirmed"))
        }
    }

    // Helper to create a registry with mock tools
    fn create_test_registry() -> Arc<ToolRegistry> {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(MockTool::new("mock_read", "file content")));
        registry.register(Arc::new(MockTool::new("mock_grep", "grep results")));
        registry.register(Arc::new(MockTool::new("mock_glob", "glob results")));
        Arc::new(registry)
    }

    // ==================== Basic Tests ====================

    #[test]
    fn test_batch_tool_name() {
        let tool = BatchTool::new();
        assert_eq!(tool.name(), "batch");
    }

    #[test]
    fn test_batch_tool_description() {
        let tool = BatchTool::new();
        let desc = tool.description();
        assert!(desc.contains("parallel"));
        assert!(desc.contains("Maximum 10"));
    }

    #[test]
    fn test_batch_tool_schema() {
        let tool = BatchTool::new();
        let schema = tool.parameters_schema();

        assert!(schema.get("properties").is_some());
        assert!(schema["properties"]["tool_calls"].is_object());
        assert_eq!(schema["properties"]["tool_calls"]["type"], "array");
        assert_eq!(schema["properties"]["tool_calls"]["minItems"], 1);
        assert_eq!(schema["properties"]["tool_calls"]["maxItems"], 10);
    }

    #[test]
    fn test_disallowed_tools() {
        assert!(DISALLOWED_TOOLS.contains(&"batch"));
        assert!(DISALLOWED_TOOLS.contains(&"task"));
        assert!(DISALLOWED_TOOLS.contains(&"ask_user"));
    }

    #[test]
    fn test_max_batch_size() {
        assert_eq!(MAX_BATCH_SIZE, 10);
    }

    #[test]
    fn test_set_registry() {
        let tool = BatchTool::new();
        assert!(tool.get_registry().is_none());

        let registry = Arc::new(ToolRegistry::new());
        tool.set_registry(registry.clone());

        assert!(tool.get_registry().is_some());
    }

    #[test]
    fn test_default_impl() {
        let tool = BatchTool::default();
        assert_eq!(tool.name(), "batch");
        assert!(tool.get_registry().is_none());
    }

    // ==================== Execution Tests ====================

    #[tokio::test]
    async fn test_execute_single_tool() {
        let batch = BatchTool::new();
        let registry = create_test_registry();
        batch.set_registry(registry);

        let params = json!({
            "tool_calls": [
                {"tool": "mock_read", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await.expect("execute should succeed");

        assert!(!result.is_error);
        assert!(result.content.contains("1/1 tools executed successfully"));
        assert!(result.content.contains("file content"));
    }

    #[tokio::test]
    async fn test_execute_multiple_tools() {
        let batch = BatchTool::new();
        let registry = create_test_registry();
        batch.set_registry(registry);

        let params = json!({
            "tool_calls": [
                {"tool": "mock_read", "parameters": {}},
                {"tool": "mock_grep", "parameters": {}},
                {"tool": "mock_glob", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await.expect("execute should succeed");

        assert!(!result.is_error);
        assert!(result.content.contains("3/3 tools executed successfully"));
        assert!(result.content.contains("file content"));
        assert!(result.content.contains("grep results"));
        assert!(result.content.contains("glob results"));

        // Check data
        let data = result.data.expect("should have data");
        assert_eq!(data["total"], 3);
        assert_eq!(data["successful"], 3);
        assert_eq!(data["failed"], 0);
    }

    #[tokio::test]
    async fn test_execute_with_failure() {
        let batch = BatchTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(MockTool::new("mock_ok", "success")));
        registry.register(Arc::new(MockTool::failing("mock_fail", "error message")));
        batch.set_registry(Arc::new(registry));

        let params = json!({
            "tool_calls": [
                {"tool": "mock_ok", "parameters": {}},
                {"tool": "mock_fail", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await.expect("execute should succeed");

        // Partial failure - not all failed
        assert!(!result.is_error);
        assert!(result.content.contains("1/2 tools executed successfully"));

        let data = result.data.expect("should have data");
        assert_eq!(data["successful"], 1);
        assert_eq!(data["failed"], 1);
    }

    #[tokio::test]
    async fn test_execute_all_fail() {
        let batch = BatchTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(MockTool::failing("mock_fail1", "error1")));
        registry.register(Arc::new(MockTool::failing("mock_fail2", "error2")));
        batch.set_registry(Arc::new(registry));

        let params = json!({
            "tool_calls": [
                {"tool": "mock_fail1", "parameters": {}},
                {"tool": "mock_fail2", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await.expect("execute should succeed");

        // All failed - is_error should be true
        assert!(result.is_error);
        assert!(result.content.contains("0/2 tools executed successfully"));
    }

    // ==================== Error Handling Tests ====================

    #[tokio::test]
    async fn test_execute_without_registry() {
        let batch = BatchTool::new();
        // Don't set registry

        let params = json!({
            "tool_calls": [
                {"tool": "mock_read", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await;

        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("registry not set"));
    }

    #[tokio::test]
    async fn test_execute_empty_calls() {
        let batch = BatchTool::new();
        let registry = create_test_registry();
        batch.set_registry(registry);

        let params = json!({
            "tool_calls": []
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await;

        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("At least one tool call is required"));
    }

    #[tokio::test]
    async fn test_execute_missing_tool_calls() {
        let batch = BatchTool::new();
        let registry = create_test_registry();
        batch.set_registry(registry);

        let params = json!({});

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await;

        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("Missing 'tool_calls'"));
    }

    #[tokio::test]
    async fn test_execute_disallowed_tool() {
        let batch = BatchTool::new();
        let registry = create_test_registry();
        batch.set_registry(registry);

        let params = json!({
            "tool_calls": [
                {"tool": "batch", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await;

        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("not allowed in batch"));
    }

    #[tokio::test]
    async fn test_execute_unknown_tool() {
        let batch = BatchTool::new();
        let registry = create_test_registry();
        batch.set_registry(registry);

        let params = json!({
            "tool_calls": [
                {"tool": "nonexistent_tool", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await;

        assert!(result.is_err());
        let err = result.expect_err("should fail");
        assert!(err.to_string().contains("not found"));
    }

    // ==================== Batch Size Tests ====================

    #[tokio::test]
    async fn test_execute_exceeds_max_batch_size() {
        let batch = BatchTool::new();
        let mut registry = ToolRegistry::new();
        for i in 0..15 {
            registry.register(Arc::new(MockTool::new(
                &format!("mock_{i}"),
                &format!("output_{i}"),
            )));
        }
        batch.set_registry(Arc::new(registry));

        // Create 12 tool calls (exceeds MAX_BATCH_SIZE of 10)
        let tool_calls: Vec<_> =
            (0..12).map(|i| json!({"tool": format!("mock_{i}"), "parameters": {}})).collect();

        let params = json!({
            "tool_calls": tool_calls
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await.expect("execute should succeed");

        // Should have 10 successful + 2 discarded
        let data = result.data.expect("should have data");
        assert_eq!(data["total"], 12);
        assert_eq!(data["successful"], 10);
        assert_eq!(data["failed"], 2);

        // Check that discarded tools have the right error message
        let results = data["results"].as_array().expect("results array");
        let discarded: Vec<_> = results
            .iter()
            .filter(|r| {
                r["error"]
                    .as_str()
                    .map(|e| e.contains("Exceeded maximum batch size"))
                    .unwrap_or(false)
            })
            .collect();
        assert_eq!(discarded.len(), 2);
    }

    // ==================== Confirmation Level Tests ====================

    #[test]
    fn test_confirmation_level_none() {
        let batch = BatchTool::new();
        let registry = create_test_registry();
        batch.set_registry(registry);

        let params = json!({
            "tool_calls": [
                {"tool": "mock_read", "parameters": {}}
            ]
        });

        let level = batch.confirmation_level(&params);
        assert!(matches!(level, ConfirmationLevel::None));
    }

    #[test]
    fn test_confirmation_level_once() {
        let batch = BatchTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(MockTool::new("mock_safe", "safe")));
        registry.register(Arc::new(ConfirmingMockTool::new("mock_once", ConfirmationLevel::Once)));
        batch.set_registry(Arc::new(registry));

        let params = json!({
            "tool_calls": [
                {"tool": "mock_safe", "parameters": {}},
                {"tool": "mock_once", "parameters": {}}
            ]
        });

        let level = batch.confirmation_level(&params);
        assert!(matches!(level, ConfirmationLevel::Once));
    }

    #[test]
    fn test_confirmation_level_always() {
        let batch = BatchTool::new();
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(ConfirmingMockTool::new("mock_once", ConfirmationLevel::Once)));
        registry
            .register(Arc::new(ConfirmingMockTool::new("mock_always", ConfirmationLevel::Always)));
        batch.set_registry(Arc::new(registry));

        let params = json!({
            "tool_calls": [
                {"tool": "mock_once", "parameters": {}},
                {"tool": "mock_always", "parameters": {}}
            ]
        });

        let level = batch.confirmation_level(&params);
        assert!(matches!(level, ConfirmationLevel::Always));
    }

    #[test]
    fn test_confirmation_level_dangerous() {
        let batch = BatchTool::new();
        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(ConfirmingMockTool::new("mock_always", ConfirmationLevel::Always)));
        registry.register(Arc::new(ConfirmingMockTool::new(
            "mock_dangerous",
            ConfirmationLevel::Dangerous,
        )));
        batch.set_registry(Arc::new(registry));

        let params = json!({
            "tool_calls": [
                {"tool": "mock_always", "parameters": {}},
                {"tool": "mock_dangerous", "parameters": {}}
            ]
        });

        let level = batch.confirmation_level(&params);
        assert!(matches!(level, ConfirmationLevel::Dangerous));
    }

    #[test]
    fn test_confirmation_level_without_registry() {
        let batch = BatchTool::new();
        // Don't set registry

        let params = json!({
            "tool_calls": [
                {"tool": "mock_read", "parameters": {}}
            ]
        });

        let level = batch.confirmation_level(&params);
        assert!(matches!(level, ConfirmationLevel::None));
    }

    // ==================== Output Format Tests ====================

    #[tokio::test]
    async fn test_output_format() {
        let batch = BatchTool::new();
        let registry = create_test_registry();
        batch.set_registry(registry);

        let params = json!({
            "tool_calls": [
                {"tool": "mock_read", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await.expect("execute should succeed");

        // Check output format
        assert!(result.content.contains("## Batch Execution Results"));
        assert!(result.content.contains("**Summary**:"));
        assert!(result.content.contains("### 1. mock_read [OK]"));
        assert!(result.content.contains("```"));
    }

    #[tokio::test]
    async fn test_output_truncation() {
        let batch = BatchTool::new();
        let mut registry = ToolRegistry::new();
        // Create a tool with very long output
        let long_output = "x".repeat(3000);
        registry.register(Arc::new(MockTool::new("mock_long", &long_output)));
        batch.set_registry(Arc::new(registry));

        let params = json!({
            "tool_calls": [
                {"tool": "mock_long", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await.expect("execute should succeed");

        // Output should be truncated
        assert!(result.content.contains("[Output truncated"));
        assert!(result.content.contains("3000 bytes total"));
    }

    // ==================== Parallel Execution Tests ====================

    #[tokio::test]
    async fn test_parallel_execution() {
        let batch = BatchTool::new();
        let mut registry = ToolRegistry::new();
        // Create tools with delays
        registry.register(Arc::new(MockTool::new("mock_fast1", "fast1").with_delay(10)));
        registry.register(Arc::new(MockTool::new("mock_fast2", "fast2").with_delay(10)));
        registry.register(Arc::new(MockTool::new("mock_fast3", "fast3").with_delay(10)));
        batch.set_registry(Arc::new(registry));

        let params = json!({
            "tool_calls": [
                {"tool": "mock_fast1", "parameters": {}},
                {"tool": "mock_fast2", "parameters": {}},
                {"tool": "mock_fast3", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let start = std::time::Instant::now();
        let result = batch.execute(params, &ctx).await.expect("execute should succeed");
        let elapsed = start.elapsed();

        // If executed in parallel, should take ~10ms, not ~30ms
        // Allow some margin for test environment variability
        assert!(elapsed.as_millis() < 100, "Execution took too long: {:?}", elapsed);
        assert!(!result.is_error);
        assert!(result.content.contains("3/3 tools executed successfully"));
    }

    // ==================== Data Structure Tests ====================

    #[tokio::test]
    async fn test_result_data_structure() {
        let batch = BatchTool::new();
        let registry = create_test_registry();
        batch.set_registry(registry);

        let params = json!({
            "tool_calls": [
                {"tool": "mock_read", "parameters": {}}
            ]
        });

        let ctx = ToolContext::default();
        let result = batch.execute(params, &ctx).await.expect("execute should succeed");

        let data = result.data.expect("should have data");
        assert!(data["total"].is_number());
        assert!(data["successful"].is_number());
        assert!(data["failed"].is_number());
        assert!(data["results"].is_array());

        let results = data["results"].as_array().expect("results array");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["tool"], "mock_read");
        assert_eq!(results[0]["success"], true);
        assert!(results[0]["output"].is_string());
    }
}
