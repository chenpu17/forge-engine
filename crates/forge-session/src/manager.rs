//! Session manager implementation
//!
//! Provides file-based session storage with caching.

use crate::{
    Result, Session, SessionConfig, SessionError, SessionId, SessionManager, SessionMetadata,
    SessionPersistenceFormat,
};
use async_trait::async_trait;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use tokio::sync::RwLock;

/// Maximum number of sessions to keep in the in-memory LRU cache.
const SESSION_CACHE_CAPACITY: NonZeroUsize = {
    // SAFETY: 10 is non-zero
    match NonZeroUsize::new(10) {
        Some(v) => v,
        None => unreachable!(),
    }
};

/// File-based session manager
pub struct FileSessionManager {
    /// Storage directory
    storage_dir: PathBuf,
    /// In-memory LRU cache (evicts least-recently-used sessions when full)
    cache: RwLock<LruCache<SessionId, Session>>,
    /// Current active session
    current: RwLock<Option<SessionId>>,
    /// Persistence format
    persistence_format: SessionPersistenceFormat,
}

impl FileSessionManager {
    /// Create a new file session manager
    ///
    /// # Errors
    /// Returns error if the storage directory cannot be created
    pub fn new(storage_dir: PathBuf) -> Result<Self> {
        // Ensure directory exists
        std::fs::create_dir_all(&storage_dir).map_err(|e| {
            SessionError::PersistenceError(format!("Failed to create storage dir: {e}"))
        })?;

        Ok(Self {
            storage_dir,
            cache: RwLock::new(LruCache::new(SESSION_CACHE_CAPACITY)),
            current: RwLock::new(None),
            persistence_format: SessionPersistenceFormat::default(),
        })
    }

    /// Override the persistence format
    #[must_use]
    pub const fn with_persistence_format(mut self, format: SessionPersistenceFormat) -> Self {
        self.persistence_format = format;
        self
    }

    /// Get the path for a session file
    fn session_path(&self, id: SessionId) -> PathBuf {
        self.storage_dir.join(format!("{id}.json"))
    }

    /// Load a session from file
    async fn load_from_file(&self, id: SessionId) -> Result<Option<Session>> {
        let path = self.session_path(id);

        if !path.exists() {
            return Ok(None);
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Failed to read session: {e}")))?;

        let session: Session = serde_json::from_str(&content)
            .map_err(|e| SessionError::PersistenceError(format!("Failed to parse session: {e}")))?;

        Ok(Some(session))
    }

    /// Save a session to file (atomic write)
    async fn save_to_file(&self, session: &Session) -> Result<()> {
        let path = self.session_path(session.id);
        let content = match self.persistence_format {
            SessionPersistenceFormat::PrettyJson => serde_json::to_string_pretty(session),
            SessionPersistenceFormat::CompactJson => serde_json::to_string(session),
        }
        .map_err(|e| SessionError::PersistenceError(format!("Failed to serialize: {e}")))?;

        // Atomic write: write to temp file then rename
        let tmp_path = path.with_extension("tmp");

        tokio::fs::write(&tmp_path, &content)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Failed to write: {e}")))?;

        // Flush data to disk before rename to prevent data loss on crash
        {
            let f = tokio::fs::File::open(&tmp_path)
                .await
                .map_err(|e| SessionError::PersistenceError(format!("Failed to open for sync: {e}")))?;
            f.sync_data()
                .await
                .map_err(|e| SessionError::PersistenceError(format!("Failed to sync: {e}")))?;
        }

        tokio::fs::rename(&tmp_path, &path)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Failed to rename: {e}")))?;

        Ok(())
    }

    /// Get the current active session ID
    pub async fn current_id(&self) -> Option<SessionId> {
        *self.current.read().await
    }

    /// Set the current active session
    pub async fn set_current(&self, id: SessionId) {
        *self.current.write().await = Some(id);
    }
}

#[async_trait]
impl SessionManager for FileSessionManager {
    async fn create(&self, config: SessionConfig) -> Result<Session> {
        let session = Session {
            id: SessionId::new(),
            config,
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        // Save to file
        self.save_to_file(&session).await?;

        // Update cache (LRU will auto-evict oldest if at capacity)
        self.cache.write().await.put(session.id, session.clone());

        // Set as current
        *self.current.write().await = Some(session.id);

        tracing::info!(session_id = %session.id, "Created new session");

        Ok(session)
    }

    async fn get(&self, id: SessionId) -> Result<Session> {
        // Check cache first (get() promotes to most-recently-used)
        if let Some(session) = self.cache.write().await.get(&id) {
            return Ok(session.clone());
        }

        // Load from file
        if let Some(session) = self.load_from_file(id).await? {
            self.cache.write().await.put(id, session.clone());
            return Ok(session);
        }

        Err(SessionError::NotFound(id))
    }

    async fn update(&self, session: &Session) -> Result<()> {
        // Update cache
        self.cache.write().await.put(session.id, session.clone());

        // Save to file
        self.save_to_file(session).await?;

        tracing::debug!(session_id = %session.id, "Updated session");

        Ok(())
    }

    async fn delete(&self, id: SessionId) -> Result<()> {
        // Remove from cache
        self.cache.write().await.pop(&id);

        // Delete file
        let path = self.session_path(id);
        if path.exists() {
            tokio::fs::remove_file(&path).await.map_err(|e| {
                SessionError::PersistenceError(format!("Failed to delete session: {e}"))
            })?;
        }

        // Clear current if this was the active session
        let mut current = self.current.write().await;
        if *current == Some(id) {
            *current = None;
        }
        drop(current);

        tracing::info!(session_id = %id, "Deleted session");

        Ok(())
    }

    async fn list(&self) -> Result<Vec<SessionId>> {
        let mut sessions = Vec::new();

        let mut entries = tokio::fs::read_dir(&self.storage_dir).await.map_err(|e| {
            SessionError::PersistenceError(format!("Failed to read storage dir: {e}"))
        })?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Failed to read entry: {e}")))?
        {
            let path = entry.path();

            if path.extension().is_some_and(|e| e == "json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(uuid) = uuid::Uuid::parse_str(stem) {
                        sessions.push(SessionId::from(uuid));
                    }
                }
            }
        }

