//! Memory management for ForgeSDK

use std::collections::HashMap;

use super::*;

impl ForgeSDK {
    fn user_memory_path(&self) -> PathBuf {
        self.memory_dir.join("memory.md")
    }

    fn project_memory_path(working_dir: &std::path::Path) -> PathBuf {
        working_dir.join(".forge").join("memory.md")
    }

    fn user_memory_dir(&self) -> PathBuf {
        self.memory_dir.join("memory")
    }

    fn project_memory_dir(working_dir: &std::path::Path) -> PathBuf {
        working_dir.join(".forge").join("memory")
    }

    pub(super) async fn resolve_tool_env(&self) -> HashMap<String, String> {
        let policy = { self.config.read().await.tools.env_policy.clone() };
        {
            let cache = self.env_cache.read().await;
            if cache.initialized && cache.policy == policy {
                return cache.env.clone();
            }
        }
        let env = policy.apply(std::env::vars());
        let mut cache = self.env_cache.write().await;
        cache.policy = policy;
        cache.env = env.clone();
        cache.initialized = true;
        env
    }

    async fn read_memory_file(path: &std::path::Path) -> Option<String> {
        const MAX_CHARS: usize = 32_000;
        let text = tokio::fs::read_to_string(path).await.ok()?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.chars().count() <= MAX_CHARS {
            return Some(trimmed.to_string());
        }
        Some(trimmed.chars().rev().take(MAX_CHARS).collect::<String>().chars().rev().collect())
    }

    async fn refresh_cached_memory(
        path: &std::path::Path,
        cached: CachedMemoryFile,
    ) -> CachedMemoryFile {
        let metadata = tokio::fs::metadata(path).await.ok();
        let mtime = metadata.and_then(|m| m.modified().ok());
        if mtime.is_none() {
            return CachedMemoryFile { mtime: None, content: None };
        }
        if cached.mtime == mtime {
            return cached;
        }
        let content = Self::read_memory_file(path).await;
        CachedMemoryFile { mtime, content }
    }

    async fn load_structured_memory_prompt(
        &self,
        memory_dir: &std::path::Path,
        scope: forge_memory::MemoryScope,
        cached: CachedMemoryFile,
    ) -> (CachedMemoryFile, Option<String>) {
        let index_path = memory_dir.join("index.md");
        let metadata = tokio::fs::metadata(&index_path).await.ok();
        let mtime = metadata.and_then(|m| m.modified().ok());
        if mtime.is_none() {
            return (CachedMemoryFile { mtime: None, content: None }, None);
        }
        if cached.mtime == mtime && cached.content.is_some() {
            let content = cached.content.clone();
            return (cached, content);
        }

        let loader = forge_memory::MemoryLoader::new(self.user_memory_dir());
        let index = match scope {
            forge_memory::MemoryScope::User => loader.load_index(scope, None).await.ok().flatten(),
            forge_memory::MemoryScope::Project => {
                loader.load_index(scope, Some(memory_dir)).await.ok().flatten()
            }
        };

        let content = index.map(|idx| idx.to_prompt_string());
        let new_cached = CachedMemoryFile { mtime, content: content.clone() };
        (new_cached, content)
    }

    pub(super) async fn build_project_prompt_with_memory(
        &self,
        base: Option<String>,
        working_dir: &std::path::Path,
    ) -> Option<String> {
        let base = base.and_then(|s| {
            let t = s.trim().to_string();
            (!t.is_empty()).then_some(t)
        });
        self.run_memory_migration(working_dir).await;
        base
    }

    async fn run_memory_migration(&self, working_dir: &std::path::Path) {
        let user_struct_dir = self.user_memory_dir();
        let user_legacy = self.user_memory_path();
        if forge_memory::MemoryMigration::needs_migration(&user_legacy, &user_struct_dir) {
            if let Err(err) = forge_memory::MemoryMigration::migrate(
                &user_legacy,
                &user_struct_dir,
                forge_memory::MemoryScope::User,
            )
            .await
            {
                tracing::warn!("Failed to migrate user memory.md: {err}");
            }
        }

        let project_struct_dir = Self::project_memory_dir(working_dir);
        let project_legacy = Self::project_memory_path(working_dir);
        if forge_memory::MemoryMigration::needs_migration(&project_legacy, &project_struct_dir) {
            if let Err(err) = forge_memory::MemoryMigration::migrate(
                &project_legacy,
                &project_struct_dir,
                forge_memory::MemoryScope::Project,
            )
            .await
            {
                tracing::warn!("Failed to migrate project memory.md: {err}");
            }
        }
    }

