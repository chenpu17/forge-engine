//! Integration tests for the General Assistant role.
//!
//! Verifies that the General Assistant persona works without forge-tools-coding,
//! and that basic tools (glob, grep, read, write, edit, bash) are available.

use forge_sdk::ForgeSDKBuilder;
use std::sync::Arc;

/// Helper: build an SDK with builtin tools and a specific persona.
/// Uses `spawn_blocking` because the SDK builder's `build()` calls `block_on`
/// internally for tool registration, which cannot nest inside a tokio runtime.
async fn build_sdk_with_persona(persona: &str) -> forge_sdk::ForgeSDK {
    let persona = persona.to_string();
    let tmp = tempfile::tempdir().expect("create temp dir");
    let work_dir = tmp.path().to_path_buf();

    // Create a prompts dir with an "assistant" persona that disables coding tools
    let prompts_dir = tmp.path().join("prompts");
    let personas_dir = prompts_dir.join("personas");
    let configs_dir = tmp.path().join("configs").join("personas");
    std::fs::create_dir_all(&personas_dir).expect("mkdir personas");
    std::fs::create_dir_all(&configs_dir).expect("mkdir configs");

    // assistant persona prompt
    std::fs::write(
        personas_dir.join("assistant.md"),
        "You are a general assistant. Help users with any task.",
    )
    .expect("write assistant.md");

    // assistant config: disable LSP and symbols tools
    std::fs::write(
        configs_dir.join("assistant.toml"),
        r#"
[persona]
name = "assistant"
description = "General assistant without coding tools"

[templates]
enabled = ["tool_usage"]

[tools]
disabled = ["lsp_diagnostics", "lsp_definition", "lsp_references", "symbols"]

[options]
bash_readonly = false
"#,
    )
    .expect("write assistant.toml");

    // coder persona prompt (needed since it's the default)
    std::fs::write(personas_dir.join("coder.md"), "You are a coding assistant.")
        .expect("write coder.md");

    let prompts_dir_clone = prompts_dir.clone();
    let persona_clone = persona.clone();

    let sdk = tokio::task::spawn_blocking(move || {
        let mock_provider = Arc::new(forge_agent::MockLlmProvider::new());
        ForgeSDKBuilder::new()
            .working_dir(&work_dir)
            .prompts_dir(&prompts_dir_clone)
            .provider(mock_provider)
            .model("mock-model")
            .default_persona(&persona_clone)
            .with_builtin_tools()
            .build()
            .expect("build SDK")
    })
    .await
    .expect("spawn_blocking");

    // Leak the TempDir so it lives long enough
    std::mem::forget(tmp);

    sdk
}

#[tokio::test]
async fn general_assistant_has_basic_tools() {
    let sdk = build_sdk_with_persona("assistant").await;
    let tools = sdk.list_tools().await;

    assert!(tools.contains(&"read".to_string()), "should have read tool");
    assert!(tools.contains(&"write".to_string()), "should have write tool");
    assert!(tools.contains(&"edit".to_string()), "should have edit tool");
    assert!(tools.contains(&"glob".to_string()), "should have glob tool");
    assert!(tools.contains(&"grep".to_string()), "should have grep tool");
}

#[tokio::test]
async fn general_assistant_has_shell_tool() {
    let sdk = build_sdk_with_persona("assistant").await;
    let tools = sdk.list_tools().await;

    let has_shell = tools.iter().any(|t| t == "bash" || t == "powershell" || t == "cmd");
    assert!(has_shell, "should have a shell tool, got: {tools:?}");
}

#[tokio::test]
async fn general_assistant_lsp_tools_disabled_in_snapshot() {
    let sdk = build_sdk_with_persona("assistant").await;

    // Set the disabled tools from persona config
    sdk.set_disabled_tools(vec![
        "lsp_diagnostics".to_string(),
        "lsp_definition".to_string(),
        "lsp_references".to_string(),
        "symbols".to_string(),
    ])
    .await;

    let snapshot = sdk.tool_registry_snapshot().await;
    let snapshot_names = snapshot.list_names();

    assert!(
        !snapshot_names.contains(&"lsp_diagnostics"),
        "lsp_diagnostics should be filtered from snapshot"
    );
    assert!(
        !snapshot_names.contains(&"lsp_definition"),
        "lsp_definition should be filtered from snapshot"
    );
    assert!(
        !snapshot_names.contains(&"lsp_references"),
        "lsp_references should be filtered from snapshot"
    );
    assert!(!snapshot_names.contains(&"symbols"), "symbols should be filtered from snapshot");
}

#[tokio::test]
async fn general_assistant_has_web_tools() {
    let sdk = build_sdk_with_persona("assistant").await;
    let tools = sdk.list_tools().await;

    assert!(tools.contains(&"web_fetch".to_string()), "should have web_fetch tool");
    assert!(tools.contains(&"web_search".to_string()), "should have web_search tool");
}

#[tokio::test]
async fn general_assistant_has_interaction_tools() {
    let sdk = build_sdk_with_persona("assistant").await;
    let tools = sdk.list_tools().await;

    assert!(tools.contains(&"ask_user".to_string()), "should have ask_user tool");
    assert!(tools.contains(&"todo_write".to_string()), "should have todo_write tool");
}

#[tokio::test]
async fn general_assistant_persona_is_set() {
    let sdk = build_sdk_with_persona("assistant").await;
    assert_eq!(sdk.current_persona().await, "assistant");
}
