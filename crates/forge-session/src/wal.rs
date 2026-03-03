//! Write-Ahead Log (WAL) session manager
//!
//! Provides crash-safe, append-only persistence with periodic snapshot compaction.
//! Falls back to reading from the legacy `.json` format when no WAL/snapshot exists.
//!
//! # File Layout
//!
//! ```text
//! storage_dir/
//! ├── {uuid}.json          # Legacy full-session file (fallback, still written for compat)
//! ├── {uuid}.snapshot      # Latest compacted session state
//! └── {uuid}.wal           # JSON Lines of delta events since last snapshot
//! ```

use crate::{
    Message, Result, Session, SessionConfig, SessionError, SessionId, SessionManager,
    SessionMetadata,
};
use async_trait::async_trait;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Number of WAL entries before triggering snapshot compaction.
const COMPACTION_THRESHOLD: usize = 50;

/// LRU cache capacity for in-memory sessions.
const CACHE_CAPACITY: usize = 10;

/// Per-session bookkeeping kept under one lock to avoid cross-lock ordering
/// issues between WAL counts and per-session mutex lookup.
struct SessionBookkeeping {
    wal_count: usize,
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl SessionBookkeeping {
    fn new() -> Self {
        Self { wal_count: 0, lock: Arc::new(tokio::sync::Mutex::new(())) }
    }
}

// ---------------------------------------------------------------------------
// WAL entry types
// ---------------------------------------------------------------------------

/// A single entry in the WAL file (one JSON line).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum WalEntry {
    /// New messages appended to the session.
    MessagesAppended { messages: Vec<Message>, metadata: SessionMetadata },
    /// All messages replaced (e.g. after context compression).
    MessagesReplaced { messages: Vec<Message>, metadata: SessionMetadata },
    /// Session config updated (e.g. model switch).
    ConfigUpdated { config: SessionConfig, metadata: SessionMetadata },
    /// Full session state (used when diff is ambiguous).
    FullState { session: Session },
}

// ---------------------------------------------------------------------------
// WalSessionManager
// ---------------------------------------------------------------------------

/// WAL-based session manager with append-only writes and periodic compaction.
pub struct WalSessionManager {
    storage_dir: PathBuf,
    /// In-memory LRU cache.
    cache: RwLock<LruCache<SessionId, Session>>,
    /// Per-session wal counters and mutex handles.
    session_state: RwLock<HashMap<SessionId, SessionBookkeeping>>,
    /// Current active session.
    current: RwLock<Option<SessionId>>,
}

impl WalSessionManager {
    /// Create a new WAL session manager.
    ///
    /// # Errors
    /// Returns error if the storage directory cannot be created.
    ///
    /// # Panics
    /// Panics if `CACHE_CAPACITY` is zero (compile-time constant, should never happen).
    pub fn new(storage_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&storage_dir).map_err(|e| {
            SessionError::PersistenceError(format!("Failed to create storage dir: {e}"))
        })?;

        // SAFETY: CACHE_CAPACITY is a non-zero compile-time constant
        #[allow(clippy::expect_used)]
        let cache_cap = NonZeroUsize::new(CACHE_CAPACITY).expect("cache capacity must be > 0");

