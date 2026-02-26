//! Memory configuration types.

use serde::{Deserialize, Serialize};

/// Memory mode — controls read/write access to the memory system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMode {
    /// Full read/write access (default).
    #[default]
    Full,
    /// Read-only: can read memory but not write/delete/move.
    ReadOnly,
    /// Memory system completely disabled.
    Off,
    /// Temporary: memory is available in-session but not persisted.
    Temporary,
}

/// Memory settings with layered override (global → workspace → session).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySettings {
    /// Global default mode.
    #[serde(default)]
    pub global_mode: MemoryMode,
    /// Workspace-level override (if set, takes precedence over global).
    #[serde(default)]
    pub workspace_mode: Option<MemoryMode>,
    /// Session-level override (not persisted, highest priority).
    #[serde(skip)]
    pub session_mode: Option<MemoryMode>,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            global_mode: MemoryMode::Full,
            workspace_mode: None,
            session_mode: None,
        }
    }
}

impl MemorySettings {
    /// Resolve the effective mode: session > workspace > global.
    #[must_use]
    pub fn effective_mode(&self) -> MemoryMode {
        self.session_mode
            .or(self.workspace_mode)
            .unwrap_or(self.global_mode)
    }

    /// Whether reading memory is allowed in the current mode.
    #[must_use]
    pub fn can_read(&self) -> bool {
        matches!(
            self.effective_mode(),
            MemoryMode::Full | MemoryMode::ReadOnly
        )
    }

    /// Whether writing memory is allowed in the current mode.
    #[must_use]
    pub fn can_write(&self) -> bool {
        matches!(self.effective_mode(), MemoryMode::Full)
    }
}
