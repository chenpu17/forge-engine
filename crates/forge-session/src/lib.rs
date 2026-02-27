//! Forge Session - Session Management
//!
//! This crate handles session lifecycle, conversation history,
//! context management, and persistence.

pub mod context;
pub mod history;
pub mod manager;
pub mod memory;
pub mod persistence;
pub mod wal;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// Re-exports
pub use context::{
    CompressionResult, CompressionStrategy, ContextConfig, ContextManager, COMPRESSION_PROMPT,
};
pub use history::{InputHistory, InputHistoryItem};
pub use manager::FileSessionManager;
pub use memory::MemorySessionManager;
pub use persistence::{AutoSaver, RecoveryManager, SessionExporter};
pub use wal::WalSessionManager;

/// Session-specific errors
#[derive(Debug, Error)]
pub enum SessionError {
    /// Session not found
    #[error("Session not found: {0}")]
    NotFound(SessionId),

    /// Session already exists
    #[error("Session already exists: {0}")]
    AlreadyExists(SessionId),

    /// Persistence error
    #[error("Persistence error: {0}")]
    PersistenceError(String),

    /// Context overflow
    #[error("Context overflow: {current} tokens exceeds limit of {limit}")]
    ContextOverflow {
        /// Current token count
        current: usize,
        /// Token limit
        limit: usize,
    },
}

impl From<std::io::Error> for SessionError {
    fn from(e: std::io::Error) -> Self {
        Self::PersistenceError(e.to_string())
    }
}

impl From<serde_json::Error> for SessionError {
    fn from(e: serde_json::Error) -> Self {
        Self::PersistenceError(e.to_string())
    }
}

/// Result type for session operations
pub type Result<T> = std::result::Result<T, SessionError>;

/// Unique session identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(Uuid);

impl SessionId {
    /// Create a new random session ID
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Parse a session ID from a string
    ///
    /// # Errors
    /// Returns error if the string is not a valid UUID
    pub fn parse(s: &str) -> std::result::Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for SessionId {
    fn from(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

/// Session configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Maximum context tokens
    pub max_context_tokens: usize,
    /// Model to use
    pub model: String,
    /// System prompt
    pub system_prompt: Option<String>,
    /// Working directory
    pub working_dir: std::path::PathBuf,
}

/// Session persistence format
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPersistenceFormat {
    /// Pretty-printed JSON (human-readable)
    #[default]
    PrettyJson,
    /// Compact JSON (smaller, faster)
    CompactJson,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 200_000,
            model: "claude-sonnet-4-5-20250929".to_string(),
            system_prompt: None,
            working_dir: std::env::current_dir().unwrap_or_default(),
        }
    }
}

/// A session represents a single conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Unique identifier
    pub id: SessionId,
    /// Session configuration
    pub config: SessionConfig,
    /// Conversation messages
    pub messages: Vec<Message>,
    /// Session metadata
    pub metadata: SessionMetadata,
}

impl Session {
    /// Add a message to the session
    pub fn add_message(&mut self, message: Message) {
        let is_tool_result_only = matches!(&message.content, MessageContent::Blocks(blocks) if !blocks.is_empty()
            && blocks.iter().all(|b| matches!(b, ContentBlock::ToolResult { .. })));
        let is_user_turn = message.role == MessageRole::User && !is_tool_result_only;

        self.messages.push(message);
        self.metadata.updated_at = Utc::now();

        // Update turn count for user messages
        if is_user_turn {
            self.metadata.turn_count += 1;
        }

        // Keep token estimate in sync with message content
        self.metadata.total_tokens = ContextManager::estimate_total_tokens(&self.messages);
    }

    /// Get the total number of messages
    #[must_use]
    pub const fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Set the session title
    pub fn set_title(&mut self, title: impl Into<String>) {
        self.metadata.title = Some(title.into());
        self.metadata.updated_at = Utc::now();
    }

