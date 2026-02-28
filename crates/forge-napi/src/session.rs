//! Session management bindings for NAPI

use napi_derive::napi;

/// Session summary for listing
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSessionSummary {
    pub id: String,
    pub title: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: u32,
    pub total_tokens: u32,
    pub tags: Vec<String>,
    pub working_dir: String,
}

impl From<forge_sdk::SessionSummary> for JsSessionSummary {
    fn from(s: forge_sdk::SessionSummary) -> Self {
        Self {
            id: s.id,
            title: s.title,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
            message_count: s.message_count as u32,
            total_tokens: s.total_tokens as u32,
            tags: s.tags,
            working_dir: s.working_dir.to_string_lossy().to_string(),
        }
    }
}

/// Session status information
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsSessionStatus {
    pub id: String,
    pub message_count: u32,
    pub model: String,
    pub working_dir: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: Option<u32>,
    pub cache_creation_tokens: Option<u32>,
    pub context_limit: u32,
    pub persona: String,
    pub title: Option<String>,
    pub is_dirty: bool,
}

impl From<forge_sdk::SessionStatus> for JsSessionStatus {
    fn from(s: forge_sdk::SessionStatus) -> Self {
        Self {
            id: s.id,
            message_count: s.message_count as u32,
            model: s.model,
            working_dir: s.working_dir.to_string_lossy().to_string(),
            input_tokens: s.token_usage.input_tokens as u32,
            output_tokens: s.token_usage.output_tokens as u32,
            cache_read_tokens: s.token_usage.cache_read_tokens.map(|t| t as u32),
            cache_creation_tokens: s.token_usage.cache_creation_tokens.map(|t| t as u32),
            context_limit: s.context_limit as u32,
            persona: s.persona,
            title: s.title,
            is_dirty: s.is_dirty,
        }
    }
}
