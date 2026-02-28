//! Integration tests for the Coder role.
//!
//! Verifies that the Coder persona loads forge-tools-coding tools
//! (LSP tools and symbols tool) and has the full tool set available.

use forge_sdk::ForgeSDKBuilder;
use forge_tools::ToolRegistry;
use forge_tools_coding::register_coding_tools;
use std::sync::Arc;

/// Helper: build SDK with builtin tools from a blocking context.
/// The SDK builder's `build()` uses `block_on` internally for tool registration,
/// which cannot nest inside a tokio runtime. We use `spawn_blocking` to work around this.
async fn build_sdk_with_builtin_tools(
    work_dir: std::path::PathBuf,
) -> forge_sdk::ForgeSDK {
    let sdk = tokio::task::spawn_blocking(move || {
        let mock_provider = Arc::new(forge_agent::MockLlmProvider::new());
        ForgeSDKBuilder::new()
            .working_dir(&work_dir)
            .provider(mock_provider)
            .model("mock-model")
            .default_persona("coder")
            .with_builtin_tools()
            .build()
            .expect("build SDK")
    })
    .await
    .expect("spawn_blocking");
    sdk
}

#[test]
fn coding_tools_register_into_registry() {
    let mut registry = ToolRegistry::new();
    register_coding_tools(&mut registry);

    let names = registry.list_names();
    assert!(
        names.contains(&"lsp_diagnostics"),
        "should register lsp_diagnostics"
    );
    assert!(
        names.contains(&"lsp_definition"),
        "should register lsp_definition"
    );
    assert!(
        names.contains(&"lsp_references"),
        "should register lsp_references"
    );
    assert!(names.contains(&"symbols"), "should register symbols");
    assert_eq!(names.len(), 4, "should register exactly 4 coding tools");
}

#[test]
fn coding_tools_have_valid_defs() {
    let mut registry = ToolRegistry::new();
    register_coding_tools(&mut registry);

    for tool in registry.list_all() {
        let def = tool.to_def();
        assert!(!def.name.is_empty(), "tool name should not be empty");
        assert!(
            !def.description.is_empty(),
            "tool description should not be empty for {}",
            def.name
        );
        assert!(
            def.parameters.is_object(),
            "parameters should be a JSON object for {}",
            def.name
        );
    }
}

#[tokio::test]
async fn coder_role_has_symbols_tool() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let sdk = build_sdk_with_builtin_tools(tmp.path().to_path_buf()).await;

    let tools = sdk.list_tools().await;
    assert!(
        tools.contains(&"symbols".to_string()),
        "coder role should have symbols tool, got: {tools:?}"
    );
}

#[tokio::test]
async fn coder_role_has_file_tools() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let sdk = build_sdk_with_builtin_tools(tmp.path().to_path_buf()).await;

    let tools = sdk.list_tools().await;
    assert!(tools.contains(&"read".to_string()));
    assert!(tools.contains(&"write".to_string()));
    assert!(tools.contains(&"edit".to_string()));
    assert!(tools.contains(&"glob".to_string()));
    assert!(tools.contains(&"grep".to_string()));
}

#[tokio::test]
async fn coder_role_has_shell_tool() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let sdk = build_sdk_with_builtin_tools(tmp.path().to_path_buf()).await;

    let tools = sdk.list_tools().await;
    let has_shell = tools.iter().any(|t| t == "bash" || t == "powershell" || t == "cmd");
    assert!(has_shell, "coder should have shell tool, got: {tools:?}");
}

#[tokio::test]
async fn coder_role_has_web_tools() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let sdk = build_sdk_with_builtin_tools(tmp.path().to_path_buf()).await;

    let tools = sdk.list_tools().await;
    assert!(tools.contains(&"web_fetch".to_string()));
    assert!(tools.contains(&"web_search".to_string()));
}

#[tokio::test]
async fn coder_role_has_plan_mode_tools() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let sdk = build_sdk_with_builtin_tools(tmp.path().to_path_buf()).await;

    let tools = sdk.list_tools().await;
    assert!(tools.contains(&"enter_plan_mode".to_string()));
    assert!(tools.contains(&"exit_plan_mode".to_string()));
}

#[tokio::test]
async fn coder_role_has_memory_tools() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let sdk = build_sdk_with_builtin_tools(tmp.path().to_path_buf()).await;

    let tools = sdk.list_tools().await;
    assert!(tools.contains(&"memory_read".to_string()));
    assert!(tools.contains(&"memory_write".to_string()));
    assert!(tools.contains(&"memory_manage".to_string()));
}

#[tokio::test]
async fn coder_default_persona_is_coder() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let mock_provider = Arc::new(forge_agent::MockLlmProvider::new());

    // No builtin tools, so no block_on conflict
    let sdk = ForgeSDKBuilder::new()
        .working_dir(tmp.path())
        .provider(mock_provider)
        .model("mock-model")
        .build()
        .expect("build SDK");

    assert_eq!(sdk.current_persona().await, "coder");
}

#[tokio::test]
async fn coder_builtin_tools_list_info() {
    let tmp = tempfile::tempdir().expect("create temp dir");
    let sdk = build_sdk_with_builtin_tools(tmp.path().to_path_buf()).await;

    let tool_infos = sdk.list_builtin_tools().await;

    assert!(
        tool_infos.len() >= 10,
        "should have at least 10 builtin tools, got {}",
        tool_infos.len()
    );

    for info in &tool_infos {
        assert!(!info.name.is_empty(), "tool name should not be empty");
        assert!(
            !info.description.is_empty(),
            "tool description should not be empty for {}",
            info.name
        );
    }

    let has_symbols = tool_infos.iter().any(|t| t.name == "symbols");
    assert!(has_symbols, "should have symbols tool in builtin list");
}