    /// Add a tag to the session
    pub fn add_tag(&mut self, tag: impl Into<String>) {
        let tag = tag.into();
        if !self.metadata.tags.contains(&tag) {
            self.metadata.tags.push(tag);
            self.metadata.updated_at = Utc::now();
        }
    }

    /// Remove a tag from the session
    pub fn remove_tag(&mut self, tag: &str) {
        if let Some(pos) = self.metadata.tags.iter().position(|t| t == tag) {
            self.metadata.tags.remove(pos);
            self.metadata.updated_at = Utc::now();
        }
    }

    /// Get a summary of this session
    #[must_use]
    pub fn summary(&self) -> SessionSummary {
        SessionSummary::from_session(self)
    }

    /// Auto-generate title from first user message if not set
    pub fn auto_title(&mut self) {
        if self.metadata.title.is_none() {
            if let Some(msg) = self.messages.iter().find(|m| m.role == MessageRole::User) {
                let text = msg.text();
                let title = if text.chars().count() > 50 {
                    format!("{}...", text.chars().take(50).collect::<String>())
                } else {
                    text
                };
                self.metadata.title = Some(title);
            }
        }
    }

    /// Search messages in this session for a query string
    ///
    /// Returns a list of matching messages with their indices
    #[must_use]
    pub fn search_messages(&self, query: &str) -> Vec<SearchResult> {
        let query_lower = query.to_lowercase();
        self.messages
            .iter()
            .enumerate()
            .filter_map(|(idx, msg)| {
                let text = msg.text();
                let text_lower = text.to_lowercase();
                if text_lower.contains(&query_lower) {
                    // Find the position of the match for context
                    let match_pos = text_lower.find(&query_lower).unwrap_or(0);
                    let context = extract_context(&text, match_pos, query.len(), 50);
                    Some(SearchResult {
                        message_index: idx,
                        role: msg.role,
                        timestamp: msg.timestamp,
                        context,
                        match_count: text_lower.matches(&query_lower).count(),
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// Check if any message in this session contains the query
    #[must_use]
    pub fn contains(&self, query: &str) -> bool {
        let query_lower = query.to_lowercase();
        self.messages.iter().any(|msg| msg.text().to_lowercase().contains(&query_lower))
    }
}

/// Result of searching within a session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Index of the message in the session
    pub message_index: usize,
    /// Role of the message author
    pub role: MessageRole,
    /// Timestamp of the message
    pub timestamp: DateTime<Utc>,
    /// Context around the match (snippet)
    pub context: String,
    /// Number of matches in this message
    pub match_count: usize,
}

/// Extract context around a match position
fn extract_context(text: &str, match_pos: usize, match_len: usize, context_chars: usize) -> String {
    let start = match_pos.saturating_sub(context_chars);
    let end = (match_pos + match_len + context_chars).min(text.len());

    // Find safe UTF-8 boundaries
    let mut safe_start = start;
    while safe_start > 0 && !text.is_char_boundary(safe_start) {
        safe_start -= 1;
    }

    let mut safe_end = end;
    while safe_end < text.len() && !text.is_char_boundary(safe_end) {
        safe_end += 1;
    }

    let snippet = &text[safe_start..safe_end];

    // Add ellipsis if truncated
    let prefix = if safe_start > 0 { "..." } else { "" };
    let suffix = if safe_end < text.len() { "..." } else { "" };

    format!("{}{}{}", prefix, snippet.trim(), suffix)
}

/// Session metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Creation time
    pub created_at: DateTime<Utc>,
    /// Last update time
    pub updated_at: DateTime<Utc>,
    /// Total tokens used
    pub total_tokens: usize,
    /// Number of turns
    pub turn_count: usize,
    /// Session title (auto-generated or user-defined)
    #[serde(default)]
    pub title: Option<String>,
    /// Tags for organization
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Default for SessionMetadata {
    fn default() -> Self {
        let now = Utc::now();
        Self {
            created_at: now,
            updated_at: now,
            total_tokens: 0,
            turn_count: 0,
            title: None,
            tags: Vec::new(),
        }
    }
}

/// Session summary for list display (lightweight version of Session)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session ID
    pub id: SessionId,
    /// Session title
    pub title: Option<String>,
    /// Tags
    pub tags: Vec<String>,
    /// Working directory
    pub working_dir: std::path::PathBuf,
    /// Creation time
    pub created_at: DateTime<Utc>,
    /// Last update time
    pub updated_at: DateTime<Utc>,
    /// Number of messages
    pub message_count: usize,
    /// Number of turns
    pub turn_count: usize,
    /// Estimated total tokens
    #[serde(default)]
    pub total_tokens: usize,
    /// First user message preview (truncated)
    pub preview: Option<String>,
}

impl SessionSummary {
    /// Create a summary from a session
    #[must_use]
    pub fn from_session(session: &Session) -> Self {
        fn truncate_with_ellipsis(text: &str, limit: usize) -> String {
            let mut chars = text.chars();
            let preview: String = chars.by_ref().take(limit).collect();
            if chars.next().is_some() {
                format!("{preview}...")
            } else {
                preview
            }
        }

        // Get first user message as preview
        let preview = session.messages.iter().find(|m| m.role == MessageRole::User).map(|m| {
            let text = m.text();
            truncate_with_ellipsis(&text, 100)
        });

        // Estimate total tokens from messages
        let total_tokens = ContextManager::estimate_total_tokens(&session.messages);

        Self {
            id: session.id,
            title: session.metadata.title.clone(),
            tags: session.metadata.tags.clone(),
            working_dir: session.config.working_dir.clone(),
            created_at: session.metadata.created_at,
            updated_at: session.metadata.updated_at,
            message_count: session.messages.len(),
            turn_count: session.metadata.turn_count,
            total_tokens,
            preview,
        }
    }

    /// Get display title (title or preview or default)
    #[must_use]
    pub fn display_title(&self) -> String {
        self.title
            .clone()
            .or_else(|| self.preview.clone())
            .unwrap_or_else(|| format!("Session {}", &self.id.to_string()[..8]))
    }
}

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Message role
    pub role: MessageRole,
    /// Message content
    pub content: MessageContent,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
}

impl Message {
    /// Create a user message
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: MessageContent::Text(content.into()),
            timestamp: Utc::now(),
        }
    }

    /// Create an assistant message
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: MessageContent::Text(content.into()),
            timestamp: Utc::now(),
        }
    }

    /// Create a system message
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: MessageContent::Text(content.into()),
            timestamp: Utc::now(),
        }
    }

    /// Get the text content of the message
    #[must_use]
    pub fn text(&self) -> String {
        self.content.text()
    }
}

