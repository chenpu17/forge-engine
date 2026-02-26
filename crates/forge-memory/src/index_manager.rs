//! Index manager for automatic `index.md` maintenance.
//!
//! Internal module used by `MemoryWriter` and `MemoryLoader`
//! to maintain `@path` references in `index.md` files.

use std::path::Path;

use crate::error::MemoryError;

/// Stateless index maintenance utility.
///
/// All methods are associated functions (no `&self`) operating directly on the filesystem.
pub struct IndexManager;

impl IndexManager {
    /// Add a `@path` reference for a new file in the parent `index.md`.
    ///
    /// Creates `index.md` if it doesn't exist. Skips if reference already exists.
    pub(crate) async fn add_reference(
        scope_dir: &Path,
        file_path: &str,
        summary: &str,
    ) -> Result<(), MemoryError> {
        let index_path = scope_dir.join("index.md");

        let existing = if index_path.exists() {
            tokio::fs::read_to_string(&index_path).await?
        } else {
            String::new()
        };

        let ref_marker = format!("@{file_path}");
        if existing.lines().any(|line| Self::line_has_exact_ref(line, &ref_marker)) {
            return Ok(());
        }

        let line = format!("- {summary} → @{file_path}\n");
        let new_content = if existing.is_empty() {
            format!(
                "---\nscope: {}\nupdated: {}\n---\n\n# Memory Index\n\n{line}",
                Self::infer_scope(scope_dir),
                chrono::Local::now().format("%Y-%m-%d"),
            )
        } else if existing.ends_with('\n') {
            format!("{existing}{line}")
        } else {
            format!("{existing}\n{line}")
        };

        tokio::fs::write(&index_path, new_content).await?;
        Ok(())
    }

    /// Remove a `@path` reference from the parent `index.md`.
    pub(crate) async fn remove_reference(
        scope_dir: &Path,
        file_path: &str,
    ) -> Result<(), MemoryError> {
        let index_path = scope_dir.join("index.md");
        if !index_path.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&index_path).await?;
        let ref_marker = format!("@{file_path}");

        let filtered: Vec<&str> =
            content.lines().filter(|line| !Self::line_has_exact_ref(line, &ref_marker)).collect();

        let mut new_content = filtered.join("\n");
        if content.ends_with('\n') && !new_content.ends_with('\n') {
            new_content.push('\n');
        }

