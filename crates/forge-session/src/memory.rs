//! In-memory session manager for testing
//!
//! Provides a memory-based session storage that doesn't persist to disk.
//! Ideal for unit tests and integration tests that need fast, isolated sessions.

use crate::{
    Result, Session, SessionConfig, SessionError, SessionId, SessionManager, SessionMetadata,
};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

/// In-memory session manager for testing
///
/// This manager stores all sessions in memory without any file I/O.
/// It's ideal for unit tests where persistence is not required.
///
/// # Example
///
/// ```rust
/// use forge_session::{MemorySessionManager, SessionConfig, SessionManager};
///
/// #[tokio::main]
/// async fn main() {
///     let manager = MemorySessionManager::new();
///     let session = manager.create(SessionConfig::default()).await.unwrap();
///     println!("Created session: {}", session.id);
/// }
/// ```
pub struct MemorySessionManager {
    /// In-memory session storage
    sessions: RwLock<HashMap<SessionId, Session>>,
    /// Current active session ID
    current: RwLock<Option<SessionId>>,
}

impl MemorySessionManager {
    /// Create a new in-memory session manager
    #[must_use]
    pub fn new() -> Self {
        Self { sessions: RwLock::new(HashMap::new()), current: RwLock::new(None) }
    }

    /// Get the current active session ID
    pub async fn current_id(&self) -> Option<SessionId> {
        *self.current.read().await
    }

    /// Set the current active session
    pub async fn set_current(&self, id: SessionId) {
        *self.current.write().await = Some(id);
    }

    /// Clear the current active session
    pub async fn clear_current(&self) {
        *self.current.write().await = None;
    }

    /// Get the number of stored sessions
    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Check if a session exists
    pub async fn exists(&self, id: SessionId) -> bool {
        self.sessions.read().await.contains_key(&id)
    }

    /// Clear all sessions (useful for test cleanup)
    pub async fn clear_all(&self) {
        self.sessions.write().await.clear();
        *self.current.write().await = None;
    }
}