        Ok(Self {
            storage_dir,
            cache: RwLock::new(LruCache::new(cache_cap)),
            session_state: RwLock::new(HashMap::new()),
            current: RwLock::new(None),
        })
    }

    // -- Path helpers -------------------------------------------------------

    fn snapshot_path(&self, id: SessionId) -> PathBuf {
        self.storage_dir.join(format!("{id}.snapshot"))
    }

    fn wal_path(&self, id: SessionId) -> PathBuf {
        self.storage_dir.join(format!("{id}.wal"))
    }

    /// Legacy `.json` path (for fallback reads and compatibility writes).
    fn legacy_path(&self, id: SessionId) -> PathBuf {
        self.storage_dir.join(format!("{id}.json"))
    }

    // -- Low-level I/O ------------------------------------------------------

    /// Write a full snapshot file (atomic: write tmp then rename).
    async fn write_snapshot(&self, session: &Session) -> Result<()> {
        let path = self.snapshot_path(session.id);
        let content = serde_json::to_string(session)
            .map_err(|e| SessionError::PersistenceError(format!("Serialize snapshot: {e}")))?;

        let tmp = path.with_extension("snapshot.tmp");
        tokio::fs::write(&tmp, &content)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Write snapshot tmp: {e}")))?;

        // fsync the temp file before rename for crash safety
        let tmp_file = tokio::fs::File::open(&tmp).await.map_err(|e| {
            SessionError::PersistenceError(format!("Open snapshot tmp for sync: {e}"))
        })?;
        tmp_file
            .sync_data()
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Sync snapshot tmp: {e}")))?;

        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Rename snapshot: {e}")))?;

        Ok(())
    }

    /// Append a single WAL entry (one JSON line).
    async fn append_wal(&self, id: SessionId, entry: &WalEntry) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        let path = self.wal_path(id);
        let mut line = serde_json::to_string(entry)
            .map_err(|e| SessionError::PersistenceError(format!("Serialize WAL entry: {e}")))?;
        line.push('\n');

        // Append (create if not exists)
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Open WAL: {e}")))?;
        file.write_all(line.as_bytes())
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Write WAL: {e}")))?;
        file.flush()
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Flush WAL: {e}")))?;
        file.sync_data()
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Sync WAL: {e}")))?;

        Ok(())
    }

    /// Truncate (delete) the WAL file for a session.
    async fn truncate_wal(&self, id: SessionId) -> Result<()> {
        let path = self.wal_path(id);
        if path.exists() {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| SessionError::PersistenceError(format!("Truncate WAL: {e}")))?;
        }
        Ok(())
    }

    /// Load a session snapshot from disk.
    async fn load_snapshot(&self, id: SessionId) -> Result<Option<Session>> {
        let path = self.snapshot_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Read snapshot: {e}")))?;
        let session: Session = serde_json::from_str(&content)
            .map_err(|e| SessionError::PersistenceError(format!("Parse snapshot: {e}")))?;
        Ok(Some(session))
    }

    /// Load and parse WAL entries for a session.
    async fn load_wal_entries(&self, id: SessionId) -> Result<Vec<WalEntry>> {
        let path = self.wal_path(id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Read WAL: {e}")))?;

        let mut entries = Vec::new();
        for (i, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<WalEntry>(line) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    // Corrupt line (e.g. partial write from crash) — stop here.
                    tracing::warn!(
                        session_id = %id,
                        line = i + 1,
                        error = %e,
                        "Corrupt WAL entry, stopping replay"
                    );
                    break;
                }
            }
        }
        Ok(entries)
    }

    /// Load a session from the legacy `.json` file.
    async fn load_legacy(&self, id: SessionId) -> Result<Option<Session>> {
        let path = self.legacy_path(id);
        if !path.exists() {
            return Ok(None);
        }
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Read legacy: {e}")))?;
        let session: Session = serde_json::from_str(&content)
            .map_err(|e| SessionError::PersistenceError(format!("Parse legacy: {e}")))?;
        Ok(Some(session))
    }

    /// Write legacy `.json` file for backward compatibility.
    async fn write_legacy(&self, session: &Session) -> Result<()> {
        let path = self.legacy_path(session.id);
        let content = serde_json::to_string_pretty(session)
            .map_err(|e| SessionError::PersistenceError(format!("Serialize legacy: {e}")))?;
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, &content)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Write legacy tmp: {e}")))?;
        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Rename legacy: {e}")))?;
        Ok(())
    }

    // -- Recovery -----------------------------------------------------------

    /// Recover a session: snapshot + WAL replay, with legacy fallback.
    async fn recover(&self, id: SessionId) -> Result<Option<Session>> {
        // Try snapshot + WAL first
        if let Some(mut session) = self.load_snapshot(id).await? {
            let entries = self.load_wal_entries(id).await?;
            let entry_count = entries.len();
            for entry in entries {
                Self::apply_entry(&mut session, entry);
            }
            // Update WAL count tracking
            self.set_wal_count(id, entry_count).await;
            return Ok(Some(session));
        }

        // Fallback to legacy .json
        if let Some(session) = self.load_legacy(id).await? {
            self.set_wal_count(id, 0).await;
            return Ok(Some(session));
        }

        Ok(None)
    }

    /// Apply a single WAL entry to a session in memory.
    fn apply_entry(session: &mut Session, entry: WalEntry) {
        match entry {
            WalEntry::MessagesAppended { messages, metadata } => {
                session.messages.extend(messages);
                session.metadata = metadata;
            }
            WalEntry::MessagesReplaced { messages, metadata } => {
                session.messages = messages;
                session.metadata = metadata;
            }
            WalEntry::ConfigUpdated { config, metadata } => {
                session.config = config;
                session.metadata = metadata;
            }
            WalEntry::FullState { session: full } => {
                *session = full;
            }
        }
    }

    // -- Delta computation --------------------------------------------------

    /// Compare two message slices for equality without requiring `PartialEq`.
    ///
    /// Uses a fast-path: compare role + text content per message. Only falls
    /// back to JSON serialization for individual messages with `Blocks` content
    /// where structural comparison is needed.
    fn messages_equal(a: &[Message], b: &[Message]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        for (ma, mb) in a.iter().zip(b.iter()) {
            if ma.role != mb.role {
                return false;
            }
            // Fast path: both are Text variant — compare strings directly
            if let (crate::MessageContent::Text(ta), crate::MessageContent::Text(tb)) =
                (&ma.content, &mb.content)
            {
                if ta != tb {
                    return false;
                }
            } else {
                // Blocks or mixed — fall back to JSON for this single message
                let ok = match (serde_json::to_vec(&ma.content), serde_json::to_vec(&mb.content)) {
                    (Ok(left), Ok(right)) => left == right,
                    _ => false,
                };
                if !ok {
                    return false;
                }
            }
        }
        true
    }

    /// Compute the WAL entry for an update by diffing against the cached state.
    fn compute_delta(cached: &Session, updated: &Session) -> WalEntry {
        let cached_len = cached.messages.len();
        let updated_len = updated.messages.len();

        // Case 1: Messages only appended (most common path)
        if updated_len > cached_len {
            // Validate that the shared prefix is unchanged.
            // Instead of serializing the entire prefix array to JSON, compare
            // messages individually for much better performance on large histories.
            let prefix_intact = if cached_len > 0 {
                Self::messages_equal(
                    &cached.messages[..cached_len],
                    &updated.messages[..cached_len],
                )
            } else {
                true
            };

            if prefix_intact {
                let new_messages = updated.messages[cached_len..].to_vec();
                return WalEntry::MessagesAppended {
                    messages: new_messages,
                    metadata: updated.metadata.clone(),
                };
            }
            // Prefix was modified — fall through to FullState
        }

        // Case 2: Messages replaced (compression, or fewer messages)
        if updated_len < cached_len {
            return WalEntry::MessagesReplaced {
                messages: updated.messages.clone(),
                metadata: updated.metadata.clone(),
            };
        }

        // Case 3: Same message count — check if config changed
        if cached.config != updated.config {
            return WalEntry::ConfigUpdated {
                config: updated.config.clone(),
                metadata: updated.metadata.clone(),
            };
        }

        // Case 4: Only metadata changed (title, tags, tokens, etc.)
        // Use MessagesReplaced with empty diff as a metadata-only update
        // is not worth a separate variant — just write full state.
        WalEntry::FullState { session: updated.clone() }
    }

    // -- Compaction ---------------------------------------------------------

    /// Compact: write snapshot, truncate WAL, update legacy file.
    async fn compact(&self, session: &Session) -> Result<()> {
        self.write_snapshot(session).await?;
        self.truncate_wal(session.id).await?;
        self.set_wal_count(session.id, 0).await;

        // Also update legacy file for backward compatibility
        self.write_legacy(session).await?;

        tracing::debug!(
            session_id = %session.id,
            messages = session.messages.len(),
            "WAL compacted to snapshot"
        );

        Ok(())
    }

    /// Check if compaction is needed and perform it.
    async fn maybe_compact(&self, session: &Session, wal_count: usize) -> Result<()> {
        if wal_count >= COMPACTION_THRESHOLD {
            self.compact(session).await?;
        }
        Ok(())
    }

    async fn get_session_lock(&self, id: SessionId) -> Arc<tokio::sync::Mutex<()>> {
        if let Some(state) = self.session_state.read().await.get(&id) {
            return state.lock.clone();
        }

        let mut state = self.session_state.write().await;
        state.entry(id).or_insert_with(SessionBookkeeping::new).lock.clone()
    }

    async fn set_wal_count(&self, id: SessionId, count: usize) {
        let mut state = self.session_state.write().await;
        let entry = state.entry(id).or_insert_with(SessionBookkeeping::new);
        entry.wal_count = count;
        drop(state);
    }

    async fn increment_wal_count(&self, id: SessionId) -> usize {
        let mut state = self.session_state.write().await;
        let entry = state.entry(id).or_insert_with(SessionBookkeeping::new);
        entry.wal_count = entry.wal_count.saturating_add(1);
        let count = entry.wal_count;
        drop(state);
        count
    }

    async fn remove_session_state(&self, id: SessionId) {
        self.session_state.write().await.remove(&id);
    }

    #[cfg(test)]
    async fn wal_count_for(&self, id: SessionId) -> usize {
        self.session_state.read().await.get(&id).map_or(0, |s| s.wal_count)
    }
}

