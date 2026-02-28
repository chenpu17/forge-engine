//! Integration tests for plugin/tool registration.
//!
//! Tests registering custom tools, listing tools, and tool execution.

use async_trait::async_trait;
use forge_sdk::ForgeSDKBuilder;
use forge_tools::{Tool, ToolContext, ToolOutput, ToolRegistry};
use serde_json::{json, Value};
use std::sync::Arc;

/// A simple custom tool for testing registration.
struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo_test"
    }

    fn description(&self) -> &str {
        "Echoes back the input message (test tool)"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Message to echo"
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(
        &self,
        params: Value,
        _ctx: &dyn forge_domain::ToolExecutionContext,
    ) -> Result<ToolOutput, forge_domain::ToolError> {
        let message = params
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("(empty)");
        Ok(ToolOutput::success(format!("Echo: {message}")))
    }

    fn is_readonly(&self) -> bool {
        true
    }
}

/// A second custom tool for testing multiple registrations.
struct CounterTool;

#[async_trait]
impl Tool for CounterTool {
    fn name(&self) -> &str {
        "counter_test"
    }

    fn description(&self) -> &str {
        "Returns a count (test tool)"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "count": {
                    "type": "integer",
                    "description": "Number to return"
                }
            },
            "required": ["count"]
        })
    }

    async fn execute(
        &self,
        params: Value,
        _ctx: &dyn forge_domain::ToolExecutionContext,
    ) -> Result<ToolOutput, forge_domain::ToolError> {
        let count = params
            .get("count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        Ok(ToolOutput::success(format!("Count: {count}")))
    }
}

#[test]
fn tool_registry_register_and_get() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool));

    assert!(registry.get("echo_test").is_some());
    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn tool_registry_list_names() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool));
    registry.register(Arc::new(CounterTool));

    let names = registry.list_names();
    assert!(names.contains(&"echo_test"));
    assert!(names.contains(&"counter_test"));
    assert_eq!(names.len(), 2);
}

#[test]
fn tool_registry_unregister() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool));
    assert!(registry.get("echo_test").is_some());

    let removed = registry.unregister("echo_test");
    assert!(removed);
    assert!(registry.get("echo_test").is_none());

    // Unregistering again returns false
    let removed_again = registry.unregister("echo_test");
    assert!(!removed_again);
}

#[test]
fn tool_registry_all_defs() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(EchoTool));
    registry.register(Arc::new(CounterTool));

    let defs = registry.all_defs();
    assert_eq!(defs.len(), 2);

    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"echo_test"));
    assert!(names.contains(&"counter_test"));
}

#[tokio::test]
async fn tool_execute_with_context() {
    let tool = EchoTool;
    let ctx = ToolContext::default();
    let params = json!({"message": "hello world"});

    let result = tool.execute(params, &ctx).await.expect("execute");
    assert!(!result.is_error);
    assert_eq!(result.content, "Echo: hello world");
}

#[tokio::test]
async fn tool_execute_counter() {
    let tool = CounterTool;
    let ctx = ToolContext::default();
    let params = json!({"count": 42});

    let result = tool.execute(params, &ctx).await.expect("execute");
    assert!(!result.is_error);
    assert_eq!(result.content, "Count: 42");
}

#[tokio::test]
async fn sdk_register_custom_tool() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let mock_provider = Arc::new(forge_agent::MockLlmProvider::new());

    let sdk = ForgeSDKBuilder::new()
        .working_dir(tmp.path())
        .provider(mock_provider)
        .model("mock-model")
        .build()
        .expect("build SDK");

    // Register custom tool
    sdk.register_tool(Arc::new(EchoTool)).await;

    let tools = sdk.list_tools().await;
    assert!(
        tools.contains(&"echo_test".to_string()),
        "registered tool should appear in list"
    );
}

#[tokio::test]
async fn sdk_register_multiple_custom_tools() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let mock_provider = Arc::new(forge_agent::MockLlmProvider::new());

    let sdk = ForgeSDKBuilder::new()
        .working_dir(tmp.path())
        .provider(mock_provider)
        .model("mock-model")
        .build()
        .expect("build SDK");

    sdk.register_tool(Arc::new(EchoTool)).await;
    sdk.register_tool(Arc::new(CounterTool)).await;

    let tools = sdk.list_tools().await;
    assert!(tools.contains(&"echo_test".to_string()));
    assert!(tools.contains(&"counter_test".to_string()));
}

#[tokio::test]
async fn sdk_unregister_tool() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let mock_provider = Arc::new(forge_agent::MockLlmProvider::new());

    let sdk = ForgeSDKBuilder::new()
        .working_dir(tmp.path())
        .provider(mock_provider)
        .model("mock-model")
        .build()
        .expect("build SDK");

    sdk.register_tool(Arc::new(EchoTool)).await;
    assert!(sdk.list_tools().await.contains(&"echo_test".to_string()));

    let removed = sdk.unregister_tool("echo_test").await;
    assert!(removed);
    assert!(!sdk.list_tools().await.contains(&"echo_test".to_string()));
}

#[tokio::test]
async fn sdk_builder_with_custom_tool() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let work_dir = tmp.path().to_path_buf();

    let sdk = tokio::task::spawn_blocking(move || {
        let mock_provider = Arc::new(forge_agent::MockLlmProvider::new());
        ForgeSDKBuilder::new()
            .working_dir(&work_dir)
            .provider(mock_provider)
            .model("mock-model")
            .tool(Arc::new(EchoTool))
            .build()
            .expect("build SDK")
    })
    .await
    .expect("spawn_blocking");

    let tools = sdk.list_tools().await;
    assert!(
        tools.contains(&"echo_test".to_string()),
        "tool passed via builder should be registered"
    );
}

#[tokio::test]
async fn sdk_disable_tool() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let work_dir = tmp.path().to_path_buf();

    let sdk = tokio::task::spawn_blocking(move || {
        let mock_provider = Arc::new(forge_agent::MockLlmProvider::new());
        ForgeSDKBuilder::new()
            .working_dir(&work_dir)
            .provider(mock_provider)
            .model("mock-model")
            .with_builtin_tools()
            .build()
            .expect("build SDK")
    })
    .await
    .expect("spawn_blocking");

    // "read" should be in the tool list
    let tools = sdk.list_tools().await;
    assert!(tools.contains(&"read".to_string()));

    // Disable "read"
    let disabled = sdk.get_disabled_tools().await;
    assert!(!disabled.contains(&"read".to_string()));

    // After disabling, the snapshot should exclude it
    sdk.set_disabled_tools(vec!["read".to_string()])
        .await
        .expect("disable tool");
    let snapshot = sdk.tool_registry_snapshot().await;
    assert!(
        snapshot.get("read").is_none(),
        "disabled tool should not appear in snapshot"
    );
}
