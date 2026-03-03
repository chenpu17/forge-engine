//! Session persistence utilities
//!
//! Provides auto-save functionality and crash recovery.

use crate::{Result, Session, SessionError, SessionManager};
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// Auto-save manager for sessions
pub struct AutoSaver {
    /// Session manager for persistence
    session_manager: Arc<dyn SessionManager>,
    /// Save interval
    interval: Duration,
    /// Dirty flag
    dirty: Arc<AtomicBool>,
}

impl AutoSaver {
    /// Create a new auto-saver
    pub fn new(session_manager: Arc<dyn SessionManager>, interval: Duration) -> Self {
        Self { session_manager, interval, dirty: Arc::new(AtomicBool::new(false)) }
    }

    /// Mark the session as needing to be saved
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Check if session needs saving
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// Start the auto-save background task
    pub fn start(&self, session: Arc<RwLock<Session>>) -> JoinHandle<()> {
        let manager = self.session_manager.clone();
        let dirty = self.dirty.clone();
        let interval = self.interval;

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);

            loop {
                ticker.tick().await;

                if dirty.swap(false, Ordering::Relaxed) {
                    let session_data = session.read().await.clone();
                    if let Err(e) = manager.update(&session_data).await {
                        dirty.store(true, Ordering::Relaxed); // restore dirty on failure
                        tracing::error!(error = %e, "Auto-save failed");
                    } else {
                        tracing::debug!("Auto-saved session");
                    }
                }
            }
        })
    }

    /// Force an immediate save
    ///
    /// # Errors
    /// Returns error if the session manager fails to persist the session
    pub async fn save_now(&self, session: &Session) -> Result<()> {
        self.session_manager.update(session).await?;
        self.dirty.store(false, Ordering::Relaxed);
        Ok(())
    }
}

/// Session exporter for various formats
pub struct SessionExporter;

impl SessionExporter {
    /// Export session to JSON
    ///
    /// # Errors
    /// Returns error if serialization fails
    pub fn to_json(session: &Session) -> Result<String> {
        serde_json::to_string_pretty(session)
            .map_err(|e| SessionError::PersistenceError(format!("JSON export failed: {e}")))
    }

    /// Export session to Markdown
    #[must_use]
    pub fn to_markdown(session: &Session) -> String {
        let mut md = String::new();

        // Header
        md.push_str("# Chat Session\n\n");
        let _ = writeln!(
            md,
            "**Created:** {}",
            session.metadata.created_at.format("%Y-%m-%d %H:%M:%S UTC")
        );
        let _ = write!(md, "**Working Directory:** {}\n\n", session.config.working_dir.display());
        md.push_str("---\n\n");

        // Messages
        for message in &session.messages {
            let role = match message.role {
                crate::MessageRole::User => "**User**",
                crate::MessageRole::Assistant => "**Assistant**",
                crate::MessageRole::System => "**System**",
            };

            let _ = write!(md, "## {role}\n\n");
            md.push_str(&message.text());
            md.push_str("\n\n");
        }

        md
    }

    /// Import session from JSON
    ///
    /// # Errors
    /// Returns error if deserialization fails
    pub fn from_json(json: &str) -> Result<Session> {
        serde_json::from_str(json)
            .map_err(|e| SessionError::PersistenceError(format!("JSON import failed: {e}")))
    }
}

/// Recovery manager for crash recovery
pub struct RecoveryManager {
    storage_dir: std::path::PathBuf,
}

impl RecoveryManager {
    /// Create a new recovery manager
    #[must_use]
    pub const fn new(storage_dir: std::path::PathBuf) -> Self {
        Self { storage_dir }
    }