#[async_trait]
impl SessionManager for WalSessionManager {
    async fn create(&self, config: SessionConfig) -> Result<Session> {
        let session = Session {
            id: SessionId::new(),
            config,
            messages: Vec::new(),
            metadata: SessionMetadata::default(),
        };

        // Write initial snapshot (no WAL entries yet)
        self.write_snapshot(&session).await?;
        // Also write legacy file
        self.write_legacy(&session).await?;

        self.cache.write().await.put(session.id, session.clone());
        self.set_wal_count(session.id, 0).await;
        *self.current.write().await = Some(session.id);

        tracing::info!(session_id = %session.id, "Created new session (WAL)");
        Ok(session)
    }

    async fn get(&self, id: SessionId) -> Result<Session> {
        // Check cache
        if let Some(session) = self.cache.write().await.get(&id) {
            return Ok(session.clone());
        }

        // Recover from disk
        if let Some(session) = self.recover(id).await? {
            self.cache.write().await.put(id, session.clone());
            return Ok(session);
        }

        Err(SessionError::NotFound(id))
    }

    async fn update(&self, session: &Session) -> Result<()> {
        let session_lock = self.get_session_lock(session.id).await;
        let _guard = session_lock.lock().await;

        // Compute delta against cached state
        let entry = {
            let mut cache = self.cache.write().await;
            if let Some(cached) = cache.get(&session.id) {
                let entry = Self::compute_delta(cached, session);
                // Update cache
                cache.put(session.id, session.clone());
                drop(cache);
                entry
            } else {
                // No cached state — write full state
                cache.put(session.id, session.clone());
                drop(cache);
                WalEntry::FullState { session: session.clone() }
            }
        };

        // Append to WAL
        self.append_wal(session.id, &entry).await?;

        // Increment WAL count and capture the new value for compaction check
        let new_count = self.increment_wal_count(session.id).await;

        // Maybe compact (pass count directly to avoid re-acquiring the lock)
        self.maybe_compact(session, new_count).await?;

        tracing::debug!(session_id = %session.id, "Updated session (WAL)");
        Ok(())
    }