/// Message role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    /// User message
    User,
    /// Assistant message
    Assistant,
    /// System message
    System,
}

/// Message content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Simple text content
    Text(String),
    /// Structured content blocks
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// Extract text content from the message
    ///
    /// For Text variant, returns the text directly.
    /// For Blocks variant, concatenates all text blocks.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::Text { text } = b {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// Content block types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text block
    Text {
        /// Text content
        text: String,
    },
    /// Tool use block
    ToolUse {
        /// Tool use ID
        id: String,
        /// Tool name
        name: String,
        /// Tool input
        input: serde_json::Value,
    },
    /// Tool result block
    ToolResult {
        /// ID of the tool use this result is for
        tool_use_id: String,
        /// Result content
        content: String,
        /// Whether the tool execution resulted in an error
        is_error: bool,
    },
}

/// Session manager trait
#[async_trait::async_trait]
pub trait SessionManager: Send + Sync {
    /// Create a new session
    async fn create(&self, config: SessionConfig) -> Result<Session>;

    /// Get a session by ID
    async fn get(&self, id: SessionId) -> Result<Session>;

    /// Update a session
    async fn update(&self, session: &Session) -> Result<()>;

    /// Delete a session
    async fn delete(&self, id: SessionId) -> Result<()>;

    /// List all sessions
    async fn list(&self) -> Result<Vec<SessionId>>;