        Ok(sessions)
    }

    async fn latest(&self) -> Result<Option<Session>> {
        let ids = self.list().await?;

        let mut latest: Option<Session> = None;

        for id in ids {
            if let Ok(session) = self.get(id).await {
                match &latest {
                    None => latest = Some(session),
                    Some(current) => {
                        if session.metadata.updated_at > current.metadata.updated_at {
                            latest = Some(session);
                        }
                    }
                }
            }
        }

        Ok(latest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_session() {
        let dir = TempDir::new().unwrap();
        let manager = FileSessionManager::new(dir.path().to_path_buf()).unwrap();

        let session = manager.create(SessionConfig::default()).await.unwrap();
        assert!(session.messages.is_empty());
        assert_eq!(session.metadata.turn_count, 0);
    }

    #[tokio::test]
    async fn test_get_session() {
        let dir = TempDir::new().unwrap();
        let manager = FileSessionManager::new(dir.path().to_path_buf()).unwrap();

        let created = manager.create(SessionConfig::default()).await.unwrap();
        let fetched = manager.get(created.id).await.unwrap();

        assert_eq!(created.id, fetched.id);
    }

    #[tokio::test]
    async fn test_update_session() {
        let dir = TempDir::new().unwrap();
        let manager = FileSessionManager::new(dir.path().to_path_buf()).unwrap();

        let mut session = manager.create(SessionConfig::default()).await.unwrap();
        session.messages.push(Message::user("test"));

        manager.update(&session).await.unwrap();

        let fetched = manager.get(session.id).await.unwrap();
        assert_eq!(fetched.messages.len(), 1);
    }

    #[tokio::test]
    async fn test_delete_session() {
        let dir = TempDir::new().unwrap();
        let manager = FileSessionManager::new(dir.path().to_path_buf()).unwrap();

        let session = manager.create(SessionConfig::default()).await.unwrap();
        let id = session.id;

        manager.delete(id).await.unwrap();

        let result = manager.get(id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let dir = TempDir::new().unwrap();
        let manager = FileSessionManager::new(dir.path().to_path_buf()).unwrap();

        manager.create(SessionConfig::default()).await.unwrap();
        manager.create(SessionConfig::default()).await.unwrap();

        let list = manager.list().await.unwrap();
        assert_eq!(list.len(), 2);
    }
}