    /// Clean up any orphaned temporary files
    ///
    /// # Errors
    /// Returns error if the storage directory cannot be read
    pub async fn cleanup(&self) -> Result<()> {
        let mut entries = tokio::fs::read_dir(&self.storage_dir).await.map_err(|e| {
            SessionError::PersistenceError(format!("Failed to read storage dir: {e}"))
        })?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Failed to read entry: {e}")))?
        {
            let path = entry.path();

            // Remove temporary files left from interrupted atomic writes
            if path.extension().is_some_and(|e| e == "tmp") {
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    tracing::warn!(path = ?path, error = %e, "Failed to remove temp file");
                } else {
                    tracing::info!(path = ?path, "Cleaned up orphaned temp file");
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageContent, MessageRole, SessionConfig, SessionId, SessionMetadata};
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::atomic::AtomicUsize;

    fn create_test_session() -> Session {
        let mut metadata = SessionMetadata::default();
        metadata.turn_count = 1;

        Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: vec![crate::Message {
                role: MessageRole::User,
                content: MessageContent::Text("Hello".to_string()),
                timestamp: Utc::now(),
            }],
            metadata,
        }
    }

    /// Mock SessionManager for testing AutoSaver
    struct MockSessionManager {
        update_count: AtomicUsize,
        should_fail: AtomicBool,
    }

    impl MockSessionManager {
        fn new() -> Self {
            Self { update_count: AtomicUsize::new(0), should_fail: AtomicBool::new(false) }
        }

        fn update_count(&self) -> usize {
            self.update_count.load(Ordering::Relaxed)
        }

        fn set_should_fail(&self, fail: bool) {
            self.should_fail.store(fail, Ordering::Relaxed);
        }
    }

    #[async_trait]
    impl SessionManager for MockSessionManager {
        async fn create(&self, _config: SessionConfig) -> Result<Session> {
            Ok(create_test_session())
        }

        async fn get(&self, _id: SessionId) -> Result<Session> {
            Ok(create_test_session())
        }

        async fn update(&self, _session: &Session) -> Result<()> {
            if self.should_fail.load(Ordering::Relaxed) {
                return Err(SessionError::PersistenceError("Mock error".into()));
            }
            self.update_count.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        async fn delete(&self, _id: SessionId) -> Result<()> {
            Ok(())
        }

        async fn list(&self) -> Result<Vec<SessionId>> {
            Ok(vec![])
        }

        async fn latest(&self) -> Result<Option<Session>> {
            Ok(None)
        }
    }

    #[test]
    fn test_export_json() {
        let session = create_test_session();
        let json = SessionExporter::to_json(&session).unwrap();
        assert!(json.contains("messages"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_export_markdown() {
        let session = create_test_session();
        let md = SessionExporter::to_markdown(&session);
        assert!(md.contains("# Chat Session"));
        assert!(md.contains("**User**"));
        assert!(md.contains("Hello"));
    }

    #[test]
    fn test_import_json() {
        let session = create_test_session();
        let json = SessionExporter::to_json(&session).unwrap();
        let imported = SessionExporter::from_json(&json).unwrap();
        assert_eq!(session.messages.len(), imported.messages.len());
    }

    // AutoSaver tests

    #[test]
    fn test_auto_saver_dirty_flag() {
        let mock_manager = Arc::new(MockSessionManager::new());
        let auto_saver = AutoSaver::new(mock_manager, Duration::from_secs(60));

        // Initially not dirty
        assert!(!auto_saver.is_dirty());

        // Mark dirty
        auto_saver.mark_dirty();
        assert!(auto_saver.is_dirty());
    }

    #[tokio::test]
    async fn test_auto_saver_save_now() {
        let mock_manager = Arc::new(MockSessionManager::new());
        let auto_saver = AutoSaver::new(mock_manager.clone(), Duration::from_secs(60));

        let session = create_test_session();

        // Mark dirty first
        auto_saver.mark_dirty();
        assert!(auto_saver.is_dirty());

        // Save now should clear dirty flag
        auto_saver.save_now(&session).await.unwrap();
        assert!(!auto_saver.is_dirty());

        // Check that update was called
        assert_eq!(mock_manager.update_count(), 1);
    }

    #[tokio::test]
    async fn test_auto_saver_save_now_clears_dirty() {
        let mock_manager = Arc::new(MockSessionManager::new());
        let auto_saver = AutoSaver::new(mock_manager.clone(), Duration::from_secs(60));

        let session = create_test_session();

        // Mark dirty and save
        auto_saver.mark_dirty();
        auto_saver.save_now(&session).await.unwrap();

        // Should be clean now
        assert!(!auto_saver.is_dirty());

        // Calling save again should still work
        auto_saver.save_now(&session).await.unwrap();
        assert_eq!(mock_manager.update_count(), 2);
    }

    #[tokio::test]
    async fn test_auto_saver_save_now_error() {
        let mock_manager = Arc::new(MockSessionManager::new());
        mock_manager.set_should_fail(true);

        let auto_saver = AutoSaver::new(mock_manager, Duration::from_secs(60));
        let session = create_test_session();

        let result = auto_saver.save_now(&session).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_auto_saver_start_saves_when_dirty() {
        let mock_manager = Arc::new(MockSessionManager::new());
        let auto_saver = AutoSaver::new(mock_manager.clone(), Duration::from_millis(50));

        let session = Arc::new(RwLock::new(create_test_session()));

        // Mark dirty before starting
        auto_saver.mark_dirty();

        // Start the auto-saver task
        let handle = auto_saver.start(session);

        // Wait for at least one tick
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should have saved at least once
        assert!(mock_manager.update_count() >= 1);

        // Clean up
        handle.abort();
    }

    #[tokio::test]
    async fn test_auto_saver_does_not_save_when_clean() {
        let mock_manager = Arc::new(MockSessionManager::new());
        let auto_saver = AutoSaver::new(mock_manager.clone(), Duration::from_millis(50));

        let session = Arc::new(RwLock::new(create_test_session()));

        // Do NOT mark dirty

        // Start the auto-saver task
        let handle = auto_saver.start(session);

        // Wait for at least one tick
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should NOT have saved (not dirty)
        assert_eq!(mock_manager.update_count(), 0);

        // Clean up
        handle.abort();
    }
}
