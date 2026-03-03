//! Memory loader — read operations for the memory system.
//!
//! `MemoryLoader` provides read-only access to memory files:
//! - Load `index.md` for system prompt injection
//! - Read individual memory files
//! - List files in a directory
//! - Resolve `@path` references

use std::path::{Path, PathBuf};

use crate::error::MemoryError;
use crate::index_manager::IndexManager;
use crate::types::{IndexEntry, MemoryFile, MemoryIndex, MemoryMeta, MemoryScope};

/// Read-only memory access.
///
/// Holds only `user_dir` (fixed at `~/.forge/memory/`).
/// `project_dir` is passed per-call since `working_dir` can change at runtime.
pub struct MemoryLoader {
    user_dir: PathBuf,
}

impl MemoryLoader {
    /// Create a new `MemoryLoader`.
    ///
    /// `user_dir` should be `~/.forge/memory/`.
    #[must_use]
    pub const fn new(user_dir: PathBuf) -> Self {
        Self { user_dir }
    }

    /// Resolve scope + `project_dir` to the actual filesystem directory.
    fn scope_dir(&self, scope: MemoryScope, project_dir: Option<&Path>) -> Option<PathBuf> {
        match scope {
            MemoryScope::User => Some(self.user_dir.clone()),
            MemoryScope::Project => project_dir.map(std::path::Path::to_path_buf),
        }
    }

    /// Validate path: reject traversal attempts and absolute paths.
    fn validate_path(path: &str) -> Result<(), MemoryError> {
        if path.contains("..") || path.starts_with('/') || path.starts_with('\\') {
            return Err(MemoryError::path_traversal(path));
        }
        Ok(())
    }

    /// Load `index.md` for the given scope, used for system prompt injection.
    ///
    /// Automatically repairs `.pending_index` before loading.
    /// Returns `None` if `index.md` doesn't exist.
    pub async fn load_index(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
    ) -> Result<Option<MemoryIndex>, MemoryError> {
        let scope_dir = match self.scope_dir(scope, project_dir) {
            Some(d) => d,
            None => return Ok(None),
        };

        if !scope_dir.exists() {
            return Ok(None);
        }

        IndexManager::repair_pending(&scope_dir).await?;

        let index_path = scope_dir.join("index.md");
        if !index_path.exists() {
            return Ok(None);
        }

        let content = tokio::fs::read_to_string(&index_path).await?;
        let body = Self::skip_frontmatter(&content);

        let mut sections: Vec<(String, Vec<IndexEntry>)> = Vec::new();
        let mut current_section = String::from("Memory Index");
        let mut current_entries: Vec<IndexEntry> = Vec::new();

        for line in body.lines() {
            let trimmed = line.trim();

            if let Some(heading) = trimmed.strip_prefix("## ") {
                if !current_entries.is_empty() {
                    sections.push((current_section.clone(), std::mem::take(&mut current_entries)));
                }
                current_section = heading.trim().to_string();
                continue;
            }
            if let Some(heading) = trimmed.strip_prefix("# ") {
                if !current_entries.is_empty() {
                    sections.push((current_section.clone(), std::mem::take(&mut current_entries)));
                }
                current_section = heading.trim().to_string();
                continue;
            }

            if let Some(item) = trimmed.strip_prefix("- ") {
                let (summary, reference) = if let Some(pos) = item.find(" → @") {
                    let summary = item[..pos].trim().to_string();
                    let path = item[pos + " → @".len()..].trim().to_string();
                    (summary, Some(path))
                } else if let Some(pos) = item.find("→ @") {
                    let summary = item[..pos].trim().to_string();
                    let path = item[pos + "→ @".len()..].trim().to_string();
                    (summary, Some(path))
                } else {
                    (item.trim().to_string(), None)
                };

                if !summary.is_empty() {
                    current_entries.push(IndexEntry { summary, reference });
                }
            }
        }

        if !current_entries.is_empty() {
            sections.push((current_section, current_entries));
        }

        Ok(Some(MemoryIndex { scope, sections }))
    }

    /// Read a specific memory file by relative path.
    ///
    /// Returns `None` if the file doesn't exist (not an error).
    /// Enforces 2000-token hard limit with truncation.
    pub async fn read_file(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
        path: &str,
    ) -> Result<Option<MemoryFile>, MemoryError> {
        self.read_file_inner(scope, project_dir, path, true).await
    }