    /// Load memory indexes for user and project scopes.
    pub(super) async fn load_memory_indexes(
        &self,
        working_dir: &std::path::Path,
    ) -> (Option<String>, Option<String>) {
        let memory_settings = { self.config.read().await.tools.memory.clone() };
        if !memory_settings.can_read() {
            return (None, None);
        }

        let user_struct_dir = self.user_memory_dir();
        let project_struct_dir = Self::project_memory_dir(working_dir);

        let (user_struct_cached, project_struct_cached) = {
            let cache = self.memory_prompt_cache.read().await;
            (
                cache.user_structured.clone(),
                cache.projects_structured.get(working_dir).cloned().unwrap_or_default(),
            )
        };

        let (user_struct_cached, user_struct_mem) = self
            .load_structured_memory_prompt(
                &user_struct_dir,
                forge_memory::MemoryScope::User,
                user_struct_cached,
            )
            .await;

        let (project_struct_cached, project_struct_mem) = self
            .load_structured_memory_prompt(
                &project_struct_dir,
                forge_memory::MemoryScope::Project,
                project_struct_cached,
            )
            .await;

        let user_mem = if user_struct_mem.is_some() {
            user_struct_mem
        } else {
            let user_path = self.user_memory_path();
            let cached = { self.memory_prompt_cache.read().await.user.clone() };
            let cached = Self::refresh_cached_memory(&user_path, cached).await;
            {
                let mut cache = self.memory_prompt_cache.write().await;
                cache.user = cached.clone();
            }
            cached.content.map(|c| format!("<memory scope=\"user\">\n{c}\n</memory>"))
        };

        let project_mem = if project_struct_mem.is_some() {
            project_struct_mem
        } else {
            let project_path = Self::project_memory_path(working_dir);
            let cached = {
                self.memory_prompt_cache
                    .read()
                    .await
                    .projects
                    .get(working_dir)
                    .cloned()
                    .unwrap_or_default()
            };
            let cached = Self::refresh_cached_memory(&project_path, cached).await;
            {
                let mut cache = self.memory_prompt_cache.write().await;
                cache.projects.insert(working_dir.to_path_buf(), cached.clone());
            }
            cached.content.map(|c| format!("<memory scope=\"project\">\n{c}\n</memory>"))
        };

        {
            let mut cache = self.memory_prompt_cache.write().await;
            cache.user_structured = user_struct_cached;
            cache.projects_structured.insert(working_dir.to_path_buf(), project_struct_cached);
        }

        (user_mem, project_mem)
    }

    /// Append a memory entry.
    ///
    /// # Errors
    ///
    /// Returns error if write fails.
    pub async fn add_memory(&self, scope: MemoryScope, content: &str) -> Result<()> {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let working_dir = { self.config.read().await.working_dir.clone() };
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        let entry = format!("### {ts}\n{trimmed}\n");

        let infra_scope = match scope {
            MemoryScope::User => forge_memory::MemoryScope::User,
            MemoryScope::Project => forge_memory::MemoryScope::Project,
        };
        let project_dir = Self::project_memory_dir(&working_dir);
        let writer = forge_memory::MemoryWriter::new(self.user_memory_dir());
        writer
            .write_file(
                infra_scope,
                Some(&project_dir),
                "notes.md",
                &entry,
                forge_memory::WriteMode::Append,
                false,
            )
            .await
            .map_err(|e| ForgeError::StorageError(e.to_string()))?;
        Ok(())
    }

    /// Read memory content for display/debugging.
    ///
    /// # Errors
    ///
    /// Returns error if read fails.
    pub async fn get_memory(&self, scope: MemoryScope) -> Result<Option<String>> {
        let working_dir = { self.config.read().await.working_dir.clone() };
        let infra_scope = match scope {
            MemoryScope::User => forge_memory::MemoryScope::User,
            MemoryScope::Project => forge_memory::MemoryScope::Project,
        };
        let project_dir = Self::project_memory_dir(&working_dir);
        let loader = forge_memory::MemoryLoader::new(self.user_memory_dir());

        let files =
            loader.list_files_recursive(infra_scope, Some(&project_dir)).await.unwrap_or_default();

        if !files.is_empty() {
            let mut parts = Vec::new();
            for (file_path, _summary) in &files {
                if file_path == "index.md" {
                    continue;
                }
                if let Ok(Some(mf)) =
                    loader.read_file_raw(infra_scope, Some(&project_dir), file_path).await
                {
                    if !mf.content.trim().is_empty() {
                        parts.push(format!("## {file_path}\n\n{}", mf.content.trim()));
                    }
                }
            }
            if !parts.is_empty() {
                return Ok(Some(parts.join("\n\n")));
            }
        }

        let legacy_path = match scope {
            MemoryScope::User => self.user_memory_path(),
            MemoryScope::Project => Self::project_memory_path(&working_dir),
        };
        Ok(Self::read_memory_file(&legacy_path).await)
    }

    pub(super) async fn persist_active_session_snapshot(&self) -> Result<()> {
        let snapshot = {
            let guard = self.active_session.read().await;
            guard.as_ref().cloned()
        };
        if let Some(session) = snapshot {
            self.session_manager.update(&session).await?;
        }
        Ok(())
    }
}