    async fn delete(&self, id: SessionId) -> Result<()> {
        let session_lock = self.get_session_lock(id).await;
        let _guard = session_lock.lock().await;

        self.cache.write().await.pop(&id);
        self.remove_session_state(id).await;

        // Remove all files
        for path in [self.snapshot_path(id), self.wal_path(id), self.legacy_path(id)] {
            if path.exists() {
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        return Err(SessionError::PersistenceError(format!(
                            "Delete session file {}: {e}",
                            path.display()
                        )));
                    }
                }
            }
        }

        // Also clean up temp files
        for ext in ["snapshot.tmp", "tmp"] {
            let tmp = self.storage_dir.join(format!("{id}.{ext}"));
            if tmp.exists() {
                if let Err(e) = tokio::fs::remove_file(&tmp).await {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(
                            session_id = %id,
                            path = %tmp.display(),
                            error = %e,
                            "Failed to remove WAL temp file during session delete"
                        );
                    }
                }
            }
        }

        let mut current = self.current.write().await;
        if *current == Some(id) {
            *current = None;
        }
        drop(current);

        tracing::info!(session_id = %id, "Deleted session (WAL)");
        Ok(())
    }

    async fn list(&self) -> Result<Vec<SessionId>> {
        let mut sessions = std::collections::HashSet::new();

        let mut entries = tokio::fs::read_dir(&self.storage_dir).await.map_err(|e| {
            SessionError::PersistenceError(format!("Failed to read storage dir: {e}"))
        })?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| SessionError::PersistenceError(format!("Failed to read entry: {e}")))?
        {
            let path = entry.path();
            // Accept .json, .snapshot, .wal files
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "json" | "snapshot" | "wal") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(uuid) = uuid::Uuid::parse_str(stem) {
                        sessions.insert(SessionId::from(uuid));
                    }
                }
            }
        }

        Ok(sessions.into_iter().collect())
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionConfig;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_and_get() {
        let dir = TempDir::new().unwrap();
        let mgr = WalSessionManager::new(dir.path().to_path_buf()).unwrap();

        let session = mgr.create(SessionConfig::default()).await.unwrap();
        let fetched = mgr.get(session.id).await.unwrap();
        assert_eq!(session.id, fetched.id);
        assert!(fetched.messages.is_empty());
    }

    #[tokio::test]
    async fn test_update_appends_wal() {
        let dir = TempDir::new().unwrap();
        let mgr = WalSessionManager::new(dir.path().to_path_buf()).unwrap();

        let mut session = mgr.create(SessionConfig::default()).await.unwrap();
        session.add_message(Message::user("hello"));
        mgr.update(&session).await.unwrap();

        // WAL file should exist
        assert!(mgr.wal_path(session.id).exists());

        // Verify recovery
        let recovered = mgr.recover(session.id).await.unwrap().unwrap();
        assert_eq!(recovered.messages.len(), 1);
        assert_eq!(recovered.messages[0].text(), "hello");
    }

    #[tokio::test]
    async fn test_recovery_from_snapshot_plus_wal() {
        let dir = TempDir::new().unwrap();
        let mgr = WalSessionManager::new(dir.path().to_path_buf()).unwrap();

        let mut session = mgr.create(SessionConfig::default()).await.unwrap();
        let id = session.id;

        // Add several messages
        for i in 0..5 {
            session.add_message(Message::user(format!("msg {i}")));
            mgr.update(&session).await.unwrap();
        }

        // Clear cache to force disk recovery
        mgr.cache.write().await.pop(&id);

        let recovered = mgr.get(id).await.unwrap();
        assert_eq!(recovered.messages.len(), 5);
    }

    #[tokio::test]
    async fn test_compaction_triggers() {
        let dir = TempDir::new().unwrap();
        let mgr = WalSessionManager::new(dir.path().to_path_buf()).unwrap();

        let mut session = mgr.create(SessionConfig::default()).await.unwrap();
        let id = session.id;

        // Write enough updates to trigger compaction
        for i in 0..COMPACTION_THRESHOLD + 5 {
            session.add_message(Message::user(format!("msg {i}")));
            mgr.update(&session).await.unwrap();
        }

        // After compaction, WAL should be truncated
        let wal_path = mgr.wal_path(id);
        if wal_path.exists() {
            let content = tokio::fs::read_to_string(&wal_path).await.unwrap();
            let line_count = content.lines().filter(|l| !l.trim().is_empty()).count();
            // Should have at most 5 entries (the ones after compaction)
            assert!(line_count <= 5, "WAL should be compacted, got {line_count} entries");
        }

        // Snapshot should exist and be up to date
        let snapshot = mgr.load_snapshot(id).await.unwrap().unwrap();
        assert!(snapshot.messages.len() >= COMPACTION_THRESHOLD);

        // WAL count should be reset
        let count = mgr.wal_count_for(id).await;
        assert!(count < COMPACTION_THRESHOLD);
    }

    #[tokio::test]
    async fn test_message_replacement_delta() {
        let dir = TempDir::new().unwrap();
        let mgr = WalSessionManager::new(dir.path().to_path_buf()).unwrap();

        let mut session = mgr.create(SessionConfig::default()).await.unwrap();
        let id = session.id;

        // Add messages
        for i in 0..10 {
            session.add_message(Message::user(format!("msg {i}")));
        }
        mgr.update(&session).await.unwrap();

        // Simulate compression: replace messages with fewer
        session.messages = vec![
            Message::system("[Summary] Previous conversation about messages 0-9"),
            Message::user("msg 9"),
        ];
        session.metadata.updated_at = chrono::Utc::now();
        mgr.update(&session).await.unwrap();

        // Clear cache and recover
        mgr.cache.write().await.pop(&id);
        let recovered = mgr.get(id).await.unwrap();
        assert_eq!(recovered.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_removes_all_files() {
        let dir = TempDir::new().unwrap();
        let mgr = WalSessionManager::new(dir.path().to_path_buf()).unwrap();

        let mut session = mgr.create(SessionConfig::default()).await.unwrap();
        let id = session.id;
        session.add_message(Message::user("test"));
        mgr.update(&session).await.unwrap();

        // Files should exist
        assert!(mgr.snapshot_path(id).exists());
        assert!(mgr.wal_path(id).exists());
        assert!(mgr.legacy_path(id).exists());

        mgr.delete(id).await.unwrap();

        // All files should be gone
        assert!(!mgr.snapshot_path(id).exists());
        assert!(!mgr.wal_path(id).exists());
        assert!(!mgr.legacy_path(id).exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_delete_surfaces_remove_errors() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let storage_dir = dir.path().to_path_buf();
        let mgr = WalSessionManager::new(storage_dir.clone()).unwrap();

        let session = mgr.create(SessionConfig::default()).await.unwrap();
        let id = session.id;

        let mut readonly_perms = std::fs::metadata(&storage_dir).unwrap().permissions();
        readonly_perms.set_mode(0o555);
        std::fs::set_permissions(&storage_dir, readonly_perms).unwrap();

        let delete_result = mgr.delete(id).await;

        let mut writable_perms = std::fs::metadata(&storage_dir).unwrap().permissions();
        writable_perms.set_mode(0o755);
        std::fs::set_permissions(&storage_dir, writable_perms).unwrap();

        assert!(delete_result.is_err(), "delete should fail when files cannot be removed");
        let err_text = delete_result.unwrap_err().to_string();
        assert!(err_text.contains("Delete session file"), "unexpected error: {err_text}");
    }

    #[tokio::test]
    async fn test_legacy_fallback() {
        let dir = TempDir::new().unwrap();
        let mgr = WalSessionManager::new(dir.path().to_path_buf()).unwrap();

        // Manually write a legacy .json file (simulating old FileSessionManager)
        let session = Session {
            id: SessionId::new(),
            config: SessionConfig::default(),
            messages: vec![Message::user("legacy message")],
            metadata: SessionMetadata::default(),
        };
        let legacy_path = mgr.legacy_path(session.id);
        let content = serde_json::to_string_pretty(&session).unwrap();
        tokio::fs::write(&legacy_path, &content).await.unwrap();

        // Should be able to load via get()
        let loaded = mgr.get(session.id).await.unwrap();
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].text(), "legacy message");
    }

    #[tokio::test]
    async fn test_list_sessions() {
        let dir = TempDir::new().unwrap();
        let mgr = WalSessionManager::new(dir.path().to_path_buf()).unwrap();

        mgr.create(SessionConfig::default()).await.unwrap();
        mgr.create(SessionConfig::default()).await.unwrap();

        let list = mgr.list().await.unwrap();
        assert_eq!(list.len(), 2);
    }

    #[tokio::test]
    async fn test_corrupt_wal_line_recovery() {
        let dir = TempDir::new().unwrap();
        let mgr = WalSessionManager::new(dir.path().to_path_buf()).unwrap();

        let mut session = mgr.create(SessionConfig::default()).await.unwrap();
        let id = session.id;

        // Add valid messages
        session.add_message(Message::user("msg 1"));
        mgr.update(&session).await.unwrap();
        session.add_message(Message::user("msg 2"));
        mgr.update(&session).await.unwrap();

        // Append corrupt line to WAL
        let wal_path = mgr.wal_path(id);
        tokio::fs::write(
            &wal_path,
            format!(
                "{}\n{}\n{{corrupt json\n",
                // Re-read existing WAL content
                tokio::fs::read_to_string(&wal_path).await.unwrap().trim(),
                "" // empty line (should be skipped)
            ),
        )
        .await
        .unwrap();

        // Clear cache and recover — should get 2 messages (corrupt line skipped)
        mgr.cache.write().await.pop(&id);
        let recovered = mgr.get(id).await.unwrap();
        assert_eq!(recovered.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_config_update_delta() {
        let dir = TempDir::new().unwrap();
        let mgr = WalSessionManager::new(dir.path().to_path_buf()).unwrap();

        let mut session = mgr.create(SessionConfig::default()).await.unwrap();
        let id = session.id;

        // Change model (config update)
        session.config.model = "gpt-4o".to_string();
        session.metadata.updated_at = chrono::Utc::now();
        mgr.update(&session).await.unwrap();

        // Clear cache and recover
        mgr.cache.write().await.pop(&id);
        let recovered = mgr.get(id).await.unwrap();
        assert_eq!(recovered.config.model, "gpt-4o");
    }

    #[test]
    fn test_compute_delta_falls_back_when_middle_prefix_changes() {
        let id = SessionId::new();
        let config = SessionConfig::default();
        let metadata = SessionMetadata::default();
        let cached = Session {
            id,
            config: config.clone(),
            messages: vec![
                Message::user("first"),
                Message::assistant("middle-original"),
                Message::user("third"),
            ],
            metadata: metadata.clone(),
        };

        let updated = Session {
            id,
            config,
            messages: vec![
                Message::user("first"),
                Message::assistant("middle-mutated"),
                Message::user("third"),
                Message::assistant("new-tail"),
            ],
            metadata,
        };

        let delta = WalSessionManager::compute_delta(&cached, &updated);
        assert!(
            matches!(delta, WalEntry::FullState { .. }),
            "prefix mutation must not be encoded as append-only delta"
        );
    }
}