    /// Read a specific memory file by relative path without truncation.
    ///
    /// Returns `None` if the file doesn't exist (not an error).
    pub async fn read_file_raw(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
        path: &str,
    ) -> Result<Option<MemoryFile>, MemoryError> {
        self.read_file_inner(scope, project_dir, path, false).await
    }

    async fn read_file_inner(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
        path: &str,
        truncate: bool,
    ) -> Result<Option<MemoryFile>, MemoryError> {
        Self::validate_path(path)?;

        let scope_dir = match self.scope_dir(scope, project_dir) {
            Some(d) => d,
            None => return Ok(None),
        };

        let file_path = scope_dir.join(path);

        // If path points to a directory, auto-read its index.md
        if file_path.is_dir() {
            let index_path = file_path.join("index.md");
            if !index_path.exists() {
                return Ok(None);
            }
            let index_rel = if path.ends_with('/') {
                format!("{path}index.md")
            } else {
                format!("{path}/index.md")
            };
            Self::validate_path(&index_rel)?;
            let canonical_scope = scope_dir.canonicalize().map_err(MemoryError::Io)?;
            let canonical = index_path.canonicalize().map_err(MemoryError::Io)?;
            if !canonical.starts_with(&canonical_scope) {
                return Err(MemoryError::path_traversal(&index_rel));
            }
            let raw = tokio::fs::read_to_string(&canonical).await?;
            let (meta, body) = Self::parse_frontmatter(&raw, &index_rel)?;
            let references = Self::extract_references(&body);
            return Ok(Some(MemoryFile { path: index_rel, meta, content: body, references }));
        }

        // Canonicalize and verify prefix to prevent symlink traversal
        let canonical = if file_path.exists() {
            let canonical = file_path.canonicalize().map_err(MemoryError::Io)?;
            let canonical_scope = scope_dir.canonicalize().map_err(MemoryError::Io)?;
            if !canonical.starts_with(&canonical_scope) {
                return Err(MemoryError::path_traversal(path));
            }
            canonical
        } else {
            return Ok(None);
        };

        let raw = tokio::fs::read_to_string(&canonical).await?;

        let token_estimate = forge_infra::estimate_tokens_fast(&raw);
        let content_str = if truncate && token_estimate > 2000 {
            let max_bytes = 2000 * 4;
            let safe_end = Self::floor_char_boundary(&raw, max_bytes);
            let mut truncated = raw[..safe_end].to_string();
            if let Some(pos) = truncated.rfind('\n') {
                truncated.truncate(pos + 1);
            }
            truncated.push_str("\n[内容已截断，建议拆分为更小的文件]\n");
            truncated
        } else {
            raw
        };

        let (meta, body) = Self::parse_frontmatter(&content_str, path)?;
        let references = Self::extract_references(&body);

        Ok(Some(MemoryFile { path: path.to_string(), meta, content: body, references }))
    }