    /// Get the most recent session
    async fn latest(&self) -> Result<Option<Session>>;

    /// List all sessions with summaries, sorted by `updated_at` descending
    async fn list_summaries(&self) -> Result<Vec<SessionSummary>> {
        let ids = self.list().await?;
        let mut summaries = Vec::with_capacity(ids.len());
        for id in ids {
            if let Ok(session) = self.get(id).await {
                summaries.push(session.summary());
            }
        }
        // Sort by updated_at descending (most recent first)
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(summaries)
    }

    /// Search sessions by query (searches title, tags, preview)
    async fn search(&self, query: &str) -> Result<Vec<SessionSummary>> {
        let query = query.to_lowercase();
        let summaries = self.list_summaries().await?;
        Ok(summaries
            .into_iter()
            .filter(|s| {
                // Search in title
                s.title
                    .as_ref()
                    .is_some_and(|t| t.to_lowercase().contains(&query))
                    // Search in tags
                    || s.tags.iter().any(|t| t.to_lowercase().contains(&query))
                    // Search in preview
                    || s.preview
                        .as_ref()
                        .is_some_and(|p| p.to_lowercase().contains(&query))
                    // Search in working directory
                    || s.working_dir
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&query)
            })
            .collect())
    }

    /// Find sessions by tag
    async fn find_by_tag(&self, tag: &str) -> Result<Vec<SessionSummary>> {
        let tag = tag.to_lowercase();
        let summaries = self.list_summaries().await?;
        Ok(summaries
            .into_iter()
            .filter(|s| s.tags.iter().any(|t| t.to_lowercase() == tag))
            .collect())
    }

    /// Find sessions by working directory
    async fn find_by_dir(&self, dir: &std::path::Path) -> Result<Vec<SessionSummary>> {
        let summaries = self.list_summaries().await?;
        Ok(summaries.into_iter().filter(|s| s.working_dir.starts_with(dir)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_id_new() {
        let id1 = SessionId::new();
        let id2 = SessionId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_session_id_parse() {
        let id = SessionId::new();
        let parsed = SessionId::parse(&id.to_string()).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_message_user() {
        let msg = Message::user("hello");
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.text(), "hello");
    }

    #[test]
    fn test_message_text_from_blocks() {
        let msg = Message {
            role: MessageRole::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text { text: "Hello ".to_string() },
                ContentBlock::Text { text: "World".to_string() },
            ]),
            timestamp: Utc::now(),
        };
        assert_eq!(msg.text(), "Hello \nWorld");
    }

    #[test]
    fn test_session_add_message() {
        let mut session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        session.add_message(Message::user("test"));
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.metadata.turn_count, 1);
        assert_eq!(
            session.metadata.total_tokens,
            ContextManager::estimate_total_tokens(&session.messages)
        );
    }

    #[test]
    fn test_session_add_tool_result_does_not_increment_turns() {
        let mut session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        let tool_result_message = Message {
            role: MessageRole::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                content: "ok".to_string(),
                is_error: false,
            }]),
            timestamp: Utc::now(),
        };

        session.add_message(tool_result_message);
        assert_eq!(session.metadata.turn_count, 0);
    }

    #[test]
    fn test_session_title_and_tags() {
        let mut session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        // Set title
        session.set_title("My Test Session");
        assert_eq!(session.metadata.title, Some("My Test Session".to_string()));

        // Add tags
        session.add_tag("rust");
        session.add_tag("test");
        assert_eq!(session.metadata.tags, vec!["rust", "test"]);

        // Duplicate tag should not be added
        session.add_tag("rust");
        assert_eq!(session.metadata.tags.len(), 2);

        // Remove tag
        session.remove_tag("test");
        assert_eq!(session.metadata.tags, vec!["rust"]);
    }

    #[test]
    fn test_session_auto_title() {
        let mut session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        // No auto-title without messages
        session.auto_title();
        assert!(session.metadata.title.is_none());

        // Add user message and auto-title
        session.add_message(Message::user("Help me write a function"));
        session.auto_title();
        assert_eq!(session.metadata.title, Some("Help me write a function".to_string()));

        // Don't override existing title
        session.set_title("Custom Title");
        session.add_message(Message::user("Another message"));
        session.auto_title();
        assert_eq!(session.metadata.title, Some("Custom Title".to_string()));
    }

    #[test]
    fn test_session_summary() {
        let mut session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        session.set_title("Test Session");
        session.add_tag("test");
        session.add_message(Message::user("Hello world"));
        session.add_message(Message::assistant("Hi there!"));

        let summary = session.summary();
        assert_eq!(summary.id, session.id);
        assert_eq!(summary.title, Some("Test Session".to_string()));
        assert_eq!(summary.tags, vec!["test"]);
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.turn_count, 1);
        assert_eq!(summary.preview, Some("Hello world".to_string()));
    }

    #[test]
    fn test_session_summary_display_title() {
        let session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: vec![Message::user("What is Rust?")],
            metadata: SessionMetadata::default(),
        };

        let summary = session.summary();
        // Without title, should show preview
        assert_eq!(summary.display_title(), "What is Rust?");
    }

    #[test]
    fn test_session_search_messages() {
        let mut session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        session.add_message(Message::user("How do I implement a function in Rust?"));
        session.add_message(Message::assistant("Here's how to implement a function in Rust..."));
        session.add_message(Message::user("What about async functions?"));
        session.add_message(Message::assistant("Async functions use the async keyword."));

        // Search for "function" - appears in all 4 messages
        let results = session.search_messages("function");
        assert_eq!(results.len(), 4);

        // Verify first result
        assert_eq!(results[0].message_index, 0);
        assert_eq!(results[0].role, MessageRole::User);
        assert!(results[0].match_count >= 1);

        // Search for "Rust" (case insensitive)
        let results = session.search_messages("rust");
        assert_eq!(results.len(), 2);

        // Search for non-existent term
        let results = session.search_messages("python");
        assert!(results.is_empty());
    }

    #[test]
    fn test_session_contains() {
        let mut session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        session.add_message(Message::user("Hello world"));
        session.add_message(Message::assistant("Hi there!"));

        // Case insensitive search
        assert!(session.contains("hello"));
        assert!(session.contains("HELLO"));
        assert!(session.contains("world"));
        assert!(session.contains("there"));
        assert!(!session.contains("goodbye"));
    }

    #[test]
    fn test_extract_context() {
        // Test basic context extraction
        let text = "This is a long text with the keyword somewhere in the middle of it.";
        let context = extract_context(text, 27, 7, 10); // "keyword" at position 27
        assert!(context.contains("keyword"));
        assert!(context.starts_with("...") || context.len() < text.len());

        // Test at beginning
        let context = extract_context(text, 0, 4, 10); // "This" at position 0
        assert!(context.contains("This"));
        assert!(!context.starts_with("..."));

        // Test at end
        let text = "Short text here";
        let context = extract_context(text, 11, 4, 10); // "here" at position 11
        assert!(context.contains("here"));
    }

    #[test]
    fn test_extract_context_utf8_safety() {
        // Test with UTF-8 characters
        let text = "你好世界，这是一个测试文本";
        let context = extract_context(text, 0, 6, 10); // "你好" (6 bytes)
        assert!(context.contains("你好"));

        // Test with mixed content
        let text = "Hello 世界 World";
        let context = extract_context(text, 6, 6, 5); // "世界" at position 6
        assert!(context.contains("世界"));
    }

    #[test]
    fn test_search_result_context() {
        let mut session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        session.add_message(Message::user(
            "This is a very long message that contains the word error somewhere in the middle of it."
        ));

        let results = session.search_messages("error");
        assert_eq!(results.len(), 1);

        // Context should contain the match and surrounding text
        let context = &results[0].context;
        assert!(context.contains("error"));
        // Context should be truncated (not the full message)
        assert!(context.len() < 100);
    }
}
