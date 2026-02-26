//! Memory system type definitions.
//!
//! Core data structures for the structured memory system including
//! metadata, file representation, index entries, and write modes.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// Memory scope — user-level or project-level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    /// User-level memory (`~/.forge/memory/`).
    User,
    /// Project-level memory (`{working_dir}/.forge/memory/`).
    Project,
}

impl std::fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Project => write!(f, "project"),
        }
    }
}

/// Write mode for memory operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Completely overwrite file content.
    Replace,
    /// Append to end of file.
    Append,
    /// Smart merge by section headings (Phase 2).
    Merge,
}

/// Memory file metadata (parsed from YAML frontmatter).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryMeta {
    /// Scope — only required in index.md; topic files infer from directory.
    #[serde(default)]
    pub scope: Option<MemoryScope>,
    /// Date string (YYYY-MM-DD or ISO 8601), auto-set by `MemoryWriter`.
    #[serde(default)]
    pub updated: String,
    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Source: `user_explicit` / `agent_inferred` / `migration`.
    #[serde(default)]
    pub source: Option<String>,
    /// Confidence score (0.0–1.0) for prune candidate ranking.
    #[serde(default)]
    pub confidence: Option<f32>,
    /// Last time this file was read by `memory_read` (ISO 8601).
    #[serde(default)]
    pub last_used_at: Option<String>,
    /// Cumulative write count (append/merge/replace).
    #[serde(default)]
    pub write_count: u32,
}

/// A complete memory file representation.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    /// Relative path from memory root, e.g. `"projects/forge/desktop.md"`.
    pub path: String,
    /// Frontmatter metadata.
    pub meta: MemoryMeta,
    /// Markdown body content (without frontmatter).
    pub content: String,
    /// `@path` references found in the file.
    pub references: Vec<String>,
}

/// An entry in the memory index file.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    /// One-line summary of the entry.
    pub summary: String,
    /// Associated `@path` reference (optional).
    pub reference: Option<String>,
}

/// Structured representation of a memory index (`index.md`).
#[derive(Debug, Clone)]
pub struct MemoryIndex {
    /// Scope of this index.
    pub scope: MemoryScope,
    /// Sections: `(section_title, entries)`.
    pub sections: Vec<(String, Vec<IndexEntry>)>,
}

impl MemoryIndex {
    /// Serialize to system prompt injection format with XML wrapper.
    ///
    /// Output: `<memory scope="user">\n## Section\n- ...\n</memory>`
    /// Total output capped at ~500 tokens; truncated with note if exceeded.
    #[must_use] 
    pub fn to_prompt_string(&self) -> String {
        let mut output = format!("<memory scope=\"{}\">\n", self.scope);

        for (title, entries) in &self.sections {
            let _ = writeln!(output, "## {title}");
            for entry in entries {
                if let Some(ref path) = entry.reference {
                    let _ = writeln!(output, "- {} → @{path}", entry.summary);
                } else {
                    let _ = writeln!(output, "- {}", entry.summary);
                }
            }
            output.push('\n');
        }

        let token_estimate = forge_infra::estimate_tokens_fast(&output);
        if token_estimate > 500 {
            let max_bytes = 500 * 4;
            if output.len() > max_bytes {
                let mut safe_end = max_bytes;
                while safe_end > 0 && !output.is_char_boundary(safe_end) {
                    safe_end -= 1;
                }
                output.truncate(safe_end);
                if let Some(pos) = output.rfind('\n') {
                    output.truncate(pos + 1);
                }
                output.push_str("[索引已截断]\n");
            }
        }

        output.push_str("</memory>");
        output
    }
}

/// Result of a `move_file` operation.
#[derive(Debug, Clone)]
pub struct MoveResult {
    /// `@path` references that could not be automatically updated.
    pub dangling_refs: Vec<String>,
}
