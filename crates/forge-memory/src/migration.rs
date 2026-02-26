//! Memory migration — convert flat `memory.md` to structured `memory/` directory.
//!
//! Handles automatic migration from the legacy flat `memory.md` model
//! to the new structured `memory/` directory system.

use std::path::{Path, PathBuf};

use crate::error::MemoryError;
use crate::index_manager::IndexManager;
use crate::types::MemoryScope;

/// Result of a migration operation.
#[derive(Debug, Clone)]
pub struct MigrationResult {
    /// Number of sections migrated as individual files.
    pub files_created: usize,
    /// Path to the backup file (`memory.md.bak`).
    pub backup_path: PathBuf,
    /// Whether migration was actually performed.
    pub migrated: bool,
}

/// Memory migration utility.
pub struct MemoryMigration;

impl MemoryMigration {
    /// Check if migration is needed for a given scope.
    #[must_use]
    pub fn needs_migration(legacy_path: &Path, memory_dir: &Path) -> bool {
        if !legacy_path.exists() {
            return false;
        }
        let index_path = memory_dir.join("index.md");
        !index_path.exists()
    }

    /// Migrate a flat `memory.md` to structured `memory/` directory.
    ///
    /// # Errors
    /// Returns error if file I/O fails.
    pub async fn migrate(
        legacy_path: &Path,
        memory_dir: &Path,
        scope: MemoryScope,
    ) -> Result<MigrationResult, MemoryError> {
        let backup_path = legacy_path.with_extension("md.bak");

        if !legacy_path.exists() {
            return Ok(MigrationResult { files_created: 0, backup_path, migrated: false });
        }

        let content = tokio::fs::read_to_string(legacy_path).await?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Ok(MigrationResult { files_created: 0, backup_path, migrated: false });
        }

        // Backup original BEFORE writing new files
        tokio::fs::copy(legacy_path, &backup_path).await?;

        tokio::fs::create_dir_all(memory_dir).await?;

        let sections = Self::split_sections(trimmed);

        let mut files_created = 0;
        for (title, body) in &sections {
            let filename = Self::title_to_filename(title);
            let file_path = memory_dir.join(&filename);

            let today = chrono::Local::now().format("%Y-%m-%d");
            let file_content = format!(
                "---\nscope: {scope}\nupdated: {today}\nsource: migration\nwrite_count: 0\n---\n\n# {title}\n\n{body}\n"
            );

            tokio::fs::write(&file_path, &file_content).await?;

            IndexManager::add_reference(memory_dir, &filename, title).await?;
            files_created += 1;
        }

        // If no sections found, migrate as single file
        if sections.is_empty() {
            let filename = "migrated.md";
            let today = chrono::Local::now().format("%Y-%m-%d");
            let file_content = format!(
                "---\nscope: {scope}\nupdated: {today}\nsource: migration\nwrite_count: 0\n---\n\n{trimmed}\n"
            );
            tokio::fs::write(memory_dir.join(filename), &file_content).await?;
            IndexManager::add_reference(memory_dir, filename, "Migrated memory").await?;
            files_created = 1;
        }

        tokio::fs::remove_file(legacy_path).await?;

        Ok(MigrationResult { files_created, backup_path, migrated: true })
    }

    /// Split flat memory content into `(title, body)` sections by `## heading`.
    fn split_sections(content: &str) -> Vec<(String, String)> {
        let mut sections = Vec::new();
        let mut current_title: Option<String> = None;
        let mut current_body = String::new();

        for line in content.lines() {
            if let Some(heading) = line.strip_prefix("## ") {
                if let Some(title) = current_title.take() {
                    let body = current_body.trim().to_string();
                    if !body.is_empty() {
                        sections.push((title, body));
                    }
                }
                current_title = Some(heading.trim().to_string());
                current_body.clear();
            } else {
                current_body.push_str(line);
                current_body.push('\n');
            }
        }

        if let Some(title) = current_title {
            let body = current_body.trim().to_string();
            if !body.is_empty() {
                sections.push((title, body));
            }
        }

        sections
    }

    /// Convert a section title to a safe filename.
    fn title_to_filename(title: &str) -> String {
        let safe: String = title
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();

        let mut result = String::new();
        let mut prev_underscore = false;
        for c in safe.chars() {
            if c == '_' {
                if !prev_underscore {
                    result.push(c);
                }
                prev_underscore = true;
            } else {
                result.push(c);
                prev_underscore = false;
            }
        }

        let trimmed = result.trim_matches('_');
        if trimmed.is_empty() {
            "untitled.md".to_string()
        } else {
            format!("{trimmed}.md")
        }
    }
}
