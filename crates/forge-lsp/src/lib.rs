//! Forge LSP - Language Server Protocol client manager
//!
//! Provides lazy-connecting LSP clients for code intelligence features
//! like diagnostics, go-to-definition, and find-references.
//!
//! # Architecture
//!
//! - `LspManager` holds per-language singleton clients
//! - Clients are spawned lazily on first use
//! - Language servers are auto-detected from project marker files

pub mod client;
pub mod detect;

pub use client::path_to_file_uri;
use client::LspClient;
use detect::ServerConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// LSP-specific errors
#[derive(Debug, thiserror::Error)]
pub enum LspError {
    /// Failed to spawn language server process
    #[error("Failed to spawn server: {0}")]
    ServerSpawn(String),

    /// LSP protocol error (framing, serialization)
    #[error("Protocol error: {0}")]
    Protocol(String),

    /// Server returned an error response
    #[error("Server error ({code}): {message}")]
    ServerError {
        /// JSON-RPC error code
        code: i64,
        /// Error message
        message: String,
    },

    /// Request timed out
    #[error("Request timed out after {0}s")]
    Timeout(u64),

    /// No language server available for the requested language
    #[error("No language server available for: {0}")]
    NoServer(String),

    /// Server command not found in PATH
    #[error("Server not installed: {0}")]
    NotInstalled(String),
}

/// Result type for LSP operations
pub type Result<T> = std::result::Result<T, LspError>;

/// Manages per-language LSP client connections
///
/// Clients are created lazily — the language server is only spawned
/// when a tool first requests it. Each language gets a single shared client.
pub struct LspManager {
    /// Active clients keyed by language identifier
    clients: Arc<Mutex<HashMap<String, Arc<LspClient>>>>,
    /// Project root directory
    working_dir: PathBuf,
}

impl std::fmt::Debug for LspManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspManager").field("working_dir", &self.working_dir).finish_non_exhaustive()
    }
}

impl LspManager {
    /// Create a new manager for the given project directory
    #[must_use]
    pub fn new(working_dir: PathBuf) -> Self {
        Self { clients: Arc::new(Mutex::new(HashMap::new())), working_dir }
    }

    /// Get or create an LSP client for the given file
    ///
    /// Detects the language from the file extension, finds the appropriate
    /// server config, and lazily spawns the server if not already running.
    ///
    /// # Errors
    /// Returns error if no server is available or spawning fails
    pub async fn client_for_file(&self, file_path: &Path) -> Result<Arc<LspClient>> {
        let ext = file_path
            .extension()
            .and_then(|e| e.to_str())
            .ok_or_else(|| LspError::NoServer("unknown file type".to_string()))?;

        let config =
            detect::server_for_extension(ext).ok_or_else(|| LspError::NoServer(ext.to_string()))?;

        self.get_or_create(config).await
    }

    /// Get or create an LSP client for a specific language
    ///
    /// # Errors
    /// Returns error if no server is available or spawning fails
    pub async fn client_for_language(&self, language: &str) -> Result<Arc<LspClient>> {
        let config = detect::server_for_language(language)
            .ok_or_else(|| LspError::NoServer(language.to_string()))?;

        self.get_or_create(config).await
    }

    /// List languages with active LSP connections
    pub async fn active_languages(&self) -> Vec<String> {
        let clients = self.clients.lock().await;
        clients.keys().cloned().collect()
    }

    /// Shut down all active language servers
    pub async fn shutdown_all(&self) {
        let mut clients = self.clients.lock().await;
        for (lang, client) in clients.drain() {
            tracing::info!(language = %lang, "Shutting down LSP server");
            let _ = client.shutdown().await;
        }
    }

    /// Get or create a client for the given server config
    ///
    /// If a cached client's server has died, it is removed and a fresh one is spawned.
    async fn get_or_create(&self, config: &ServerConfig) -> Result<Arc<LspClient>> {
        let mut clients = self.clients.lock().await;

        if let Some(client) = clients.get(config.language) {
            if !client.is_dead() {
                return Ok(client.clone());
            }
            // Server died — remove stale entry and respawn below
            tracing::warn!(language = %config.language, "LSP server died, respawning");
            clients.remove(config.language);
        }

        // Check if the server command is available
        if !detect::is_command_available(config.command) {
            return Err(LspError::NotInstalled(format!(
                "{} (install it to enable {} code intelligence)",
                config.command, config.language
            )));
        }

        // Spawn the server
        let client =
            LspClient::spawn(config.command, config.args, &self.working_dir, config.language)
                .await?;

        // Initialize
        client.initialize().await?;

        let client = Arc::new(client);
        clients.insert(config.language.to_string(), client.clone());
        drop(clients);

        tracing::info!(
            language = %config.language,
            command = %config.command,
            "LSP server connected"
        );

        Ok(client)
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        // Best-effort shutdown — clients are killed on drop anyway (kill_on_drop)
        tracing::debug!("LspManager dropped, server processes will be killed");
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_lsp_error_display() {
        let err = LspError::NoServer("xyz".to_string());
        assert_eq!(err.to_string(), "No language server available for: xyz");

        let err = LspError::NotInstalled("rust-analyzer".to_string());
        assert!(err.to_string().contains("rust-analyzer"));

        let err = LspError::Timeout(30);
        assert!(err.to_string().contains("30"));
    }

    #[test]
    fn test_manager_creation() {
        let manager = LspManager::new(PathBuf::from("/tmp"));
        assert_eq!(manager.working_dir, PathBuf::from("/tmp"));
    }

    #[tokio::test]
    async fn test_active_languages_empty() {
        let manager = LspManager::new(PathBuf::from("/tmp"));
        let languages = manager.active_languages().await;
        assert!(languages.is_empty());
    }

    #[tokio::test]
    async fn test_client_for_unknown_extension() {
        let manager = LspManager::new(PathBuf::from("/tmp"));
        let result = manager.client_for_file(Path::new("file.xyz")).await;
        assert!(result.is_err());
        assert!(matches!(result.expect_err("should be NoServer"), LspError::NoServer(_)));
    }

    #[tokio::test]
    async fn test_client_for_no_extension() {
        let manager = LspManager::new(PathBuf::from("/tmp"));
        let result = manager.client_for_file(Path::new("Makefile")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_client_for_unknown_language() {
        let manager = LspManager::new(PathBuf::from("/tmp"));
        let result = manager.client_for_language("brainfuck").await;
        assert!(result.is_err());
        assert!(matches!(result.expect_err("should be NoServer"), LspError::NoServer(_)));
    }
}
