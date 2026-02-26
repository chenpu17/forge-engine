//! Memory writer — write operations for the memory system.
//!
//! `MemoryWriter` provides write access to memory files:
//! - Write files (replace/append/merge modes)
//! - Delete files
//! - Move files with reference updates
//! - Auto-maintain `index.md` via `IndexManager`

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::MemoryError;
use crate::index_manager::IndexManager;
use crate::types::{MemoryScope, MoveResult, WriteMode};

/// Bypassable sensitive patterns — rejected by default, allowed with `allow_sensitive=true`.
const BYPASSABLE_PATTERNS: &[(&str, &str)] = &[
    (r"(?i)(api[_-]?key|apikey)\s*[:=]\s*\S{8,}", "API key"),
    (r"(?i)(secret[_-]?key|secretkey)\s*[:=]\s*\S{8,}", "secret key"),
    (r"(?i)(access[_-]?token|auth[_-]?token)\s*[:=]\s*\S{8,}", "access token"),
    (r"(?i)password\s*[:=]\s*\S{6,}", "password"),
    (r"(?i)(aws[_-]?access[_-]?key[_-]?id)\s*[:=]\s*\S{16,}", "AWS access key"),
    (r"sk-[a-zA-Z0-9]{20,}", "API secret key (sk-...)"),
    (r"ghp_[a-zA-Z0-9]{36,}", "GitHub personal access token"),
    (r"gho_[a-zA-Z0-9]{36,}", "GitHub OAuth token"),
];

/// High-risk patterns — always rejected even with `allow_sensitive=true`.
const HIGH_RISK_PATTERNS: &[(&str, &str)] =
    &[(r"-----BEGIN\s+(\w+\s+)*PRIVATE\s+KEY(\s+BLOCK)?-----", "private key block")];

/// Maximum total size per scope directory (1 MB).
const MAX_SCOPE_SIZE_BYTES: u64 = 1_048_576;

/// Write access to memory files.
///
/// Holds only `user_dir` (fixed at `~/.forge/memory/`).
/// `project_dir` is passed per-call since `working_dir` can change at runtime.
pub struct MemoryWriter {
    user_dir: PathBuf,
}

impl MemoryWriter {
    /// Create a new `MemoryWriter`.
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

    /// Check content for sensitive data patterns.
    ///
    /// When `allow_sensitive` is `true`, bypassable patterns are skipped but high-risk
    /// patterns (e.g. private key blocks) are still rejected unconditionally.
    fn check_sensitive_content(content: &str, allow_sensitive: bool) -> Result<(), MemoryError> {
        use std::sync::OnceLock;

        static HIGH_RISK_COMPILED: OnceLock<Vec<(regex::Regex, &str)>> = OnceLock::new();
        static BYPASSABLE_COMPILED: OnceLock<Vec<(regex::Regex, &str)>> = OnceLock::new();

        fn compile(
            patterns: &[(&'static str, &'static str)],
        ) -> Vec<(regex::Regex, &'static str)> {
            patterns
                .iter()
                .filter_map(|(pattern, label)| match regex::Regex::new(pattern) {
                    Ok(re) => Some((re, *label)),
                    Err(e) => {
                        tracing::warn!("Invalid sensitive content pattern '{label}': {e}");
                        None
                    }
                })
                .collect()
        }

        let high_risk = HIGH_RISK_COMPILED.get_or_init(|| compile(HIGH_RISK_PATTERNS));
        for (re, label) in high_risk {
            if re.is_match(content) {
                return Err(MemoryError::SensitiveContent((*label).to_string()));
            }
        }

        if !allow_sensitive {
            let bypassable = BYPASSABLE_COMPILED.get_or_init(|| compile(BYPASSABLE_PATTERNS));
            for (re, label) in bypassable {
                if re.is_match(content) {
                    return Err(MemoryError::SensitiveContent((*label).to_string()));
                }
            }
        }

        Ok(())
    }