        tokio::fs::write(&index_path, new_content).await?;
        Ok(())
    }

    /// Update a `@path` reference (for move operations).
    pub(crate) async fn update_reference(
        scope_dir: &Path,
        old_path: &str,
        new_path: &str,
    ) -> Result<(), MemoryError> {
        let index_path = scope_dir.join("index.md");
        if !index_path.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&index_path).await?;
        let old_ref = format!("@{old_path}");
        let new_ref = format!("@{new_path}");

        let new_content: String = content
            .lines()
            .map(|line| {
                if Self::line_has_exact_ref(line, &old_ref) {
                    line.replace(&old_ref, &new_ref)
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let new_content = if content.ends_with('\n') && !new_content.ends_with('\n') {
            format!("{new_content}\n")
        } else {
            new_content
        };
        tokio::fs::write(&index_path, new_content).await?;
        Ok(())
    }

    /// Repair pending index entries from `.pending_index` marker file.
    ///
    /// If a crash occurred during write, `.pending_index` contains paths
    /// that need to be indexed. After 3 failed attempts, the pending file
    /// is renamed to `.pending_index.failed`.
    pub(crate) async fn repair_pending(scope_dir: &Path) -> Result<(), MemoryError> {
        const MAX_RETRIES: u32 = 3;

        let pending_path = scope_dir.join(".pending_index");
        if !pending_path.exists() {
            return Ok(());
        }

        let retries_path = scope_dir.join(".pending_index.retries");
        let retry_count: u32 = if retries_path.exists() {
            tokio::fs::read_to_string(&retries_path)
                .await
                .ok()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0)
        } else {
            0
        };

        if retry_count >= MAX_RETRIES {
            let failed_path = scope_dir.join(".pending_index.failed");
            tracing::warn!(
                "repair_pending exceeded {MAX_RETRIES} retries, moving to {failed_path:?}"
            );
            let _ = tokio::fs::rename(&pending_path, &failed_path).await;
            let _ = tokio::fs::remove_file(&retries_path).await;
            return Ok(());
        }

        tokio::fs::write(&retries_path, (retry_count + 1).to_string()).await?;

        let pending = tokio::fs::read_to_string(&pending_path).await?;
        let canonical_scope = scope_dir.canonicalize().map_err(MemoryError::Io)?;

        let mut failed_paths: Vec<String> = Vec::new();

        for line in pending.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if line.contains("..") || line.starts_with('/') || line.starts_with('\\') {
                tracing::warn!("Skipping invalid path in .pending_index: {line}");
                continue;
            }

            let file_path = scope_dir.join(line);
            if file_path.exists() {
                if let Ok(canonical) = file_path.canonicalize() {
                    if !canonical.starts_with(&canonical_scope) {
                        tracing::warn!("Skipping out-of-scope path in .pending_index: {line}");
                        continue;
                    }
                }
                let content = match tokio::fs::read_to_string(&file_path).await {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("Skipping unreadable file in .pending_index: {line}: {e}");
                        failed_paths.push(line.to_string());
                        continue;
                    }
                };
                let summary = Self::extract_summary(&content, line);
                if let Err(e) = Self::add_reference(scope_dir, line, &summary).await {
                    tracing::warn!("Failed to add index reference for {line}: {e}");
                    failed_paths.push(line.to_string());
                }
            }
        }

        if failed_paths.is_empty() {
            tokio::fs::remove_file(&pending_path).await?;
            let _ = tokio::fs::remove_file(&retries_path).await;
        } else {
            let remaining = failed_paths.join("\n") + "\n";
            tokio::fs::write(&pending_path, remaining).await?;
        }
        Ok(())
    }

    /// Extract summary from file content (first heading or filename).
    pub(crate) fn extract_summary(content: &str, filename: &str) -> String {
        let body = Self::skip_frontmatter(content);

        for line in body.lines() {
            let trimmed = line.trim();
            if let Some(heading) = trimmed.strip_prefix("# ") {
                return heading.trim().to_string();
            }
            if let Some(heading) = trimmed.strip_prefix("## ") {
                return heading.trim().to_string();
            }
        }

        filename.strip_suffix(".md").unwrap_or(filename).replace(['_', '-'], " ")
    }

    /// Skip YAML frontmatter (`---` delimited) and return body.
    fn skip_frontmatter(content: &str) -> &str {
        if !content.starts_with("---") {
            return content;
        }
        if let Some(end) = content[3..].find("\n---") {
            let after = end + 3 + 4;
            if after < content.len() {
                return content[after..].trim_start_matches('\n');
            }
        }
        content
    }

    /// Check if a line contains an exact `@path` reference (not a substring).
    ///
    /// Ensures the character after `@path` is a word boundary.
    pub(crate) fn line_has_exact_ref(line: &str, ref_marker: &str) -> bool {
        let mut start = 0;
        while let Some(pos) = line[start..].find(ref_marker) {
            let abs_pos = start + pos;
            let after_idx = abs_pos + ref_marker.len();
            if after_idx >= line.len() {
                return true;
            }
            match line.as_bytes()[after_idx] {
                b' ' | b'\t' | b'\n' | b'\r' | b')' | b']' => return true,
                _ => {}
            }
            start = abs_pos + 1;
        }
        false
    }

    /// Infer scope name from directory path.
    fn infer_scope(scope_dir: &Path) -> &str {
        let data_memory = forge_infra::data_dir().join("memory");
        if scope_dir == data_memory.as_path() {
            "user"
        } else {
            "project"
        }
    }
}
