//! Trust level system for tool permission management
//!
//! This module provides a 4-level trust system that controls how tool
//! permissions are checked, from cautious (all writes need confirmation)
//! to yolo (skip all confirmations except hardcoded safety).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::ConfirmationLevel;

/// Trust level for tool execution
///
/// Higher levels provide more convenience but less security.
/// All levels are still subject to the hardcoded safety layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    /// Cautious mode - all write operations need confirmation
    #[default]
    Cautious = 0,
    /// Development mode - project-internal operations auto-allowed
    Development = 1,
    /// Trusted mode - only dangerous commands need confirmation
    Trusted = 2,
    /// Yolo mode - skip all confirmations (hardcoded safety still applies)
    Yolo = 3,
}

impl TrustLevel {
    /// Get human-readable name
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Cautious => "Cautious",
            Self::Development => "Development",
            Self::Trusted => "Trusted",
            Self::Yolo => "Yolo",
        }
    }

    /// Get description
    #[must_use]
    pub const fn description(&self) -> &'static str {
        match self {
            Self::Cautious => "All write operations need confirmation",
            Self::Development => "Project-internal operations auto-allowed",
            Self::Trusted => "Only dangerous commands need confirmation",
            Self::Yolo => "Skip all confirmations (safety layer still applies)",
        }
    }
}

/// Reason for hard blocking an operation
///
/// Hard blocks cannot be bypassed even in Yolo mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HardBlockReason {
    /// Attempted to access a system-protected path
    SystemPath {
        /// The path that was blocked
        path: PathBuf,
        /// Human-readable description
        description: String,
    },
    /// Attempted to execute a destructive command
    DestructiveCommand {
        /// The command that was blocked
        command: String,
        /// The pattern that matched
        pattern: String,
    },
    /// Attempted a mass operation exceeding threshold
    MassOperation {
        /// Type of operation (e.g., "delete", "write")
        operation: String,
        /// Number of items affected
        count: usize,
        /// Threshold that was exceeded
        threshold: usize,
    },
    /// Attempted remote code execution
    RemoteCodeExecution {
        /// The command that was blocked
        command: String,
    },
}

impl std::fmt::Display for HardBlockReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SystemPath { path, description } => {
                write!(f, "System path blocked: {} - {}", path.display(), description)
            }
            Self::DestructiveCommand { command, pattern } => {
                write!(f, "Destructive command blocked: {command} (matched: {pattern})")
            }
            Self::MassOperation { operation, count, threshold } => {
                write!(
                    f,
                    "Mass {operation} blocked: {count} items exceeds threshold of {threshold}"
                )
            }
            Self::RemoteCodeExecution { command } => {
                write!(f, "Remote code execution blocked: {command}")
            }
        }
    }
}

/// Extended permission check result
///
/// This extends the existing `PermissionCheck` with `HardBlocked` variant
/// for operations that cannot be bypassed even with user confirmation.
#[derive(Debug, Clone)]
pub enum PermissionCheckResult {
    /// Operation is allowed
    Allowed,
    /// Operation needs user confirmation
    NeedsConfirmation {
        /// Confirmation level required
        level: ConfirmationLevel,
        /// Optional reason for confirmation
        reason: Option<String>,
    },
    /// Operation is denied by policy (can be overridden by user)
    Denied {
        /// Reason for denial
        reason: String,
    },
    /// Operation is hard blocked (cannot be bypassed)
    HardBlocked {
        /// Reason for hard block
        reason: HardBlockReason,
    },
}

impl PermissionCheckResult {
    /// Check if the operation is allowed
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// Check if the operation is hard blocked
    #[must_use]
    pub const fn is_hard_blocked(&self) -> bool {
        matches!(self, Self::HardBlocked { .. })
    }
}

use crate::permission::PermissionCheck;

/// Convert from existing `PermissionCheck` to `PermissionCheckResult`
impl From<PermissionCheck> for PermissionCheckResult {
    fn from(check: PermissionCheck) -> Self {
        match check {
            PermissionCheck::Allowed => Self::Allowed,
            PermissionCheck::NeedsConfirmation(level) => {
                Self::NeedsConfirmation { level, reason: None }
            }
            PermissionCheck::Denied(reason) => Self::Denied { reason },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trust_level_ordering() {
        assert!(TrustLevel::Cautious < TrustLevel::Development);
        assert!(TrustLevel::Development < TrustLevel::Trusted);
        assert!(TrustLevel::Trusted < TrustLevel::Yolo);
    }

    #[test]
    fn test_trust_level_default() {
        assert_eq!(TrustLevel::default(), TrustLevel::Cautious);
    }

    #[test]
    fn test_trust_level_serialization() {
        let level = TrustLevel::Development;
        let json = serde_json::to_string(&level).unwrap();
        assert_eq!(json, "\"development\"");

        let parsed: TrustLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, level);
    }

    #[test]
    fn test_hard_block_reason_display() {
        let reason = HardBlockReason::SystemPath {
            path: PathBuf::from("/usr/bin"),
            description: "System binary directory".to_string(),
        };
        let display = format!("{}", reason);
        assert!(display.contains("/usr/bin"));
    }

    #[test]
    fn test_permission_check_result_from() {
        let check = PermissionCheck::Allowed;
        let result: PermissionCheckResult = check.into();
        assert!(result.is_allowed());

        let check = PermissionCheck::NeedsConfirmation(ConfirmationLevel::Once);
        let result: PermissionCheckResult = check.into();
        assert!(matches!(result, PermissionCheckResult::NeedsConfirmation { .. }));
    }
}