    /// List all memory files in a directory (with first-line summary).
    ///
    /// Returns `Vec<(relative_path, summary)>`. Empty vec if directory doesn't exist.
    pub async fn list_files(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
        dir: &str,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        Self::validate_path(dir)?;

        let scope_dir = match self.scope_dir(scope, project_dir) {
            Some(d) => d,
            None => return Ok(Vec::new()),
        };

        let target_dir = if dir.is_empty() { scope_dir.clone() } else { scope_dir.join(dir) };

        if !target_dir.exists() || !target_dir.is_dir() {
            return Ok(Vec::new());
        }

        let canonical_scope = scope_dir.canonicalize().map_err(MemoryError::Io)?;
        let canonical_target = target_dir.canonicalize().map_err(MemoryError::Io)?;
        if !canonical_target.starts_with(&canonical_scope) {
            return Err(MemoryError::path_traversal(dir));
        }

        let mut results = Vec::new();
        let mut entries = tokio::fs::read_dir(&canonical_target).await?;

        while let Some(entry) = entries.next_entry().await? {
            let entry_path = entry.path();

            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }

            let safe_entry = if entry_path.exists() {
                match entry_path.canonicalize() {
                    Ok(canonical) if canonical.starts_with(&canonical_scope) => canonical,
                    _ => continue,
                }
            } else {
                continue;
            };

            if safe_entry.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                let rel_path = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
                let content = tokio::fs::read_to_string(&safe_entry).await.unwrap_or_default();
                let summary = IndexManager::extract_summary(&content, &name);
                results.push((rel_path, summary));
            } else if safe_entry.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                let rel_path =
                    if dir.is_empty() { format!("{name}/") } else { format!("{dir}/{name}/") };
                results.push((rel_path, String::from("[directory]")));
            }
        }

        results.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(results)
    }

    /// List all memory files recursively (for export).
    ///
    /// Returns `Vec<(relative_path, summary)>` including files in subdirectories.
    pub async fn list_files_recursive(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        let scope_dir = match self.scope_dir(scope, project_dir) {
            Some(d) => d,
            None => return Ok(Vec::new()),
        };

        if !scope_dir.exists() || !scope_dir.is_dir() {
            return Ok(Vec::new());
        }

        let canonical_scope = scope_dir.canonicalize().map_err(MemoryError::Io)?;

        let mut results = Vec::new();
        let mut stack: Vec<(PathBuf, String)> = vec![(scope_dir.clone(), String::new())];

        while let Some((dir, prefix)) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(e) => e,
                Err(_) => continue,
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();

                if name.starts_with('.') {
                    continue;
                }

                let entry_path = entry.path();

                let safe_entry = if entry_path.exists() {
                    match entry_path.canonicalize() {
                        Ok(canonical) if canonical.starts_with(&canonical_scope) => canonical,
                        _ => continue,
                    }
                } else {
                    continue;
                };

                let rel_path =
                    if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };

                if safe_entry.is_file() {
                    let content = tokio::fs::read_to_string(&safe_entry).await.unwrap_or_default();
                    let summary = IndexManager::extract_summary(&content, &name);
                    results.push((rel_path, summary));
                } else if safe_entry.is_dir() {
                    stack.push((safe_entry, rel_path));
                }
            }
        }

        results.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(results)
    }

    /// Resolve a `@path` reference to an actual filesystem path.
    pub fn resolve_reference(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
        ref_path: &str,
    ) -> Result<PathBuf, MemoryError> {
        Self::validate_path(ref_path)?;

        let scope_dir = self
            .scope_dir(scope, project_dir)
            .ok_or_else(|| MemoryError::file_not_found(ref_path))?;

        let resolved = scope_dir.join(ref_path);

        if resolved.exists() {
            let canonical = resolved.canonicalize().map_err(MemoryError::Io)?;
            let canonical_scope = scope_dir.canonicalize().map_err(MemoryError::Io)?;
            if !canonical.starts_with(&canonical_scope) {
                return Err(MemoryError::path_traversal(ref_path));
            }
            Ok(canonical)
        } else {
            Err(MemoryError::file_not_found(ref_path))
        }
    }

    /// Parse YAML frontmatter from file content.
    fn parse_frontmatter(content: &str, path: &str) -> Result<(MemoryMeta, String), MemoryError> {
        if !content.starts_with("---") {
            return Ok((MemoryMeta::default(), content.to_string()));
        }

        let rest = &content[3..];
        if let Some(end) = rest.find("\n---") {
            let yaml_str = &rest[..end];
            let after = end + 4;
            let body = if after < rest.len() {
                rest[after..].trim_start_matches('\n').to_string()
            } else {
                String::new()
            };

            let meta: MemoryMeta = serde_yaml::from_str(yaml_str)
                .map_err(|e| MemoryError::parse_error(path, e.to_string()))?;

            Ok((meta, body))
        } else {
            Ok((MemoryMeta::default(), content.to_string()))
        }
    }

    /// Skip YAML frontmatter and return body (for index parsing).
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

    /// Find the largest valid UTF-8 char boundary at or before `index`.
    const fn floor_char_boundary(s: &str, index: usize) -> usize {
        if index >= s.len() {
            return s.len();
        }
        let mut i = index;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    /// Extract `@path` references from markdown content.
    fn extract_references(content: &str) -> Vec<String> {
        let mut refs = Vec::new();
        for line in content.lines() {
            let mut remaining = line;
            while let Some(pos) = remaining.find('@') {
                let after = &remaining[pos + 1..];
                let end = after
                    .find(|c: char| c.is_whitespace() || c == ')' || c == ']')
                    .unwrap_or(after.len());
                let path = &after[..end];
                let has_md_ext = std::path::Path::new(path)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
                if (has_md_ext || path.contains('/')) && !path.is_empty() {
                    refs.push(path.to_string());
                }
                remaining = &after[end..];
            }
        }
        refs
    }
}