impl Default for MemorySessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SessionManager for MemorySessionManager {
    async fn create(&self, config: SessionConfig) -> Result<Session> {
        let session = Session {
            id: SessionId::new(),
            config,
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        // Store in memory
        self.sessions.write().await.insert(session.id, session.clone());

        // Set as current
        *self.current.write().await = Some(session.id);

        tracing::debug!(session_id = %session.id, "Created new in-memory session");

        Ok(session)
    }

    async fn get(&self, id: SessionId) -> Result<Session> {
        self.sessions.read().await.get(&id).cloned().ok_or(SessionError::NotFound(id))
    }

    async fn update(&self, session: &Session) -> Result<()> {
        let mut sessions = self.sessions.write().await;

        // Check if session exists
        if !sessions.contains_key(&session.id) {
            return Err(SessionError::NotFound(session.id));
        }

        sessions.insert(session.id, session.clone());
        drop(sessions);

        tracing::debug!(session_id = %session.id, "Updated in-memory session");

        Ok(())
    }

    async fn delete(&self, id: SessionId) -> Result<()> {
        let mut sessions = self.sessions.write().await;

        if sessions.remove(&id).is_none() {
            return Err(SessionError::NotFound(id));
        }
        drop(sessions);

        // Clear current if this was the active session
        let mut current = self.current.write().await;
        if *current == Some(id) {
            *current = None;
        }
        drop(current);

        tracing::debug!(session_id = %id, "Deleted in-memory session");

        Ok(())
    }

    async fn list(&self) -> Result<Vec<SessionId>> {
        Ok(self.sessions.read().await.keys().copied().collect())
    }

    async fn latest(&self) -> Result<Option<Session>> {
        let sessions = self.sessions.read().await;

        Ok(sessions.values().max_by_key(|s| s.metadata.updated_at).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;

    #[tokio::test]
    async fn test_create_session() {
        let manager = MemorySessionManager::new();

        let session = manager.create(SessionConfig::default()).await.unwrap();
        assert!(session.messages.is_empty());
        assert_eq!(session.metadata.turn_count, 0);

        // Should be set as current
        assert_eq!(manager.current_id().await, Some(session.id));
    }

    #[tokio::test]
    async fn test_get_session() {
        let manager = MemorySessionManager::new();

        let created = manager.create(SessionConfig::default()).await.unwrap();
        let fetched = manager.get(created.id).await.unwrap();

        assert_eq!(created.id, fetched.id);
    }

    #[tokio::test]
    async fn test_get_nonexistent_session() {
        let manager = MemorySessionManager::new();

        let result = manager.get(SessionId::new()).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(SessionError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_update_session() {
        let manager = MemorySessionManager::new();

        let mut session = manager.create(SessionConfig::default()).await.unwrap();
        session.add_message(Message::user("test"));

        manager.update(&session).await.unwrap();

        let fetched = manager.get(session.id).await.unwrap();
        assert_eq!(fetched.messages.len(), 1);
        assert_eq!(fetched.metadata.turn_count, 1);
    }

    #[tokio::test]
    async fn test_update_nonexistent_session() {
        let manager = MemorySessionManager::new();

        let session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        let result = manager.update(&session).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(SessionError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_delete_session() {
        let manager = MemorySessionManager::new();

        let session = manager.create(SessionConfig::default()).await.unwrap();
        let id = session.id;

        manager.delete(id).await.unwrap();

        let result = manager.get(id).await;
        assert!(result.is_err());

        // Current should be cleared
        assert!(manager.current_id().await.is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent_session() {
        let manager = MemorySessionManager::new();

        let result = manager.delete(SessionId::new()).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(SessionError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let manager = MemorySessionManager::new();

        manager.create(SessionConfig::default()).await.unwrap();
        manager.create(SessionConfig::default()).await.unwrap();
        manager.create(SessionConfig::default()).await.unwrap();

        let list = manager.list().await.unwrap();
        assert_eq!(list.len(), 3);
    }

    #[tokio::test]
    async fn test_latest_session() {
        let manager = MemorySessionManager::new();

        let session1 = manager.create(SessionConfig::default()).await.unwrap();

        // Create and update second session to make it latest
        let mut session2 = manager.create(SessionConfig::default()).await.unwrap();
        session2.add_message(Message::user("test"));
        manager.update(&session2).await.unwrap();

        let latest = manager.latest().await.unwrap();
        assert!(latest.is_some());

        // session2 should be latest because it was updated more recently
        let latest = latest.unwrap();
        assert_eq!(latest.id, session2.id);
        assert_ne!(latest.id, session1.id);
    }

    #[tokio::test]
    async fn test_latest_empty() {
        let manager = MemorySessionManager::new();

        let latest = manager.latest().await.unwrap();
        assert!(latest.is_none());
    }

    #[tokio::test]
    async fn test_session_count() {
        let manager = MemorySessionManager::new();

        assert_eq!(manager.session_count().await, 0);

        manager.create(SessionConfig::default()).await.unwrap();
        assert_eq!(manager.session_count().await, 1);

        manager.create(SessionConfig::default()).await.unwrap();
        assert_eq!(manager.session_count().await, 2);
    }

    #[tokio::test]
    async fn test_exists() {
        let manager = MemorySessionManager::new();

        let session = manager.create(SessionConfig::default()).await.unwrap();

        assert!(manager.exists(session.id).await);
        assert!(!manager.exists(SessionId::new()).await);
    }

    #[tokio::test]
    async fn test_clear_all() {
        let manager = MemorySessionManager::new();

        manager.create(SessionConfig::default()).await.unwrap();
        manager.create(SessionConfig::default()).await.unwrap();

        assert_eq!(manager.session_count().await, 2);
        assert!(manager.current_id().await.is_some());

        manager.clear_all().await;

        assert_eq!(manager.session_count().await, 0);
        assert!(manager.current_id().await.is_none());
    }

    #[tokio::test]
    async fn test_set_and_clear_current() {
        let manager = MemorySessionManager::new();

        let session = manager.create(SessionConfig::default()).await.unwrap();

        // Create another session
        let session2 = manager.create(SessionConfig::default()).await.unwrap();

        // session2 should be current
        assert_eq!(manager.current_id().await, Some(session2.id));

        // Set session1 as current
        manager.set_current(session.id).await;
        assert_eq!(manager.current_id().await, Some(session.id));

        // Clear current
        manager.clear_current().await;
        assert!(manager.current_id().await.is_none());
    }

    #[tokio::test]
    async fn test_list_summaries() {
        let manager = MemorySessionManager::new();

        let mut session1 = manager.create(SessionConfig::default()).await.unwrap();
        session1.set_title("Session 1");
        session1.add_message(Message::user("Hello 1"));
        manager.update(&session1).await.unwrap();

        let mut session2 = manager.create(SessionConfig::default()).await.unwrap();
        session2.set_title("Session 2");
        session2.add_message(Message::user("Hello 2"));
        manager.update(&session2).await.unwrap();

        let summaries = manager.list_summaries().await.unwrap();
        assert_eq!(summaries.len(), 2);

        // Should be sorted by updated_at descending (session2 is more recent)
        assert_eq!(summaries[0].id, session2.id);
        assert_eq!(summaries[1].id, session1.id);
    }

    #[tokio::test]
    async fn test_search() {
        let manager = MemorySessionManager::new();

        let mut session1 = manager.create(SessionConfig::default()).await.unwrap();
        session1.set_title("Rust Programming");
        session1.add_message(Message::user("How to use Rust"));
        manager.update(&session1).await.unwrap();

        let mut session2 = manager.create(SessionConfig::default()).await.unwrap();
        session2.set_title("Python Tips");
        session2.add_message(Message::user("Python best practices"));
        manager.update(&session2).await.unwrap();

        // Search for "Rust"
        let results = manager.search("rust").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, session1.id);

        // Search for "programming" (case insensitive)
        let results = manager.search("PROGRAMMING").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, session1.id);

        // Search for "best"
        let results = manager.search("best").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, session2.id);
    }

    #[tokio::test]
    async fn test_find_by_tag() {
        let manager = MemorySessionManager::new();

        let mut session1 = manager.create(SessionConfig::default()).await.unwrap();
        session1.add_tag("rust");
        session1.add_tag("coding");
        manager.update(&session1).await.unwrap();

        let mut session2 = manager.create(SessionConfig::default()).await.unwrap();
        session2.add_tag("python");
        session2.add_tag("coding");
        manager.update(&session2).await.unwrap();

        // Find by "rust" tag
        let results = manager.find_by_tag("rust").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, session1.id);

        // Find by "coding" tag (both sessions)
        let results = manager.find_by_tag("coding").await.unwrap();
        assert_eq!(results.len(), 2);
    }
}
