//! Agent events for Forge SDK
//!
//! Re-exports core event types from `forge-domain` and adds SDK-level
//! convenience helpers.

// Re-export the canonical event type from forge-domain.
pub use forge_domain::AgentEvent;
pub use forge_domain::TodoItem;

use serde::{Deserialize, Serialize};

/// Token usage statistics (SDK-level convenience type).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input tokens used
    pub input_tokens: usize,
    /// Output tokens generated
    pub output_tokens: usize,
    /// Cache read tokens (if supported)
    pub cache_read_tokens: Option<usize>,
    /// Cache creation tokens (if supported)
    pub cache_creation_tokens: Option<usize>,
}

/// Extension methods for [`AgentEvent`].
pub trait AgentEventExt {
    /// Check if this is a terminal event (Done, Cancelled, or Error).
    fn is_terminal(&self) -> bool;
    /// Check if this is an error event.
    fn is_error(&self) -> bool;
}

impl AgentEventExt for AgentEvent {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Done { .. } | Self::Cancelled | Self::Error { .. }
        )
    }

    fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }
}
