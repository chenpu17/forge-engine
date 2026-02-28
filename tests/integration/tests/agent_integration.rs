//! Cross-crate integration tests for forge-agent + forge-domain.

use forge_agent::{ErrorKind, RecoveryAction, Reflector};
use forge_domain::ToolResult;

#[test]
fn reflector_error_recovery_flow() {
    let mut reflector = Reflector::new();

    let network_error = ToolResult::error("1", "Connection refused");
    reflector.record_result(&network_error, "bash");
    let analysis = reflector.analyze(&network_error, "bash");

    assert!(!analysis.success);
    assert_eq!(analysis.error_kind, Some(ErrorKind::NetworkError));
    assert!(matches!(analysis.recovery_action, RecoveryAction::Retry { .. }));

    // Recovery after retry
    let success = ToolResult::success("2", "OK");
    reflector.record_result(&success, "bash");
    assert_eq!(reflector.consecutive_failures(), None);
}

#[test]
fn reflector_consecutive_failure_detection() {
    let mut reflector = Reflector::new();

    for i in 0..5 {
        let error = ToolResult::error(&format!("{i}"), "Unknown error");
        reflector.record_result(&error, "bash");
    }

    assert_eq!(reflector.consecutive_failures(), Some(5));
    assert!(reflector.error_rate() > 0.9);
}

#[test]
fn reflector_pattern_detection() {
    let mut reflector = Reflector::new();
    let msg = "File not found: /path/to/missing.txt";

    for i in 0..4 {
        reflector.record_result(&ToolResult::error(&format!("{i}"), msg), "read");
    }

    assert!(reflector.is_pattern_repeating(msg, "read", 3));
}

#[test]
fn reflector_error_rate_and_stop() {
    let mut reflector = Reflector::new();

    reflector.record_result(&ToolResult::success("1", "ok"), "read");
    reflector.record_result(&ToolResult::success("2", "ok"), "read");
    reflector.record_result(&ToolResult::error("3", "fail"), "bash");
    reflector.record_result(&ToolResult::error("4", "fail"), "bash");

    let rate = reflector.error_rate();
    assert!((rate - 0.5).abs() < 0.001);
    assert!(!reflector.should_stop(5, 0.8));
    assert!(reflector.should_stop(4, 0.4));
}
