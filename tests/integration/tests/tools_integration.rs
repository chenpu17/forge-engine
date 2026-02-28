//! Cross-crate integration tests for forge-tools permission system.

use forge_domain::ConfirmationLevel;
use forge_tools::trust_permission::{PermissionCheck, PermissionConfig, PermissionManager};
use serde_json::json;

#[test]
fn permission_manager_workflow() {
    let config = PermissionConfig {
        allowed_patterns: vec![r"^ls\s".to_string(), r"^cat\s".to_string()],
        denied_patterns: vec![r"rm\s+-rf\s+/\s*$".to_string()],
        safe_tools: vec!["read".to_string(), "glob".to_string()],
        dangerous_patterns: vec![r"rm\s+-rf".to_string(), r"sudo\s+".to_string()],
        confirmation_ttl_secs: 300,
        whitelist_rules: vec![],
    };

    let manager = PermissionManager::new(config);

    // Safe tools allowed
    let check = manager.check("read", &json!({"path": "/tmp/file"}), ConfirmationLevel::None);
    assert!(matches!(check, PermissionCheck::Allowed));

    // Dangerous patterns denied
    let check = manager.check("bash", &json!({"command": "rm -rf /"}), ConfirmationLevel::None);
    assert!(matches!(check, PermissionCheck::Denied(_)));
}

#[test]
fn permission_default_safe_tools() {
    let manager = PermissionManager::default();

    let safe_ops = vec![
        ("read", json!({"path": "/etc/hosts"})),
        ("glob", json!({"pattern": "*.rs"})),
        ("grep", json!({"pattern": "test", "path": "/tmp"})),
    ];

    for (tool, params) in safe_ops {
        let check = manager.check(tool, &params, ConfirmationLevel::None);
        assert!(matches!(check, PermissionCheck::Allowed), "Tool '{tool}' should be allowed");
    }
}

#[test]
fn permission_denied_dangerous_commands() {
    let manager = PermissionManager::default();

    let check = manager.check("bash", &json!({"command": "rm -rf /"}), ConfirmationLevel::None);
    assert!(matches!(check, PermissionCheck::Denied(_)));
}
