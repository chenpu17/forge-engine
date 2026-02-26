//! Project documentation discovery.

use std::path::{Path, PathBuf};

/// Project prompt document loader.
///
/// Discovers and loads project documentation files (CLAUDE.md, FORGE.md)
/// to inject into the system prompt.
pub struct ProjectPromptLoader {
    /// Global document path (~/.forge/FORGE.md).
    global_path: Option<PathBuf>,
    /// Supported document filenames (in priority order).
    doc_names: Vec<&'static str>,
}

impl ProjectPromptLoader {
    /// Create a new project prompt loader.
    #[must_use]
    pub fn new() -> Self {
        let global_path =
            dirs::home_dir().map(|d| d.join(".forge/FORGE.md"));
        Self {
            global_path,
            doc_names: vec![
                "CLAUDE.md",
                "FORGE.md",
                ".claude.md",
                ".forge.md",
            ],
        }
    }

    /// Load project documentation from the given directory.
    ///
    /// Returns combined content from global and project-level docs,
    /// or `None` if no documents are found.
    #[must_use]
    pub fn load(&self, project_dir: impl AsRef<Path>) -> Option<String> {
        let project_dir = project_dir.as_ref();
        let mut content = String::new();

        // 1. Global document
        if let Some(ref global) = self.global_path {
            if global.exists() {
                if let Ok(text) = std::fs::read_to_string(global) {
                    tracing::debug!("Loaded global project prompt from {global:?}");
                    content.push_str("# Global Instructions\n\n");
                    content.push_str(&text);
                    content.push_str("\n\n---\n\n");
                }
            }
        }

        // 2. Project document (first match only)
        for name in &self.doc_names {
            let path = project_dir.join(name);
            if path.exists() {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    tracing::debug!("Loaded project prompt from {path:?}");
                    content.push_str("# Project Instructions\n\n");
                    content.push_str(&text);
                    break;
                }
            }
        }

        if content.is_empty() {
            None
        } else {
            Some(content)
        }
    }

    /// Load project documentation, returning empty string if none found.
    #[must_use]
    pub fn load_or_default(&self, project_dir: impl AsRef<Path>) -> String {
        self.load(project_dir).unwrap_or_default()
    }

    /// Check if any project documentation exists.
    #[must_use]
    pub fn has_docs(&self, project_dir: impl AsRef<Path>) -> bool {
        let project_dir = project_dir.as_ref();
        if let Some(ref global) = self.global_path {
            if global.exists() {
                return true;
            }
        }
        self.doc_names
            .iter()
            .any(|name| project_dir.join(name).exists())
    }

    /// Get the path of the first found project document.
    #[must_use]
    pub fn find_doc_path(
        &self,
        project_dir: impl AsRef<Path>,
    ) -> Option<PathBuf> {
        let project_dir = project_dir.as_ref();
        self.doc_names
            .iter()
            .map(|name| project_dir.join(name))
            .find(|path| path.exists())
    }
}

impl Default for ProjectPromptLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_project_doc() {
        let dir = tempfile::tempdir().expect("create temp dir");
        std::fs::write(dir.path().join("FORGE.md"), "# My Project")
            .expect("write");

        let loader = ProjectPromptLoader {
            global_path: None,
            doc_names: vec!["FORGE.md"],
        };
        let content = loader.load(dir.path());
        assert!(content.is_some());
        assert!(content.unwrap().contains("My Project"));
    }

    #[test]
    fn test_no_docs() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let loader = ProjectPromptLoader {
            global_path: None,
            doc_names: vec!["FORGE.md"],
        };
        assert!(!loader.has_docs(dir.path()));
        assert!(loader.load(dir.path()).is_none());
    }
}
