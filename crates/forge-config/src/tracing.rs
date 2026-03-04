//! Tracing configuration for session recording.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn default_true() -> bool {
    true
}

fn default_trace_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("forge")
        .join("traces")
}

fn default_filename_template() -> String {
    "{timestamp}_{session_id}.jsonl".to_string()
}

fn default_buffer_size() -> usize {
    100
}

fn default_max_trace_files() -> Option<usize> {
    Some(100)
}

fn default_max_trace_age_days() -> Option<u32> {
    Some(30)
}

/// Tracing configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingConfig {
    /// Enable tracing (default: true).
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Output directory (default: ~/.local/share/forge/traces).
    #[serde(default = "default_trace_dir")]
    pub output_dir: PathBuf,

    /// Filename template (supports {session_id}, {timestamp}).
    #[serde(default = "default_filename_template")]
    pub filename_template: String,

    /// Memory buffer size for batch writing.
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,

    /// Record message content (default: true).
    #[serde(default = "default_true")]
    pub record_messages: bool,

    /// Record tool input/output details (default: true).
    #[serde(default = "default_true")]
    pub record_tool_details: bool,

    /// Maximum number of trace files to keep (None = unlimited).
    #[serde(default = "default_max_trace_files")]
    pub max_trace_files: Option<usize>,

    /// Maximum age of trace files in days (None = unlimited).
    #[serde(default = "default_max_trace_age_days")]
    pub max_trace_age_days: Option<u32>,
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            output_dir: default_trace_dir(),
            filename_template: default_filename_template(),
            buffer_size: default_buffer_size(),
            record_messages: true,
            record_tool_details: true,
            max_trace_files: default_max_trace_files(),
            max_trace_age_days: default_max_trace_age_days(),
        }
    }
}

impl TracingConfig {
    /// Generate output file path for a session.
    pub fn generate_path(&self, session_id: &str) -> PathBuf {
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let filename = self
            .filename_template
            .replace("{session_id}", session_id)
            .replace("{timestamp}", &timestamp.to_string());
        self.output_dir.join(filename)
    }

    /// Load configuration from environment variables.
    #[must_use]
    pub fn from_env(mut self) -> Self {
        if let Ok(val) = std::env::var("FORGE_TRACING_ENABLED") {
            self.enabled = val.parse().unwrap_or(self.enabled);
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_OUTPUT_DIR") {
            self.output_dir = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_BUFFER_SIZE") {
            self.buffer_size = val.parse().unwrap_or(self.buffer_size);
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_RECORD_MESSAGES") {
            self.record_messages = val.parse().unwrap_or(self.record_messages);
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_RECORD_TOOL_DETAILS") {
            self.record_tool_details = val.parse().unwrap_or(self.record_tool_details);
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_MAX_FILES") {
            self.max_trace_files = val.parse().ok();
        }
        if let Ok(val) = std::env::var("FORGE_TRACING_MAX_AGE_DAYS") {
            self.max_trace_age_days = val.parse().ok();
        }
        self
    }

    /// Clean up old trace files based on configuration.
    pub async fn cleanup_old_traces(&self) -> std::io::Result<()> {
        if !self.output_dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&self.output_dir).await?;
        let mut files = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            if let Ok(metadata) = entry.metadata().await {
                if metadata.is_file() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.ends_with(".jsonl") {
                            files.push((entry.path(), metadata));
                        }
                    }
                }
            }
        }

        // Sort by modification time (newest first)
        files.sort_by(|a, b| {
            b.1.modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
                .cmp(&a.1.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH))
        });

        let now = std::time::SystemTime::now();

        // Remove files exceeding max count
        if let Some(max_files) = self.max_trace_files {
            for (path, _) in files.iter().skip(max_files) {
                let _ = tokio::fs::remove_file(path).await;
            }
        }

        // Remove files exceeding max age
        if let Some(max_age_days) = self.max_trace_age_days {
            let max_age = std::time::Duration::from_secs(u64::from(max_age_days) * 24 * 3600);
            for (path, metadata) in &files {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        if age > max_age {
                            let _ = tokio::fs::remove_file(path).await;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
