//! Tool Description Loader
//!
//! Loads tool descriptions from external markdown files at runtime.
//! This allows updating tool descriptions without recompiling.
//!
//! # Customization
//!
//! External projects using `ForgeSDK` can customize tool descriptions via:
//!
//! 1. **Override API**: Register custom descriptions programmatically
//!    ```ignore
//!    ToolDescriptions::register_override("read", "Custom read description...");
//!    ToolDescriptions::register_overrides(hashmap);
//!    ```
//!
//! 2. **Directory Configuration**: Load from custom directory
//!    ```ignore
//!    ToolDescriptions::init(Some(Path::new("/custom/prompts/tools")));
//!    ```
//!
//! Overrides take precedence over loaded files, which take precedence over fallbacks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use tracing::{debug, warn};

/// Global tool descriptions cache (loaded from files)
static DESCRIPTIONS: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Custom overrides (set programmatically, takes precedence over DESCRIPTIONS)
static OVERRIDES: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();

/// Default prompts directory relative to the executable or config
const DEFAULT_PROMPTS_DIR: &str = "prompts/tools";

/// Tool description loader
pub struct ToolDescriptions;

impl ToolDescriptions {
    /// Initialize the descriptions cache from the given directory
    pub fn init(prompts_dir: Option<&Path>) {
        let _ = DESCRIPTIONS.get_or_init(|| {
            let mut descriptions = HashMap::new();

            // Try to find prompts directory
            let dir = prompts_dir.map(Path::to_path_buf).or_else(Self::find_prompts_dir);

            if let Some(dir) = dir {
                if dir.exists() {
                    debug!("Loading tool descriptions from: {}", dir.display());
                    Self::load_from_dir(&dir, &mut descriptions);
                } else {
                    debug!("Prompts directory not found: {}", dir.display());
                }
            }

            descriptions
        });
    }

    /// Register a single override description for a tool
    ///
    /// Overrides take precedence over loaded descriptions.
    /// Call this BEFORE creating tool instances.
    ///
    /// # Example
    ///
    /// ```ignore
    /// ToolDescriptions::register_override("read", "Custom read tool description...");
    /// ```
    pub fn register_override(tool_name: impl Into<String>, description: impl Into<String>) {
        let overrides = OVERRIDES.get_or_init(|| RwLock::new(HashMap::new()));
        if let Ok(mut guard) = overrides.write() {
            guard.insert(tool_name.into(), description.into());
        }
    }

    /// Register multiple override descriptions at once
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut overrides = HashMap::new();
    /// overrides.insert("read".to_string(), "Custom read description".to_string());
    /// overrides.insert("write".to_string(), "Custom write description".to_string());
    /// ToolDescriptions::register_overrides(overrides);
    /// ```
    pub fn register_overrides(descriptions: HashMap<String, String>) {
        let overrides = OVERRIDES.get_or_init(|| RwLock::new(HashMap::new()));
        if let Ok(mut guard) = overrides.write() {
            guard.extend(descriptions);
        }
    }

    /// Clear all registered overrides
    ///
    /// Useful for testing or resetting state.
    pub fn clear_overrides() {
        if let Some(overrides) = OVERRIDES.get() {
            if let Ok(mut guard) = overrides.write() {
                guard.clear();
            }
        }
    }

    /// Get the current override for a tool (if any)
    pub fn get_override(tool_name: &str) -> Option<String> {
        OVERRIDES.get().and_then(|o| o.read().ok()).and_then(|guard| guard.get(tool_name).cloned())
    }

