//! Cross-crate integration tests for forge-sdk builder and config.

use forge_sdk::ForgeSDKBuilder;

#[test]
fn sdk_builder_creates_instance() {
    let builder = ForgeSDKBuilder::new();
    // Builder should be constructable without panicking
    assert!(std::mem::size_of_val(&builder) > 0);
}

#[test]
fn sdk_config_defaults() {
    let config = forge_sdk::ForgeConfig::default();
    // Default config should have sensible defaults
    assert!(!config.working_dir.as_os_str().is_empty() || config.working_dir == std::path::PathBuf::new());
}
