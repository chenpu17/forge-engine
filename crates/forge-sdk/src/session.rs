//! Session management types for Forge SDK

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Session identifier
pub type SessionId = String;

/// Summary information for a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session ID
    pub id: SessionId,
    /// Session title (auto-generated or user-provided)
    pub title: Option<String>,
    /// Creation time
    pub created_at: DateTime<Utc>,
    /// Last updated time
    pub updated_at: DateTime<Utc>,
    /// Number of messages
    pub message_count: usize,
    /// Total tokens used
    pub total_tokens: usize,
    /// Session tags
    pub tags: Vec<String>,
    /// Working directory for this session
    pub working_dir: PathBuf,
}

impl SessionSummary {
    /// Create a new session summary
    pub fn new(id: impl Into<String>, working_dir: impl Into<PathBuf>) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            title: None,
            created_at: now,
            updated_at: now,
            message_count: 0,
            total_tokens: 0,
            tags: Vec::new(),
            working_dir: working_dir.into(),
        }
    }
}
