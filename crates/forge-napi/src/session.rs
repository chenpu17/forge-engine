//! Session management bindings for NAPI

use napi_derive::napi;

/// Session summary for listing.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSessionSummary {
    /// Session identifier.
    pub id: String,
    /// Session title.
    pub title: Option<String>,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
    /// Last update timestamp (RFC 3339).
    pub updated_at: String,
    /// Number of messages in the session.
    pub message_count: u32,
    /// Total tokens consumed.
    pub total_tokens: u32,
    /// Session tags.
    pub tags: Vec<String>,
    /// Working directory path.
    pub working_dir: String,
}

impl From<forge_sdk::SessionSummary> for JsSessionSummary {
    fn from(s: forge_sdk::SessionSummary) -> Self {
        Self {
            id: s.id,
            title: s.title,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
            message_count: u32::try_from(s.message_count).unwrap_or(u32::MAX),
            total_tokens: u32::try_from(s.total_tokens).unwrap_or(u32::MAX),
            tags: s.tags,
            working_dir: s.working_dir.to_string_lossy().to_string(),
        }
    }
}

/// Session status information.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSessionStatus {
    /// Session identifier.
    pub id: String,
    /// Number of messages in the session.
    pub message_count: u32,
    /// Current model name.
    pub model: String,
    /// Working directory path.
    pub working_dir: String,
    /// Input tokens consumed.
    pub input_tokens: u32,
    /// Output tokens generated.
    pub output_tokens: u32,
    /// Cache read tokens.
    pub cache_read_tokens: Option<u32>,
    /// Cache creation tokens.
    pub cache_creation_tokens: Option<u32>,
    /// Context token limit.
    pub context_limit: u32,
    /// Current persona name.
    pub persona: String,
    /// Session title.
    pub title: Option<String>,
    /// Whether the session has unsaved changes.
    pub is_dirty: bool,
}

impl From<forge_sdk::SessionStatus> for JsSessionStatus {
    fn from(s: forge_sdk::SessionStatus) -> Self {
        Self {
            id: s.id,
            message_count: u32::try_from(s.message_count).unwrap_or(u32::MAX),
            model: s.model,
            working_dir: s.working_dir.to_string_lossy().to_string(),
            input_tokens: u32::try_from(s.token_usage.input_tokens).unwrap_or(u32::MAX),
            output_tokens: u32::try_from(s.token_usage.output_tokens).unwrap_or(u32::MAX),
            cache_read_tokens: s.token_usage.cache_read_tokens.map(|t| u32::try_from(t).unwrap_or(u32::MAX)),
            cache_creation_tokens: s.token_usage.cache_creation_tokens.map(|t| u32::try_from(t).unwrap_or(u32::MAX)),
            context_limit: u32::try_from(s.context_limit).unwrap_or(u32::MAX),
            persona: s.persona,
            title: s.title,
            is_dirty: s.is_dirty,
        }
    }
}