    /// Verify that a resolved path stays within the scope directory (symlink protection).
    fn verify_within_scope(
        file_path: &Path,
        scope_dir: &Path,
    ) -> Result<PathBuf, MemoryError> {
        let canonical_scope = scope_dir.canonicalize().map_err(MemoryError::Io)?;

        if file_path.exists() {
            let canonical = file_path.canonicalize().map_err(MemoryError::Io)?;
            if !canonical.starts_with(&canonical_scope) {
                return Err(MemoryError::path_traversal(
                    file_path.to_string_lossy().to_string(),
                ));
            }
            Ok(canonical)
        } else if let Some(parent) = file_path.parent() {
            if parent.exists() {
                let canonical_parent = parent.canonicalize().map_err(MemoryError::Io)?;
                if !canonical_parent.starts_with(&canonical_scope) {
                    return Err(MemoryError::path_traversal(
                        file_path.to_string_lossy().to_string(),
                    ));
                }
                let filename = file_path.file_name().ok_or_else(|| {
                    MemoryError::path_traversal(file_path.to_string_lossy().to_string())
                })?;
                Ok(canonical_parent.join(filename))
            } else {
                Ok(file_path.to_path_buf())
            }
        } else {
            Ok(file_path.to_path_buf())
        }
    }

    /// Calculate total size of all files in a scope directory (recursive).
    async fn scope_total_size(dir: &Path) -> u64 {
        let mut total: u64 = 0;
        let mut stack = vec![dir.to_path_buf()];

        while let Some(current) = stack.pop() {
            let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let Ok(ft) = entry.file_type().await else {
                    continue;
                };
                if ft.is_symlink() {
                    continue;
                }
                if ft.is_file() {
                    if let Ok(meta) = entry.metadata().await {
                        total += meta.len();
                    }
                } else if ft.is_dir() {
                    stack.push(entry.path());
                }
            }
        }
        total
    }

    /// Generate YAML frontmatter for a new file.
    fn generate_frontmatter(scope: MemoryScope) -> String {
        let today = chrono::Local::now().format("%Y-%m-%d");
        format!("---\nscope: {scope}\nupdated: {today}\nwrite_count: 1\n---\n\n")
    }

    /// Update the `updated` date and increment `write_count` in existing frontmatter.
    fn update_frontmatter(content: &str) -> String {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        if !content.starts_with("---") {
            return content.to_string();
        }

        let rest = &content[3..];
        if let Some(end) = rest.find("\n---") {
            let yaml_str = &rest[..end];
            let after_fm = &rest[end + 4..];

            let mut lines: Vec<String> = yaml_str.lines().map(std::string::ToString::to_string).collect();
            let mut found_updated = false;
            let mut found_write_count = false;
            let mut write_count: u32 = 0;

            for line in &mut lines {
                if line.starts_with("updated:") {
                    *line = format!("updated: {today}");
                    found_updated = true;
                } else if line.starts_with("write_count:") {
                    if let Some(val) = line.strip_prefix("write_count:") {
                        write_count = val.trim().parse().unwrap_or(0);
                    }
                    write_count = write_count.saturating_add(1);
                    *line = format!("write_count: {write_count}");
                    found_write_count = true;
                }
            }

            if !found_updated {
                lines.push(format!("updated: {today}"));
            }
            if !found_write_count {
                lines.push("write_count: 1".to_string());
            }

            format!("---\n{}\n---{after_fm}", lines.join("\n"))
        } else {
            content.to_string()
        }
    }

    /// Write a memory file (supports replace/append/merge modes).
    ///
    /// Auto-maintains `index.md` via `IndexManager` after successful write.
    /// Uses `.pending_index` marker for crash safety.
    /// Enforces 2000-token hard limit.
    #[allow(clippy::too_many_lines)]
    pub async fn write_file(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
        path: &str,
        content: &str,
        mode: WriteMode,
        allow_sensitive: bool,
    ) -> Result<(), MemoryError> {
        Self::validate_path(path)?;

        if path == "index.md" || path.ends_with("/index.md") {
            return Err(MemoryError::InvalidOperation(
                "index.md is auto-maintained and cannot be written directly".into(),
            ));
        }

        Self::check_sensitive_content(content, allow_sensitive)?;

        let scope_dir = self
            .scope_dir(scope, project_dir)
            .ok_or_else(|| MemoryError::file_not_found("no scope directory"))?;

        tokio::fs::create_dir_all(&scope_dir).await?;
        let canonical_scope = scope_dir.canonicalize().map_err(MemoryError::Io)?;

        let file_path = canonical_scope.join(path);

        let current_size = Self::scope_total_size(&canonical_scope).await;
        let existing_file_size = if matches!(mode, WriteMode::Replace) && file_path.exists() {
            tokio::fs::metadata(&file_path).await.map(|m| m.len()).unwrap_or(0)
        } else {
            0
        };

        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let safe_path = Self::verify_within_scope(&file_path, &canonical_scope)?;

        let final_content = match mode {
            WriteMode::Replace => {
                let fm = Self::generate_frontmatter(scope);
                let full = format!("{fm}{content}");
                if forge_infra::estimate_tokens_fast(&full) > 2000 {
                    return Err(MemoryError::FileTooLarge(2000));
                }
                full
            }
            WriteMode::Append => {
                let existing = if safe_path.exists() {
                    let raw = tokio::fs::read_to_string(&safe_path).await?;
                    Self::update_frontmatter(&raw)
                } else {
                    Self::generate_frontmatter(scope)
                };
                let full = if existing.ends_with('\n') {
                    format!("{existing}{content}\n")
                } else {
                    format!("{existing}\n{content}\n")
                };
                if forge_infra::estimate_tokens_fast(&full) > 2000 {
                    return Err(MemoryError::FileTooLarge(2000));
                }
                full
            }
            WriteMode::Merge => {
                // Phase 2: smart section merge. For now, fall back to replace.
                let fm = Self::generate_frontmatter(scope);
                let full = format!("{fm}{content}");
                if forge_infra::estimate_tokens_fast(&full) > 2000 {
                    return Err(MemoryError::FileTooLarge(2000));
                }
                full
            }
        };

        let new_bytes = final_content.len() as u64;
        if current_size.saturating_sub(existing_file_size) + new_bytes > MAX_SCOPE_SIZE_BYTES {
            return Err(MemoryError::FileTooLarge(MAX_SCOPE_SIZE_BYTES as usize));
        }

        // Crash-safe: write pending marker before file write
        let pending_path = canonical_scope.join(".pending_index");
        let pending_entry = format!("{path}\n");
        {
            use tokio::io::AsyncWriteExt;
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&pending_path)
                .await?;
            file.write_all(pending_entry.as_bytes()).await?;
        }

        tokio::fs::write(&safe_path, &final_content).await?;

        let summary = IndexManager::extract_summary(&final_content, path);
        IndexManager::add_reference(&canonical_scope, path, &summary).await?;

        let _ = tokio::fs::remove_file(&pending_path).await;

        Ok(())
    }

    /// Delete a memory file and remove its `index.md` reference.
    pub async fn delete_file(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
        path: &str,
    ) -> Result<(), MemoryError> {
        Self::validate_path(path)?;

        if path == "index.md" || path.ends_with("/index.md") {
            return Err(MemoryError::InvalidOperation(
                "index.md is auto-maintained and cannot be deleted directly".into(),
            ));
        }

        let scope_dir = self
            .scope_dir(scope, project_dir)
            .ok_or_else(|| MemoryError::file_not_found("no scope directory"))?;

        let file_path = scope_dir.join(path);
        if !file_path.exists() {
            return Err(MemoryError::file_not_found(path));
        }

        let safe_path = Self::verify_within_scope(&file_path, &scope_dir)?;

        tokio::fs::remove_file(&safe_path).await?;

        IndexManager::remove_reference(&scope_dir, path).await?;

        if let Some(parent) = file_path.parent() {
            Self::cleanup_empty_dirs(parent, &scope_dir).await;
        }

        Ok(())
    }

    /// Move a memory file, updating `index.md` references.
    ///
    /// Returns `MoveResult` with any dangling references that couldn't be auto-updated.
    pub async fn move_file(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
        from: &str,
        to: &str,
    ) -> Result<MoveResult, MemoryError> {
        Self::validate_path(from)?;
        Self::validate_path(to)?;

        if from == "index.md" || from.ends_with("/index.md") {
            return Err(MemoryError::InvalidOperation(
                "index.md is auto-maintained and cannot be moved".into(),
            ));
        }
        if to == "index.md" || to.ends_with("/index.md") {
            return Err(MemoryError::InvalidOperation(
                "index.md is auto-maintained and cannot be overwritten".into(),
            ));
        }

        let scope_dir = self
            .scope_dir(scope, project_dir)
            .ok_or_else(|| MemoryError::file_not_found("no scope directory"))?;

        let from_path = scope_dir.join(from);
        let to_path = scope_dir.join(to);

        if !from_path.exists() {
            return Err(MemoryError::file_not_found(from));
        }

        let safe_from = Self::verify_within_scope(&from_path, &scope_dir)?;

        if let Some(parent) = to_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let safe_to = Self::verify_within_scope(&to_path, &scope_dir)?;

        tokio::fs::rename(&safe_from, &safe_to).await?;

        IndexManager::update_reference(&scope_dir, from, to).await?;

        let mut failed_updates = Self::update_scope_references(&scope_dir, from, to).await;

        let mut dangling = Self::find_dangling_refs(&scope_dir, from).await;
        dangling.append(&mut failed_updates);
        dangling.sort();
        dangling.dedup();

        if let Some(parent) = from_path.parent() {
            Self::cleanup_empty_dirs(parent, &scope_dir).await;
        }

        Ok(MoveResult { dangling_refs: dangling })
    }

    /// Update exact `@old_path` references to `@new_path` across all markdown files.
    async fn update_scope_references(
        scope_dir: &Path,
        old_path: &str,
        new_path: &str,
    ) -> Vec<String> {
        let canonical_scope = match scope_dir.canonicalize() {
            Ok(path) => path,
            Err(_) => return Vec::new(),
        };

        let old_ref = format!("@{old_path}");
        let new_ref = format!("@{new_path}");
        let mut failed = Vec::new();
        let mut visited: HashSet<PathBuf> = HashSet::new();
        let mut stack = vec![canonical_scope.clone()];
        visited.insert(canonical_scope.clone());

        while let Some(current) = stack.pop() {
            let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
                continue;
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();

                if name.starts_with('.') {
                    continue;
                }

                let Ok(file_type) = entry.file_type().await else {
                    continue;
                };

                if file_type.is_symlink() {
                    continue;
                }

                let path = entry.path();

                if file_type.is_dir() {
                    let Ok(canonical_dir) = path.canonicalize() else {
                        continue;
                    };
                    if !canonical_dir.starts_with(&canonical_scope) {
                        continue;
                    }
                    if visited.insert(canonical_dir.clone()) {
                        stack.push(canonical_dir);
                    }
                    continue;
                }

                if !file_type.is_file() || path.extension().is_none_or(|ext| ext != "md") {
                    continue;
                }

                let Ok(canonical_file) = path.canonicalize() else {
                    continue;
                };
                if !canonical_file.starts_with(&canonical_scope) {
                    continue;
                }

                if canonical_file.file_name().is_some_and(|n| n == "index.md") {
                    continue;
                }

                let rel_path = canonical_file
                    .strip_prefix(&canonical_scope)
                    .ok().map_or_else(|| canonical_file.to_string_lossy().to_string(), |p| p.to_string_lossy().to_string());

                let content = if let Ok(content) = tokio::fs::read_to_string(&canonical_file).await { content } else {
                    failed.push(rel_path);
                    continue;
                };

                if !content.lines().any(|line| IndexManager::line_has_exact_ref(line, &old_ref)) {
                    continue;
                }

                let rewritten = content
                    .lines()
                    .map(|line| Self::replace_exact_ref(line, &old_ref, &new_ref))
                    .collect::<Vec<_>>()
                    .join("\n");
                let rewritten = if content.ends_with('\n') && !rewritten.ends_with('\n') {
                    format!("{rewritten}\n")
                } else {
                    rewritten
                };

                if tokio::fs::write(&canonical_file, rewritten).await.is_err() {
                    failed.push(rel_path);
                }
            }
        }

        failed
    }

    /// Replace exact `old_ref` occurrences in a line with `new_ref`.
    fn replace_exact_ref(line: &str, old_ref: &str, new_ref: &str) -> String {
        let mut output = String::with_capacity(line.len());
        let mut start = 0;

        while let Some(pos) = line[start..].find(old_ref) {
            let abs_pos = start + pos;
            let after_idx = abs_pos + old_ref.len();

            let is_exact = if after_idx >= line.len() {
                true
            } else {
                matches!(
                    line.as_bytes()[after_idx],
                    b' ' | b'\t' | b'\n' | b'\r' | b')' | b']'
                )
            };

            if is_exact {
                output.push_str(&line[start..abs_pos]);
                output.push_str(new_ref);
                start = after_idx;
            } else {
                let next = abs_pos + 1;
                output.push_str(&line[start..next]);
                start = next;
            }
        }

        output.push_str(&line[start..]);
        output
    }

    /// Scan memory files recursively for dangling `@path` references.
    async fn find_dangling_refs(scope_dir: &Path, old_path: &str) -> Vec<String> {
        let ref_marker = format!("@{old_path}");
        let mut dangling = Vec::new();
        let mut stack = vec![scope_dir.to_path_buf()];

        while let Some(current) = stack.pop() {
            let Ok(mut entries) = tokio::fs::read_dir(&current).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if let Ok(meta) = entry.metadata().await {
                    if meta.is_dir() {
                        if !entry.file_name().to_string_lossy().starts_with('.') {
                            stack.push(path);
                        }
                    } else if meta.is_file()
                        && path.extension().is_some_and(|e| e == "md")
                        && path.file_name().is_none_or(|n| n != "index.md")
                    {
                        if let Ok(content) = tokio::fs::read_to_string(&path).await {
                            let has_ref = content
                                .lines()
                                .any(|line| IndexManager::line_has_exact_ref(line, &ref_marker));
                            if has_ref {
                                if let Ok(rel) = path.strip_prefix(scope_dir) {
                                    dangling.push(rel.to_string_lossy().to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        dangling
    }

    /// Remove empty parent directories up to (but not including) `scope_dir`.
    async fn cleanup_empty_dirs(dir: &Path, scope_dir: &Path) {
        let mut current = dir.to_path_buf();
        while current != scope_dir.to_path_buf() {
            if tokio::fs::remove_dir(&current).await.is_err() {
                break;
            }
            match current.parent() {
                Some(parent) => current = parent.to_path_buf(),
                None => break,
            }
        }
    }

    /// Update the `last_used_at` field in a memory file's YAML frontmatter.
    ///
    /// Best-effort: silently ignores files without frontmatter or I/O errors.
    pub async fn update_last_used_at(
        &self,
        scope: MemoryScope,
        project_dir: Option<&Path>,
        path: &str,
    ) -> Result<(), MemoryError> {
        Self::validate_path(path)?;

        let scope_dir = match self.scope_dir(scope, project_dir) {
            Some(d) => d,
            None => return Ok(()),
        };

        let file_path = scope_dir.join(path);
        if !file_path.exists() {
            return Ok(());
        }

        let safe_path = Self::verify_within_scope(&file_path, &scope_dir)?;
        let content = tokio::fs::read_to_string(&safe_path).await?;

        if !content.starts_with("---") {
            return Ok(());
        }

        let rest = &content[3..];
        let end_pos = match rest.find("\n---") {
            Some(pos) => pos,
            None => return Ok(()),
        };

        let yaml_str = &rest[..end_pos];
        let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();

        let new_yaml = if yaml_str.contains("last_used_at:") {
            yaml_str
                .lines()
                .map(|line| {
                    if line.trim_start().starts_with("last_used_at:") {
                        format!("last_used_at: {now}")
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            format!("{yaml_str}\nlast_used_at: {now}")
        };

        let body = &rest[end_pos + 4..];
        let new_content = format!("---{new_yaml}\n---{body}");
        tokio::fs::write(&safe_path, new_content).await?;

        Ok(())
    }
}
