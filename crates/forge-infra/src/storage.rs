//! Storage abstraction and directory management.
//!
//! Provides:
//! - [`ForgeDirectories`] — standard directory structure management
//! - [`KvStore`] trait — key-value storage abstraction
//! - [`JsonFileStore`] — JSON file-based implementation

use crate::{InfraError, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};

/// Forge data directory structure.
///
/// Manages standard directories for data, config, cache, logs, and sessions.
/// All directories are unified under `~/.forge`.
#[derive(Debug, Clone)]
pub struct ForgeDirectories {
    /// Data directory (`~/.forge`).
    pub data: PathBuf,
    /// Config directory (`~/.forge`) — unified with data.
    pub config: PathBuf,
    /// Cache directory (`~/.forge/cache`).
    pub cache: PathBuf,
    /// Logs directory (`~/.forge/logs`).
    pub logs: PathBuf,
    /// Sessions directory (`~/.forge/sessions`).
    pub sessions: PathBuf,
}

impl ForgeDirectories {
    /// Get or create the standard directory structure.
    ///
    /// # Errors
    /// Returns error if home directory cannot be determined or directories
    /// cannot be created.
    pub fn get_or_create() -> Result<Self> {
        let home = dirs::home_dir()
            .ok_or_else(|| InfraError::Config("Cannot determine home directory".into()))?;

        let forge_dir = home.join(".forge");
        let dirs = Self {
            data: forge_dir.clone(),
            config: forge_dir.clone(),
            cache: forge_dir.join("cache"),
            logs: forge_dir.join("logs"),
            sessions: forge_dir.join("sessions"),
        };

        std::fs::create_dir_all(&dirs.data)?;
        std::fs::create_dir_all(&dirs.cache)?;
        std::fs::create_dir_all(&dirs.logs)?;
        std::fs::create_dir_all(&dirs.sessions)?;

        Ok(dirs)
    }

    /// Get project-specific directory (`<project_root>/.forge`).
    #[must_use]
    pub fn project_dir(project_root: &Path) -> PathBuf {
        project_root.join(".forge")
    }

    /// Get log file path for current date.
    #[must_use]
    pub fn log_file(&self) -> PathBuf {
        let date = chrono::Local::now().format("%Y-%m-%d");
        self.logs.join(format!("forge-{date}.log"))
    }

    /// Get session file path.
    #[must_use]
    pub fn session_file(&self, session_id: &str) -> PathBuf {
        self.sessions.join(format!("{session_id}.json"))
    }
}

/// Key-value storage trait.
pub trait KvStore: Send + Sync {
    /// Get a value by key.
    ///
    /// # Errors
    /// Returns error if storage operation fails.
    fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>>;

    /// Set a value by key.
    ///
    /// # Errors
    /// Returns error if storage operation fails.
    fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()>;

    /// Delete a value by key.
    ///
    /// # Errors
    /// Returns error if storage operation fails.
    fn delete(&self, key: &str) -> Result<()>;

    /// List all keys with a prefix.
    fn list_keys(&self, prefix: &str) -> Vec<String>;

    /// Flush pending writes.
    ///
    /// # Errors
    /// Returns error if flush fails.
    fn flush(&self) -> Result<()>;
}

/// JSON file-based key-value store.
///
/// Each key is stored as a separate JSON file.
pub struct JsonFileStore {
    /// Storage directory.
    dir: PathBuf,
}

impl JsonFileStore {
    /// Create a new JSON file store.
    ///
    /// # Errors
    /// Returns error if directory cannot be created.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    /// Convert key to safe file path.
    fn key_to_path(&self, key: &str) -> PathBuf {
        let safe_key = key
            .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
            .replace("..", "_");
        self.dir.join(format!("{safe_key}.json"))
    }
}

impl KvStore for JsonFileStore {
    fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let path = self.key_to_path(key);
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        let value = serde_json::from_str(&content)
            .map_err(|e| InfraError::Storage(format!("Failed to deserialize: {e}")))?;
        Ok(Some(value))
    }

    fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let path = self.key_to_path(key);
        let content = serde_json::to_string_pretty(value)
            .map_err(|e| InfraError::Storage(format!("Failed to serialize: {e}")))?;
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let path = self.key_to_path(key);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }

    fn list_keys(&self, prefix: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension()? != "json" {
                    return None;
                }
                let stem = path.file_stem()?.to_str()?;
                if stem.starts_with(prefix) {
                    Some(stem.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    fn flush(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_json_file_store() {
        let dir = tempdir().expect("create temp dir");
        let store = JsonFileStore::open(dir.path()).expect("open store");

        store
            .set("key1", &"value1".to_string())
            .expect("set");
        let value: Option<String> = store.get("key1").expect("get");
        assert_eq!(value, Some("value1".to_string()));

        let value: Option<String> = store.get("nonexistent").expect("get");
        assert!(value.is_none());

        store.delete("key1").expect("delete");
        let value: Option<String> = store.get("key1").expect("get");
        assert!(value.is_none());
    }

    #[test]
    fn test_json_file_store_list_keys() {
        let dir = tempdir().expect("create temp dir");
        let store = JsonFileStore::open(dir.path()).expect("open store");

        store.set("session_1", &"data1".to_string()).expect("set");
        store.set("session_2", &"data2".to_string()).expect("set");
        store.set("config_1", &"data3".to_string()).expect("set");

        let session_keys = store.list_keys("session_");
        assert_eq!(session_keys.len(), 2);
    }

    #[test]
    fn test_key_sanitization() {
        let dir = tempdir().expect("create temp dir");
        let store = JsonFileStore::open(dir.path()).expect("open store");

        store
            .set("../../../etc/passwd", &"test".to_string())
            .expect("set");
        let path = store.key_to_path("../../../etc/passwd");
        assert!(!path.to_string_lossy().contains(".."));
    }
}
