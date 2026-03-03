//! Unit tests for the memory system.

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use tempfile::TempDir;

    use crate::index_manager::IndexManager;
    use crate::loader::MemoryLoader;
    use crate::migration::MemoryMigration;
    use crate::types::*;
    use crate::writer::MemoryWriter;

    /// Helper: create a temp dir and return (temp_dir_handle, path).
    fn temp_scope() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("create temp dir");
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    // ========================
    // IndexManager tests
    // ========================

    #[tokio::test]
    async fn test_index_add_reference_creates_index() {
        let (_dir, scope_dir) = temp_scope();

        IndexManager::add_reference(&scope_dir, "prefs.md", "User preferences")
            .await
            .expect("add reference");

        let content =
            tokio::fs::read_to_string(scope_dir.join("index.md")).await.expect("read index");

        assert!(content.contains("@prefs.md"));
        assert!(content.contains("User preferences"));
    }

    #[tokio::test]
    async fn test_index_add_reference_skips_duplicate() {
        let (_dir, scope_dir) = temp_scope();

        IndexManager::add_reference(&scope_dir, "prefs.md", "Prefs").await.expect("first add");
        IndexManager::add_reference(&scope_dir, "prefs.md", "Prefs again")
            .await
            .expect("second add");

        let content =
            tokio::fs::read_to_string(scope_dir.join("index.md")).await.expect("read index");

        let count = content.matches("@prefs.md").count();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_index_remove_reference() {
        let (_dir, scope_dir) = temp_scope();

        IndexManager::add_reference(&scope_dir, "a.md", "File A").await.expect("add a");
        IndexManager::add_reference(&scope_dir, "b.md", "File B").await.expect("add b");

        IndexManager::remove_reference(&scope_dir, "a.md").await.expect("remove a");

        let content =
            tokio::fs::read_to_string(scope_dir.join("index.md")).await.expect("read index");

        assert!(!content.contains("@a.md"));
        assert!(content.contains("@b.md"));
    }

    #[tokio::test]
    async fn test_index_update_reference() {
        let (_dir, scope_dir) = temp_scope();

        IndexManager::add_reference(&scope_dir, "old.md", "My file").await.expect("add");

        IndexManager::update_reference(&scope_dir, "old.md", "new.md").await.expect("update");

        let content =
            tokio::fs::read_to_string(scope_dir.join("index.md")).await.expect("read index");

        assert!(!content.contains("@old.md"));
        assert!(content.contains("@new.md"));
    }

    // ========================
    // MemoryWriter tests
    // ========================

    #[tokio::test]
    async fn test_writer_write_replace() {
        let (_dir, scope_dir) = temp_scope();
        let writer = MemoryWriter::new(scope_dir.clone());

        writer
            .write_file(
                MemoryScope::User,
                None,
                "prefs.md",
                "# Preferences\n\nDark mode enabled.",
                WriteMode::Replace,
                false,
            )
            .await
            .expect("write file");

        let content =
            tokio::fs::read_to_string(scope_dir.join("prefs.md")).await.expect("read file");

        assert!(content.contains("# Preferences"));
        assert!(content.contains("Dark mode enabled"));
        assert!(content.contains("scope: user"));
    }

    #[tokio::test]
    async fn test_writer_write_append() {
        let (_dir, scope_dir) = temp_scope();
        let writer = MemoryWriter::new(scope_dir.clone());

        writer
            .write_file(
                MemoryScope::User,
                None,
                "log.md",
                "First entry.",
                WriteMode::Replace,
                false,
            )
            .await
            .expect("write");

        writer
            .write_file(
                MemoryScope::User,
                None,
                "log.md",
                "Second entry.",
                WriteMode::Append,
                false,
            )
            .await
            .expect("append");

        let content = tokio::fs::read_to_string(scope_dir.join("log.md")).await.expect("read");

        assert!(content.contains("First entry."));
        assert!(content.contains("Second entry."));
    }

    #[tokio::test]
    async fn test_writer_delete_file() {
        let (_dir, scope_dir) = temp_scope();
        let writer = MemoryWriter::new(scope_dir.clone());

        writer
            .write_file(MemoryScope::User, None, "temp.md", "Temporary.", WriteMode::Replace, false)
            .await
            .expect("write");

        assert!(scope_dir.join("temp.md").exists());

        writer.delete_file(MemoryScope::User, None, "temp.md").await.expect("delete");

        assert!(!scope_dir.join("temp.md").exists());
    }

    #[tokio::test]
    async fn test_writer_move_updates_references_recursively() {
        let (_dir, scope_dir) = temp_scope();
        let writer = MemoryWriter::new(scope_dir.clone());

        writer
            .write_file(
                MemoryScope::User,
                None,
                "projects/old.md",
                "# Old\n\ncontent",
                WriteMode::Replace,
                false,
            )
            .await
            .expect("write source");

        writer
            .write_file(
                MemoryScope::User,
                None,
                "root_ref.md",
                "# Ref\n\nsee @projects/old.md",
                WriteMode::Replace,
                false,
            )
            .await
            .expect("write root ref");

        writer
            .write_file(
                MemoryScope::User,
                None,
                "nested/child_ref.md",
                "# Child\n\nlink(@projects/old.md)",
                WriteMode::Replace,
                false,
            )
            .await
            .expect("write nested ref");

        writer
            .write_file(
                MemoryScope::User,
                None,
                "substring_ref.md",
                "# Substring\n\nkeep @projects/old.md.bak",
                WriteMode::Replace,
                false,
            )
            .await
            .expect("write substring ref");

        let result = writer
            .move_file(MemoryScope::User, None, "projects/old.md", "projects/new.md")
            .await
            .expect("move");

        assert!(result.dangling_refs.is_empty());
        assert!(scope_dir.join("projects/new.md").exists());
        assert!(!scope_dir.join("projects/old.md").exists());

        let root_ref =
            tokio::fs::read_to_string(scope_dir.join("root_ref.md")).await.expect("read root ref");
        assert!(root_ref.contains("@projects/new.md"));
        assert!(!root_ref.contains("@projects/old.md"));

        let child_ref = tokio::fs::read_to_string(scope_dir.join("nested/child_ref.md"))
            .await
            .expect("read child ref");
        assert!(child_ref.contains("@projects/new.md"));
        assert!(!child_ref.contains("@projects/old.md)"));

        let substring_ref = tokio::fs::read_to_string(scope_dir.join("substring_ref.md"))
            .await
            .expect("read substring ref");
        assert!(substring_ref.contains("@projects/old.md.bak"));
    }

    // ========================
    // MemoryLoader tests
    // ========================

    #[tokio::test]
    async fn test_loader_load_index() {
        let (_dir, scope_dir) = temp_scope();
        let writer = MemoryWriter::new(scope_dir.clone());
        let loader = MemoryLoader::new(scope_dir.clone());

        writer
            .write_file(
                MemoryScope::User,
                None,
                "prefs.md",
                "# Preferences",
                WriteMode::Replace,
                false,
            )
            .await
            .expect("write prefs");

        writer
            .write_file(MemoryScope::User, None, "notes.md", "# Notes", WriteMode::Replace, false)
            .await
            .expect("write notes");

        let index = loader.load_index(MemoryScope::User, None).await.expect("load index");

        assert!(index.is_some());
        let idx = index.expect("index exists");
        assert_eq!(idx.scope, MemoryScope::User);

        let prompt = idx.to_prompt_string();
        assert!(prompt.contains("@prefs.md"));
        assert!(prompt.contains("@notes.md"));
    }

    #[tokio::test]
    async fn test_loader_read_file() {
        let (_dir, scope_dir) = temp_scope();
        let writer = MemoryWriter::new(scope_dir.clone());
        let loader = MemoryLoader::new(scope_dir.clone());

        writer
            .write_file(
                MemoryScope::User,
                None,
                "prefs.md",
                "# Preferences\n\nVim mode.",
                WriteMode::Replace,
                false,
            )
            .await
            .expect("write");

        let file = loader.read_file(MemoryScope::User, None, "prefs.md").await.expect("read");

        assert!(file.is_some());
        let f = file.expect("file exists");
        assert_eq!(f.path, "prefs.md");
        assert!(f.content.contains("Vim mode"));
    }

    #[tokio::test]
    async fn test_loader_read_nonexistent_returns_none() {
        let (_dir, scope_dir) = temp_scope();
        let loader = MemoryLoader::new(scope_dir);

        let file = loader.read_file(MemoryScope::User, None, "nope.md").await.expect("read");

        assert!(file.is_none());
    }

    #[tokio::test]
    async fn test_loader_read_file_raw_not_truncated() {
        let (_dir, scope_dir) = temp_scope();
        let loader = MemoryLoader::new(scope_dir.clone());

        let large_body = "A".repeat(9000);
        let content = format!("---\nscope: user\nupdated: 2026-02-08\n---\n\n{large_body}\n");
        tokio::fs::write(scope_dir.join("large.md"), content).await.expect("write large file");

        let truncated = loader
            .read_file(MemoryScope::User, None, "large.md")
            .await
            .expect("read truncated")
            .expect("file exists");
        assert!(truncated.content.contains("[内容已截断"));

        let raw = loader
            .read_file_raw(MemoryScope::User, None, "large.md")
            .await
            .expect("read raw")
            .expect("file exists");
        assert!(!raw.content.contains("[内容已截断"));
        assert!(raw.content.len() > truncated.content.len());
    }

    #[tokio::test]
    async fn test_path_traversal_rejected() {
        let (_dir, scope_dir) = temp_scope();
        let loader = MemoryLoader::new(scope_dir);

        let result = loader.read_file(MemoryScope::User, None, "../etc/passwd").await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Path traversal"));
    }

    // ========================
    // Migration tests
    // ========================

    #[tokio::test]
    async fn test_migration_splits_sections() {
        let (_dir, scope_dir) = temp_scope();
        let legacy_path = scope_dir.join("memory.md");
        let memory_dir = scope_dir.join("memory");

        let legacy_content = "\
## Preferences\n\
Dark mode enabled.\n\
Vim keybindings.\n\
\n\
## Project Notes\n\
Working on forge.\n";

        tokio::fs::write(&legacy_path, legacy_content).await.expect("write legacy");

        let result = MemoryMigration::migrate(&legacy_path, &memory_dir, MemoryScope::User)
            .await
            .expect("migrate");

        assert!(result.migrated);
        assert_eq!(result.files_created, 2);
        assert!(!legacy_path.exists());
        assert!(result.backup_path.exists());
    }

    // ========================
    // Sensitive content tests
    // ========================

    #[tokio::test]
    async fn test_sensitive_content_rejected_by_default() {
        let (_dir, scope_dir) = temp_scope();
        let writer = MemoryWriter::new(scope_dir);

        let result = writer
            .write_file(
                MemoryScope::User,
                None,
                "secrets.md",
                "api_key = sk-abcdefghijklmnopqrstuvwxyz1234567890",
                WriteMode::Replace,
                false,
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Sensitive content detected"));
    }

    #[tokio::test]
    async fn test_sensitive_content_allowed_with_flag() {
        let (_dir, scope_dir) = temp_scope();
        let writer = MemoryWriter::new(scope_dir.clone());

        let result = writer
            .write_file(
                MemoryScope::User,
                None,
                "secrets.md",
                "api_key = sk-abcdefghijklmnopqrstuvwxyz1234567890",
                WriteMode::Replace,
                true,
            )
            .await;

        assert!(result.is_ok(), "allow_sensitive=true should bypass API key check");
        assert!(scope_dir.join("secrets.md").exists());
    }

    #[tokio::test]
    async fn test_high_risk_rejected_even_with_allow() {
        let (_dir, scope_dir) = temp_scope();
        let writer = MemoryWriter::new(scope_dir);

        let content =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA...\n-----END RSA PRIVATE KEY-----";
        let result = writer
            .write_file(MemoryScope::User, None, "key.md", content, WriteMode::Replace, true)
            .await;

        assert!(
            result.is_err(),
            "private key block must be rejected even with allow_sensitive=true"
        );
        let err = result.unwrap_err().to_string();
        assert!(err.contains("private key block"));
    }
}
