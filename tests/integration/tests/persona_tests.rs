//! Integration tests for persona management.
//!
//! Tests loading, switching, and listing personas through the SDK.

use forge_prompt::PromptManager;
use forge_sdk::ForgeSDKBuilder;
use std::sync::Arc;
use tempfile::TempDir;

/// Helper: create a temp prompts directory with persona files.
fn setup_prompts_dir() -> TempDir {
    let dir = tempfile::tempdir().expect("create temp dir");
    let prompts = dir.path().join("prompts");
    let personas = prompts.join("personas");
    let templates = prompts.join("templates");
    let configs = dir.path().join("configs").join("personas");
    std::fs::create_dir_all(&personas).expect("mkdir personas");
    std::fs::create_dir_all(&templates).expect("mkdir templates");
    std::fs::create_dir_all(&configs).expect("mkdir configs");

    // coder persona
    std::fs::write(
        personas.join("coder.md"),
        "You are a coding assistant. Write clean, tested code.",
    )
    .expect("write coder.md");

    // assistant persona
    std::fs::write(
        personas.join("assistant.md"),
        "You are a general assistant. Help users with any task.",
    )
    .expect("write assistant.md");

    // analyst persona
    std::fs::write(
        personas.join("analyst.md"),
        "You are a data analyst. Analyze data and provide insights.",
    )
    .expect("write analyst.md");

    // coder config with disabled tools
    std::fs::write(
        configs.join("coder.toml"),
        r#"
[persona]
name = "coder"
description = "AI coding assistant"

[templates]
enabled = ["tool_usage"]

[tools]
disabled = []

[options]
bash_readonly = false
reflection_enabled = true
"#,
    )
    .expect("write coder.toml");

    // analyst config with bash_readonly
    std::fs::write(
        configs.join("analyst.toml"),
        r#"
[persona]
name = "analyst"
description = "Data analyst persona"

[templates]
enabled = ["tool_usage"]

[tools]
disabled = ["write", "edit"]

[options]
bash_readonly = true
reflection_enabled = true
max_iterations = 20
"#,
    )
    .expect("write analyst.toml");

    // tool_usage template
    std::fs::write(
        templates.join("tool_usage.md"),
        "## Tool Usage\n\nUse tools wisely and verify results.",
    )
    .expect("write tool_usage.md");

    dir
}

#[test]
fn persona_loading_from_directory() {
    let dir = setup_prompts_dir();
    let prompts_dir = dir.path().join("prompts");

    let manager = PromptManager::from_dir(&prompts_dir).expect("load prompts");

    let personas = manager.list_personas();
    // "coder" is always listed as builtin, plus "analyst" and "assistant"
    assert!(personas.contains(&"coder"), "should contain coder");
    assert!(personas.contains(&"assistant"), "should contain assistant");
    assert!(personas.contains(&"analyst"), "should contain analyst");
}

#[test]
fn persona_default_is_coder() {
    let dir = setup_prompts_dir();
    let prompts_dir = dir.path().join("prompts");

    let manager = PromptManager::from_dir(&prompts_dir).expect("load prompts");
    assert_eq!(manager.current_persona(), "coder");
}

#[test]
fn persona_switching_at_runtime() {
    let dir = setup_prompts_dir();
    let prompts_dir = dir.path().join("prompts");

    let mut manager = PromptManager::from_dir(&prompts_dir).expect("load prompts");

    // Switch to analyst
    manager.set_persona("analyst").expect("switch to analyst");
    assert_eq!(manager.current_persona(), "analyst");

    // Switch to assistant
    manager.set_persona("assistant").expect("switch to assistant");
    assert_eq!(manager.current_persona(), "assistant");

    // Switch back to coder
    manager.set_persona("coder").expect("switch to coder");
    assert_eq!(manager.current_persona(), "coder");
}

#[test]
fn persona_switch_nonexistent_fails() {
    let dir = setup_prompts_dir();
    let prompts_dir = dir.path().join("prompts");

    let mut manager = PromptManager::from_dir(&prompts_dir).expect("load prompts");
    let result = manager.set_persona("nonexistent_persona");
    assert!(result.is_err(), "switching to nonexistent persona should fail");
}

#[test]
fn persona_config_loads_disabled_tools() {
    let dir = setup_prompts_dir();
    let prompts_dir = dir.path().join("prompts");

    let manager = PromptManager::from_dir(&prompts_dir).expect("load prompts");

    // Switch to analyst and check disabled tools
    let mut manager = manager;
    manager.set_persona("analyst").expect("switch to analyst");
    let persona = manager.get_current_persona().expect("analyst persona exists");
    assert!(
        persona.disabled_tools.contains(&"write".to_string()),
        "analyst should have 'write' disabled"
    );
    assert!(
        persona.disabled_tools.contains(&"edit".to_string()),
        "analyst should have 'edit' disabled"
    );
}

#[test]
fn persona_config_loads_options() {
    let dir = setup_prompts_dir();
    let prompts_dir = dir.path().join("prompts");

    let manager = PromptManager::from_dir(&prompts_dir).expect("load prompts");

    let mut manager = manager;
    manager.set_persona("analyst").expect("switch to analyst");
    let persona = manager.get_current_persona().expect("analyst persona exists");
    assert!(persona.options.bash_readonly, "analyst should have bash_readonly");
    assert_eq!(persona.options.max_iterations, Some(20));
    assert!(persona.options.reflection_enabled);
}

#[tokio::test]
async fn sdk_persona_list_and_switch() {
    let dir = setup_prompts_dir();
    let prompts_dir = dir.path().join("prompts");
    let work_dir = dir.path().to_path_buf();

    let mock_provider = Arc::new(forge_agent::MockLlmProvider::new());

    let sdk = ForgeSDKBuilder::new()
        .working_dir(&work_dir)
        .prompts_dir(&prompts_dir)
        .provider(mock_provider)
        .model("mock-model")
        .build()
        .expect("build SDK");

    // List personas
    let personas = sdk.list_personas().await;
    assert!(personas.contains(&"coder".to_string()));

    // Default persona
    let current = sdk.current_persona().await;
    assert_eq!(current, "coder");

    // Switch persona
    sdk.set_persona("assistant").await.expect("switch to assistant");
    assert_eq!(sdk.current_persona().await, "assistant");

    // Switch back
    sdk.set_persona("coder").await.expect("switch to coder");
    assert_eq!(sdk.current_persona().await, "coder");
}

#[test]
fn persona_prompt_content_is_loaded() {
    let dir = setup_prompts_dir();
    let prompts_dir = dir.path().join("prompts");

    let manager = PromptManager::from_dir(&prompts_dir).expect("load prompts");
    let persona = manager.get_current_persona().expect("coder persona exists");
    assert!(
        persona.prompt.contains("coding assistant"),
        "coder prompt should contain 'coding assistant'"
    );
}

#[test]
fn persona_reload_picks_up_changes() {
    let dir = setup_prompts_dir();
    let prompts_dir = dir.path().join("prompts");

    let mut manager = PromptManager::from_dir(&prompts_dir).expect("load prompts");

    // Add a new persona file
    std::fs::write(prompts_dir.join("personas/researcher.md"), "You are a research assistant.")
        .expect("write researcher.md");

    manager.reload().expect("reload prompts");
    let personas = manager.list_personas();
    assert!(personas.contains(&"researcher"), "should contain newly added researcher persona");
}
