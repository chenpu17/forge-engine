//! Episodic memory store for successful recovery strategies.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::{AgentError, Result};

/// A single episode recording a recovery strategy outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeRecord {
    /// Error signature that triggered the recovery (e.g. `"not_found:read"`).
    pub signature: String,
    /// Fingerprint of the repository/context where the episode occurred.
    pub context_fingerprint: String,
    /// Description of the recovery strategy that was applied.
    pub strategy: String,
    /// Whether the strategy succeeded.
    pub success: bool,
    /// Number of tokens consumed during the recovery attempt.
    pub tokens_used: usize,
    /// When this episode was recorded.
    pub created_at: DateTime<Utc>,
}

/// JSONL-backed store for episodic memory records.
#[derive(Debug, Clone)]
pub struct EpisodicMemoryStore {
    path: PathBuf,
}

impl EpisodicMemoryStore {
    /// Create a new store writing to `working_dir/.forge/episodic-memory.jsonl`.
    #[must_use]
    pub fn new(working_dir: &Path) -> Self {
        let path = working_dir.join(".forge").join("episodic-memory.jsonl");
        Self { path }
    }

    /// Append a successful recovery episode to the store.
    ///
    /// Skips writing if the record is not marked as successful or contains
    /// sensitive content (API keys, tokens, etc.).
    ///
    /// # Errors
    ///
    /// Returns an error if the memory directory cannot be created, the file
    /// cannot be opened, or serialization/write fails.
    pub async fn append_success(&self, mut record: EpisodeRecord) -> Result<()> {
        if !record.success {
            return Ok(());
        }
        if contains_sensitive(&record.strategy) {
            tracing::debug!("Skipping episodic memory write due to sensitive content");
            return Ok(());
        }

        if record.created_at.timestamp() == 0 {
            record.created_at = Utc::now();
        }

        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                AgentError::SessionError(format!("Failed to create memory dir: {e}"))
            })?;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .map_err(|e| {
                AgentError::SessionError(format!("Failed to open episodic memory: {e}"))
            })?;

        let line = serde_json::to_string(&record)
            .map_err(|e| AgentError::SessionError(format!("Failed to serialize episode: {e}")))?;
        file.write_all(line.as_bytes())
            .await
            .map_err(|e| AgentError::SessionError(format!("Failed to write episode: {e}")))?;
        file.write_all(b"\n").await.map_err(|e| {
            AgentError::SessionError(format!("Failed to write episode newline: {e}"))
        })?;
        Ok(())
    }

    /// Find the most recent episode matching the given signature and context.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read.
    pub async fn find_latest(
        &self,
        signature: &str,
        context_fingerprint: &str,
    ) -> Result<Option<EpisodeRecord>> {
        let file = match tokio::fs::File::open(&self.path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(AgentError::SessionError(format!(
                    "Failed to open episodic memory: {e}"
                )));
            }
        };

        let mut reader = BufReader::new(file);
        let mut buf = String::new();
        let mut best: Option<EpisodeRecord> = None;

        loop {
            buf.clear();
            let n = reader.read_line(&mut buf).await.map_err(|e| {
                AgentError::SessionError(format!("Failed to read episodic memory: {e}"))
            })?;
            if n == 0 {
                break;
            }
            let line = buf.trim();
            if line.is_empty() {
                continue;
            }
            let parsed = serde_json::from_str::<EpisodeRecord>(line);
            let Ok(record) = parsed else {
                continue;
            };
            if record.signature != signature {
                continue;
            }
            if record.context_fingerprint != context_fingerprint {
                continue;
            }
            let replace = match &best {
                None => true,
                Some(existing) => record.created_at > existing.created_at,
            };
            if replace {
                best = Some(record);
            }
        }

        Ok(best)
    }
}

fn contains_sensitive(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("-----begin private key-----")
        || lower.contains("api_key")
        || lower.contains("access_token")
        || lower.contains("authorization: bearer")
        || lower.contains("password=")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_append_and_find_episode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = EpisodicMemoryStore::new(dir.path());
        let record = EpisodeRecord {
            signature: "not_found:read".to_string(),
            context_fingerprint: "repo-a".to_string(),
            strategy: "use glob before read".to_string(),
            success: true,
            tokens_used: 12,
            created_at: Utc::now(),
        };
        store.append_success(record.clone()).await.expect("append");

        let found =
            store.find_latest("not_found:read", "repo-a").await.expect("find").expect("must exist");
        assert_eq!(found.strategy, record.strategy);
    }

    #[tokio::test]
    async fn test_sensitive_content_is_filtered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = EpisodicMemoryStore::new(dir.path());
        let record = EpisodeRecord {
            signature: "sig".to_string(),
            context_fingerprint: "repo".to_string(),
            strategy: "Authorization: Bearer secret".to_string(),
            success: true,
            tokens_used: 1,
            created_at: Utc::now(),
        };
        store.append_success(record).await.expect("append");
        let found = store.find_latest("sig", "repo").await.expect("find");
        assert!(found.is_none());
    }
}