    /// List all registered overrides
    pub fn list_overrides() -> Vec<String> {
        OVERRIDES
            .get()
            .and_then(|o| o.read().ok())
            .map(|guard| guard.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Find the prompts directory
    fn find_prompts_dir() -> Option<PathBuf> {
        // Try relative to current directory
        let cwd = std::env::current_dir().ok()?;
        let relative = cwd.join(DEFAULT_PROMPTS_DIR);
        if relative.exists() {
            return Some(relative);
        }

        // Try relative to executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let exe_relative = exe_dir.join(DEFAULT_PROMPTS_DIR);
                if exe_relative.exists() {
                    return Some(exe_relative);
                }

                // Try one level up (for development)
                if let Some(parent) = exe_dir.parent() {
                    let parent_relative = parent.join(DEFAULT_PROMPTS_DIR);
                    if parent_relative.exists() {
                        return Some(parent_relative);
                    }
                }
            }
        }

        // Try config directory (~/.forge/prompts/tools)
        if let Some(home_dir) = dirs::home_dir() {
            let config_relative = home_dir.join(".forge").join("prompts/tools");
            if config_relative.exists() {
                return Some(config_relative);
            }
        }

        None
    }

    /// Load descriptions from a directory
    fn load_from_dir(dir: &Path, descriptions: &mut HashMap<String, String>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                warn!("Failed to read prompts directory: {}", e);
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "md") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    match std::fs::read_to_string(&path) {
                        Ok(content) => {
                            debug!("Loaded description for tool: {}", stem);
                            descriptions.insert(stem.to_string(), content);
                        }
                        Err(e) => {
                            warn!("Failed to read {}: {}", path.display(), e);
                        }
                    }
                }
            }
        }

        debug!("Loaded {} tool descriptions", descriptions.len());
    }

    /// Get the description for a tool
    ///
    /// Priority order:
    /// 1. Registered overrides (highest)
    /// 2. Loaded from external markdown files
    /// 3. Fallback string (lowest)
    pub fn get(tool_name: &str, fallback: &str) -> String {
        // 1. Check overrides first (highest priority)
        if let Some(override_desc) = Self::get_override(tool_name) {
            return override_desc;
        }

        // 2. Check loaded descriptions
        // Ensure initialized
        let _ = DESCRIPTIONS.get_or_init(|| {
            let mut descriptions = HashMap::new();
            if let Some(dir) = Self::find_prompts_dir() {
                Self::load_from_dir(&dir, &mut descriptions);
            }
            descriptions
        });

        DESCRIPTIONS
            .get()
            .and_then(|d| d.get(tool_name))
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    }

    /// Get the short description (first paragraph) for a tool
    #[must_use]
    pub fn get_short(tool_name: &str, fallback: &str) -> String {
        let full = Self::get(tool_name, fallback);

        // Extract first paragraph (skip title if present)
        let lines: Vec<&str> = full.lines().collect();
        let mut result = Vec::new();
        let mut started = false;

        for line in lines {
            // Skip title line
            if line.starts_with('#') {
                continue;
            }

            // Skip empty lines before content
            if !started && line.trim().is_empty() {
                continue;
            }

            // Start collecting
            started = true;

            // Stop at next empty line or section
            if started && (line.trim().is_empty() || line.starts_with('#')) {
                break;
            }

            result.push(line);
        }

        if result.is_empty() {
            fallback.to_string()
        } else {
            result.join(" ")
        }
    }

    /// Check if external descriptions are available
    pub fn is_available() -> bool {
        DESCRIPTIONS.get().is_some_and(|d| !d.is_empty())
    }

    /// List all available tool descriptions
    pub fn list() -> Vec<String> {
        DESCRIPTIONS.get().map(|d| d.keys().cloned().collect()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static OVERRIDE_TEST_LOCK: Mutex<()> = Mutex::new(());

    // ==================== Fallback Tests ====================

    #[test]
    fn test_get_with_fallback() {
        let desc = ToolDescriptions::get("nonexistent_tool", "fallback description");
        assert_eq!(desc, "fallback description");
    }

    #[test]
    fn test_get_short_with_fallback() {
        let desc = ToolDescriptions::get_short("nonexistent_tool", "short fallback");
        assert_eq!(desc, "short fallback");
    }

    #[test]
    fn test_get_with_empty_fallback() {
        let desc = ToolDescriptions::get("nonexistent_tool_2", "");
        assert_eq!(desc, "");
    }

    // ==================== Short Description Extraction Tests ====================

    /// Helper function to test short description extraction logic
    fn extract_short(content: &str, fallback: &str) -> String {
        // Replicate the get_short logic for testing
        let lines: Vec<&str> = content.lines().collect();
        let mut result = Vec::new();
        let mut started = false;

        for line in lines {
            if line.starts_with('#') {
                continue;
            }
            if !started && line.trim().is_empty() {
                continue;
            }
            started = true;
            if started && (line.trim().is_empty() || line.starts_with('#')) {
                break;
            }
            result.push(line);
        }

        if result.is_empty() {
            fallback.to_string()
        } else {
            result.join(" ")
        }
    }

    #[test]
    fn test_extract_short_simple() {
        let content = "This is a simple description.";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "This is a simple description.");
    }

    #[test]
    fn test_extract_short_with_title() {
        let content = "# Tool Name\n\nThis is the description.";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "This is the description.");
    }

    #[test]
    fn test_extract_short_multiline_paragraph() {
        let content = "# Tool Name\n\nFirst line of description.\nSecond line of description.";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "First line of description. Second line of description.");
    }

    #[test]
    fn test_extract_short_stops_at_empty_line() {
        let content = "# Tool Name\n\nFirst paragraph.\n\nSecond paragraph.";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "First paragraph.");
    }

    #[test]
    fn test_extract_short_stops_at_section() {
        // Note: The section header must be on its own line after content
        // to be detected as a section break
        let content = "# Tool Name\n\nDescription.\n\n## Usage\nUsage info.";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "Description.");
    }

    #[test]
    fn test_extract_short_empty_content() {
        let content = "";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "fallback");
    }

    #[test]
    fn test_extract_short_only_title() {
        let content = "# Tool Name";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "fallback");
    }

    #[test]
    fn test_extract_short_only_empty_lines() {
        let content = "\n\n\n";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "fallback");
    }

    #[test]
    fn test_extract_short_with_code_block() {
        let content = "# Tool\n\nDescription here.\n\n```rust\ncode\n```";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "Description here.");
    }

    #[test]
    fn test_extract_short_preserves_inline_formatting() {
        let content = "# Tool\n\nThis has **bold** and `code` formatting.";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "This has **bold** and `code` formatting.");
    }

    // ==================== Load From Directory Tests ====================

    #[test]
    fn test_load_from_dir_empty() {
        let temp_dir = TempDir::new().unwrap();
        let mut descriptions = HashMap::new();

        ToolDescriptions::load_from_dir(temp_dir.path(), &mut descriptions);

        assert!(descriptions.is_empty());
    }

    #[test]
    fn test_load_from_dir_with_md_files() {
        let temp_dir = TempDir::new().unwrap();

        // Create test markdown files
        std::fs::write(temp_dir.path().join("read.md"), "# Read Tool\n\nReads files from disk.")
            .unwrap();
        std::fs::write(temp_dir.path().join("write.md"), "# Write Tool\n\nWrites files to disk.")
            .unwrap();

        let mut descriptions = HashMap::new();
        ToolDescriptions::load_from_dir(temp_dir.path(), &mut descriptions);

        assert_eq!(descriptions.len(), 2);
        assert!(descriptions.contains_key("read"));
        assert!(descriptions.contains_key("write"));
        assert!(descriptions["read"].contains("Reads files"));
        assert!(descriptions["write"].contains("Writes files"));
    }

    #[test]
    fn test_load_from_dir_ignores_non_md_files() {
        let temp_dir = TempDir::new().unwrap();

        // Create various files
        std::fs::write(temp_dir.path().join("tool.md"), "# Tool\n\nDescription.").unwrap();
        std::fs::write(temp_dir.path().join("readme.txt"), "Not a tool").unwrap();
        std::fs::write(temp_dir.path().join("config.json"), "{}").unwrap();

        let mut descriptions = HashMap::new();
        ToolDescriptions::load_from_dir(temp_dir.path(), &mut descriptions);

        assert_eq!(descriptions.len(), 1);
        assert!(descriptions.contains_key("tool"));
    }

    #[test]
    fn test_load_from_dir_handles_subdirectories() {
        let temp_dir = TempDir::new().unwrap();

        // Create a subdirectory (should be ignored)
        std::fs::create_dir(temp_dir.path().join("subdir")).unwrap();
        std::fs::write(
            temp_dir.path().join("subdir").join("nested.md"),
            "# Nested\n\nNested description.",
        )
        .unwrap();

        // Create a file in root
        std::fs::write(temp_dir.path().join("root.md"), "# Root\n\nRoot description.").unwrap();

        let mut descriptions = HashMap::new();
        ToolDescriptions::load_from_dir(temp_dir.path(), &mut descriptions);

        // Should only load root.md, not nested.md
        assert_eq!(descriptions.len(), 1);
        assert!(descriptions.contains_key("root"));
    }

    #[test]
    fn test_load_from_dir_nonexistent() {
        let mut descriptions = HashMap::new();
        ToolDescriptions::load_from_dir(Path::new("/nonexistent/path"), &mut descriptions);

        assert!(descriptions.is_empty());
    }

    #[test]
    fn test_load_from_dir_unicode_content() {
        let temp_dir = TempDir::new().unwrap();

        std::fs::write(
            temp_dir.path().join("unicode.md"),
            "# 工具名称\n\n这是一个中文描述。\n\n日本語の説明。",
        )
        .unwrap();

        let mut descriptions = HashMap::new();
        ToolDescriptions::load_from_dir(temp_dir.path(), &mut descriptions);

        assert_eq!(descriptions.len(), 1);
        assert!(descriptions["unicode"].contains("中文描述"));
    }

    #[test]
    fn test_load_from_dir_empty_file() {
        let temp_dir = TempDir::new().unwrap();

        std::fs::write(temp_dir.path().join("empty.md"), "").unwrap();

        let mut descriptions = HashMap::new();
        ToolDescriptions::load_from_dir(temp_dir.path(), &mut descriptions);

        assert_eq!(descriptions.len(), 1);
        assert_eq!(descriptions["empty"], "");
    }

    #[test]
    fn test_load_from_dir_large_file() {
        let temp_dir = TempDir::new().unwrap();

        // Create a large file
        let large_content = "# Large Tool\n\n".to_string() + &"x".repeat(100_000);
        std::fs::write(temp_dir.path().join("large.md"), &large_content).unwrap();

        let mut descriptions = HashMap::new();
        ToolDescriptions::load_from_dir(temp_dir.path(), &mut descriptions);

        assert_eq!(descriptions.len(), 1);
        assert_eq!(descriptions["large"].len(), large_content.len());
    }

    // ==================== Constants Tests ====================

    #[test]
    fn test_default_prompts_dir() {
        assert_eq!(DEFAULT_PROMPTS_DIR, "prompts/tools");
    }

    // ==================== List and Availability Tests ====================

    #[test]
    fn test_list_returns_vec() {
        let list = ToolDescriptions::list();
        // Just verify it returns a Vec<String> without panicking
        // The list may be empty or have items depending on initialization
        let _ = list;
    }

    #[test]
    fn test_is_available_returns_bool() {
        // Just verify is_available() returns without panicking
        // The result depends on whether tool descriptions are initialized
        let _available: bool = ToolDescriptions::is_available();
    }

    // ==================== File Stem Extraction Tests ====================

    #[test]
    fn test_file_stem_extraction() {
        let temp_dir = TempDir::new().unwrap();

        // Test various file names
        std::fs::write(temp_dir.path().join("simple.md"), "content").unwrap();
        std::fs::write(temp_dir.path().join("with-dash.md"), "content").unwrap();
        std::fs::write(temp_dir.path().join("with_underscore.md"), "content").unwrap();
        std::fs::write(temp_dir.path().join("CamelCase.md"), "content").unwrap();

        let mut descriptions = HashMap::new();
        ToolDescriptions::load_from_dir(temp_dir.path(), &mut descriptions);

        assert_eq!(descriptions.len(), 4);
        assert!(descriptions.contains_key("simple"));
        assert!(descriptions.contains_key("with-dash"));
        assert!(descriptions.contains_key("with_underscore"));
        assert!(descriptions.contains_key("CamelCase"));
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_get_short_with_only_whitespace_lines() {
        let content = "   \n   \n   ";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "fallback");
    }

    #[test]
    fn test_get_short_with_multiple_titles() {
        let content = "# Title 1\n# Title 2\n\nActual content.";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "Actual content.");
    }

    #[test]
    fn test_get_short_with_bullet_list() {
        let content = "# Tool\n\n- Item 1\n- Item 2";
        let result = extract_short(content, "fallback");
        assert_eq!(result, "- Item 1 - Item 2");
    }

    // ==================== Override Tests ====================

    #[test]
    fn test_register_override() {
        let _guard = OVERRIDE_TEST_LOCK.lock().unwrap();

        // Clear any existing overrides first
        ToolDescriptions::clear_overrides();

        // Register an override
        ToolDescriptions::register_override("test_tool_override", "Custom override description");

        // Check it was registered
        let result = ToolDescriptions::get_override("test_tool_override");
        assert_eq!(result, Some("Custom override description".to_string()));

        // Check get() returns the override
        let desc = ToolDescriptions::get("test_tool_override", "fallback");
        assert_eq!(desc, "Custom override description");

        // Cleanup
        ToolDescriptions::clear_overrides();
    }

    #[test]
    fn test_register_overrides_multiple() {
        let _guard = OVERRIDE_TEST_LOCK.lock().unwrap();

        ToolDescriptions::clear_overrides();

        let mut overrides = HashMap::new();
        overrides.insert("tool_a".to_string(), "Description A".to_string());
        overrides.insert("tool_b".to_string(), "Description B".to_string());

        ToolDescriptions::register_overrides(overrides);

        assert_eq!(ToolDescriptions::get("tool_a", "fallback"), "Description A");
        assert_eq!(ToolDescriptions::get("tool_b", "fallback"), "Description B");

        ToolDescriptions::clear_overrides();
    }

    #[test]
    fn test_override_takes_precedence() {
        let _guard = OVERRIDE_TEST_LOCK.lock().unwrap();

        ToolDescriptions::clear_overrides();

        // Register an override for a tool that might have a loaded description
        ToolDescriptions::register_override("read", "Override read description");

        // Override should take precedence
        let desc = ToolDescriptions::get("read", "fallback read");
        assert_eq!(desc, "Override read description");

        ToolDescriptions::clear_overrides();
    }

    #[test]
    fn test_clear_overrides() {
        let _guard = OVERRIDE_TEST_LOCK.lock().unwrap();

        ToolDescriptions::clear_overrides();

        ToolDescriptions::register_override("temp_tool", "Temporary description");
        assert!(ToolDescriptions::get_override("temp_tool").is_some());

        ToolDescriptions::clear_overrides();
        assert!(ToolDescriptions::get_override("temp_tool").is_none());
    }

    #[test]
    fn test_list_overrides() {
        let _guard = OVERRIDE_TEST_LOCK.lock().unwrap();

        ToolDescriptions::clear_overrides();

        ToolDescriptions::register_override("list_tool_1", "Desc 1");
        ToolDescriptions::register_override("list_tool_2", "Desc 2");

        let list = ToolDescriptions::list_overrides();
        assert!(list.contains(&"list_tool_1".to_string()));
        assert!(list.contains(&"list_tool_2".to_string()));

        ToolDescriptions::clear_overrides();
    }

    #[test]
    fn test_fallback_when_no_override() {
        let _guard = OVERRIDE_TEST_LOCK.lock().unwrap();

        ToolDescriptions::clear_overrides();

        // A tool with no override and no loaded description should return fallback
        let desc = ToolDescriptions::get("nonexistent_tool_xyz", "my fallback");
        assert_eq!(desc, "my fallback");
    }
}
