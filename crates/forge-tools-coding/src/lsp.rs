//! LSP code intelligence tools
//!
//! Provides diagnostics, go-to-definition, and find-references via LSP.
//!
//! # Context Requirements
//!
//! LSP tools require the concrete [`forge_tools::ToolContext`] to have its
//! `lsp_manager` field set. If the context does not provide an LSP manager,
//! the tools return a user-friendly error.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::OnceLock;

use async_trait::async_trait;
use forge_domain::{ToolError, ToolExecutionContext, ToolOutput};
use forge_tools::description::ToolDescriptions;
use forge_tools::params::{optional_bool, optional_i64, required_str};
use forge_tools::path_utils::normalize_path;
use serde_json::{json, Value};

// ─── Diagnostics Tool ───────────────────────────────────────────────────────

const DIAGNOSTICS_FALLBACK: &str = "\
Get diagnostics (errors, warnings) for a file from the language server.\n\
\n\
Returns compiler errors, warnings, and hints for the specified file.\n\
\n\
Usage:\n\
- Get diagnostics: {\"file_path\": \"src/main.rs\"}\n\
\n\
Notes:\n\
- Requires a language server (rust-analyzer, typescript-language-server, pyright, gopls)\n\
- Server is started lazily on first use\n\
- Returns empty list if no issues found";

/// LSP diagnostics tool
pub struct LspDiagnosticsTool;

#[async_trait]
impl forge_domain::Tool for LspDiagnosticsTool {
    fn name(&self) -> &'static str {
        "lsp_diagnostics"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("lsp_diagnostics", DIAGNOSTICS_FALLBACK))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file to get diagnostics for"
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_readonly(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let file_path = required_str(&params, "file_path")?;
        let abs_path = resolve_path(file_path, ctx.working_dir());
        validate_path_boundary(&abs_path, ctx.working_dir())?;

        let manager = get_lsp_manager(ctx)?;
        let client = manager
            .client_for_file(&abs_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        client.open_file(&abs_path).await.map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let uri = path_to_uri(&abs_path);
        let result = client
            .request(
                "textDocument/diagnostic",
                Some(json!({
                    "textDocument": { "uri": uri }
                })),
            )
            .await;

        match result {
            Ok(diags) => {
                let formatted = format_diagnostics(&diags, file_path);
                if formatted.is_empty() {
                    Ok(ToolOutput::success("No diagnostics found."))
                } else {
                    Ok(ToolOutput::success(formatted))
                }
            }
            Err(forge_lsp::LspError::ServerError { code: -32601, .. }) => Ok(ToolOutput::success(
                "Diagnostics not available (server does not support pull diagnostics). \
                     Try saving the file and checking compiler output instead.",
            )),
            Err(e) => Err(ToolError::ExecutionFailed(e.to_string())),
        }
    }
}

// ─── Definition Tool ────────────────────────────────────────────────────────

const DEFINITION_FALLBACK: &str = "\
Go to the definition of a symbol at a given position.\n\
\n\
Returns the file path and line number where the symbol is defined.\n\
\n\
Usage:\n\
- Find definition: {\"file_path\": \"src/main.rs\", \"line\": 10, \"character\": 5}\n\
\n\
Notes:\n\
- Line and character are 0-indexed\n\
- Requires a language server for the file type";

/// LSP go-to-definition tool
pub struct LspDefinitionTool;

#[async_trait]
impl forge_domain::Tool for LspDefinitionTool {
    fn name(&self) -> &'static str {
        "lsp_definition"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("lsp_definition", DEFINITION_FALLBACK))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file containing the symbol"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number (0-indexed)"
                },
                "character": {
                    "type": "integer",
                    "description": "Character offset in the line (0-indexed)"
                }
            },
            "required": ["file_path", "line", "character"]
        })
    }

    fn is_readonly(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let file_path = required_str(&params, "file_path")?;
        let line = optional_i64(&params, "line", 0);
        let character = optional_i64(&params, "character", 0);
        let abs_path = resolve_path(file_path, ctx.working_dir());
        validate_path_boundary(&abs_path, ctx.working_dir())?;

        let manager = get_lsp_manager(ctx)?;
        let client = manager
            .client_for_file(&abs_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        client.open_file(&abs_path).await.map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let uri = path_to_uri(&abs_path);
        let result = client
            .request(
                "textDocument/definition",
                Some(json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character }
                })),
            )
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let formatted = format_locations(&result, ctx.working_dir());
        if formatted.is_empty() {
            Ok(ToolOutput::success("No definition found."))
        } else {
            Ok(ToolOutput::success(formatted))
        }
    }
}

// ─── References Tool ────────────────────────────────────────────────────────

const REFERENCES_FALLBACK: &str = "\
Find all references to a symbol at a given position.\n\
\n\
Returns file paths and line numbers where the symbol is referenced.\n\
\n\
Usage:\n\
- Find references: {\"file_path\": \"src/lib.rs\", \"line\": 15, \"character\": 10}\n\
- Include declaration: {\"file_path\": \"src/lib.rs\", \"line\": 15, \"character\": 10, \"include_declaration\": true}\n\
\n\
Notes:\n\
- Line and character are 0-indexed\n\
- Requires a language server for the file type";

/// LSP find-references tool
pub struct LspReferencesTool;

#[async_trait]
impl forge_domain::Tool for LspReferencesTool {
    fn name(&self) -> &'static str {
        "lsp_references"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("lsp_references", REFERENCES_FALLBACK))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Path to the file containing the symbol"
                },
                "line": {
                    "type": "integer",
                    "description": "Line number (0-indexed)"
                },
                "character": {
                    "type": "integer",
                    "description": "Character offset in the line (0-indexed)"
                },
                "include_declaration": {
                    "type": "boolean",
                    "description": "Include the declaration in results (default: true)"
                }
            },
            "required": ["file_path", "line", "character"]
        })
    }

    fn is_readonly(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        let file_path = required_str(&params, "file_path")?;
        let line = optional_i64(&params, "line", 0);
        let character = optional_i64(&params, "character", 0);
        let include_decl = optional_bool(&params, "include_declaration", true);
        let abs_path = resolve_path(file_path, ctx.working_dir());
        validate_path_boundary(&abs_path, ctx.working_dir())?;

        let manager = get_lsp_manager(ctx)?;
        let client = manager
            .client_for_file(&abs_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        client.open_file(&abs_path).await.map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let uri = path_to_uri(&abs_path);
        let result = client
            .request(
                "textDocument/references",
                Some(json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "context": { "includeDeclaration": include_decl }
                })),
            )
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let formatted = format_locations(&result, ctx.working_dir());
        if formatted.is_empty() {
            Ok(ToolOutput::success("No references found."))
        } else {
            Ok(ToolOutput::success(formatted))
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Extract the LSP manager from the execution context.
///
/// Downcasts the trait object to [`forge_tools::ToolContext`] and reads
/// the `lsp_manager` field. Returns an error if the context is not a
/// `ToolContext` or if no LSP manager is configured.
fn get_lsp_manager(
    ctx: &dyn ToolExecutionContext,
) -> std::result::Result<&forge_lsp::LspManager, ToolError> {
    let tool_ctx = ctx.as_any().downcast_ref::<forge_tools::ToolContext>().ok_or_else(|| {
        ToolError::ExecutionFailed("LSP not available. Context does not support LSP.".to_string())
    })?;

    tool_ctx.lsp_manager.as_deref().ok_or_else(|| {
        ToolError::ExecutionFailed(
            "LSP not available. No language server manager configured.".to_string(),
        )
    })
}

/// Resolve a file path relative to the working directory.
fn resolve_path(file_path: &str, working_dir: &Path) -> std::path::PathBuf {
    let path = Path::new(file_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir.join(path)
    }
}

/// Validate that a resolved path is within the working directory boundary.
///
/// Returns `Err` if the path escapes the project root (e.g. via `../`).
/// Uses lexical normalization for non-existent paths to prevent traversal
/// bypasses where `canonicalize()` falls back to the raw path.
fn validate_path_boundary(
    abs_path: &Path,
    working_dir: &Path,
) -> std::result::Result<(), ToolError> {
    let canonical = abs_path.canonicalize().unwrap_or_else(|_| normalize_path(abs_path));
    let canonical_wd = working_dir.canonicalize().unwrap_or_else(|_| normalize_path(working_dir));

    if !canonical.starts_with(&canonical_wd) {
        return Err(ToolError::InvalidParams(format!(
            "Path '{}' is outside the project directory",
            abs_path.display()
        )));
    }
    Ok(())
}

/// Convert a file path to a properly encoded `file://` URI.
///
/// Delegates to `forge_lsp::path_to_file_uri` for a single shared
/// implementation of percent-encoding for LSP URIs.
fn path_to_uri(path: &Path) -> String {
    forge_lsp::path_to_file_uri(path)
}

/// Format LSP diagnostic results into human-readable text
fn format_diagnostics(value: &Value, file_path: &str) -> String {
    let mut output = String::new();

    let items =
        value.get("items").or_else(|| value.get("relatedDocuments")).and_then(Value::as_array);

    if let Some(items) = items {
        for item in items {
            let severity = match item.get("severity").and_then(Value::as_i64) {
                Some(1) => "error",
                Some(2) => "warning",
                Some(3) => "info",
                Some(4) => "hint",
                _ => "diagnostic",
            };

            let message = item.get("message").and_then(Value::as_str).unwrap_or("(no message)");

            let line =
                item.pointer("/range/start/line").and_then(Value::as_i64).map_or(0, |l| l + 1);

            let _ = writeln!(output, "{file_path}:{line}: [{severity}] {message}");
        }
    }

    output
}

/// Format LSP location results (definition/references) into human-readable text
fn format_locations(value: &Value, working_dir: &Path) -> String {
    let mut output = String::new();
    let working_dir_str = working_dir.to_string_lossy();

    let locations: Vec<&Value> = if let Some(arr) = value.as_array() {
        arr.iter().collect()
    } else if value.get("uri").is_some() {
        vec![value]
    } else {
        return output;
    };

    for loc in locations {
        let Some(uri) = loc.get("uri").and_then(Value::as_str) else {
            continue;
        };

        let path = uri.strip_prefix("file://").unwrap_or(uri);
        let display_path = path
            .strip_prefix(working_dir_str.as_ref())
            .map_or(path, |p| p.strip_prefix('/').unwrap_or(p));

        let line = loc.pointer("/range/start/line").and_then(Value::as_i64).map_or(1, |l| l + 1);

        let col =
            loc.pointer("/range/start/character").and_then(Value::as_i64).map_or(1, |c| c + 1);

        let _ = writeln!(output, "{display_path}:{line}:{col}");
    }

    output
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_path_absolute() {
        let result = resolve_path("/usr/bin/test", Path::new("/home"));
        assert_eq!(result, Path::new("/usr/bin/test"));
    }

    #[test]
    fn test_resolve_path_relative() {
        let result = resolve_path("src/main.rs", Path::new("/project"));
        assert_eq!(result, Path::new("/project/src/main.rs"));
    }

    #[test]
    fn test_format_diagnostics_empty() {
        let value = json!({"items": []});
        assert!(format_diagnostics(&value, "test.rs").is_empty());
    }

    #[test]
    fn test_format_diagnostics_with_items() {
        let value = json!({
            "items": [{
                "severity": 1,
                "message": "expected `;`",
                "range": { "start": { "line": 9, "character": 0 } }
            }, {
                "severity": 2,
                "message": "unused variable",
                "range": { "start": { "line": 4, "character": 8 } }
            }]
        });
        let result = format_diagnostics(&value, "src/main.rs");
        assert!(result.contains("src/main.rs:10: [error] expected `;`"));
        assert!(result.contains("src/main.rs:5: [warning] unused variable"));
    }

    #[test]
    fn test_format_locations_single() {
        let value = json!({
            "uri": "file:///project/src/lib.rs",
            "range": { "start": { "line": 14, "character": 3 } }
        });
        let result = format_locations(&value, Path::new("/project"));
        assert_eq!(result, "src/lib.rs:15:4\n");
    }

    #[test]
    fn test_format_locations_array() {
        let value = json!([
            {
                "uri": "file:///project/src/a.rs",
                "range": { "start": { "line": 0, "character": 0 } }
            },
            {
                "uri": "file:///project/src/b.rs",
                "range": { "start": { "line": 9, "character": 4 } }
            }
        ]);
        let result = format_locations(&value, Path::new("/project"));
        assert!(result.contains("src/a.rs:1:1"));
        assert!(result.contains("src/b.rs:10:5"));
    }

    #[test]
    fn test_format_locations_empty() {
        let value = json!([]);
        assert!(format_locations(&value, Path::new("/project")).is_empty());
    }

    #[test]
    fn test_tool_names() {
        use forge_domain::Tool;

        let diag = LspDiagnosticsTool;
        assert_eq!(diag.name(), "lsp_diagnostics");
        assert!(diag.is_readonly());

        let def = LspDefinitionTool;
        assert_eq!(def.name(), "lsp_definition");
        assert!(def.is_readonly());

        let refs = LspReferencesTool;
        assert_eq!(refs.name(), "lsp_references");
        assert!(refs.is_readonly());
    }

    #[test]
    fn test_parameter_schemas() {
        use forge_domain::Tool;

        let diag = LspDiagnosticsTool;
        let schema = diag.parameters_schema();
        assert!(schema["properties"]["file_path"].is_object());

        let def = LspDefinitionTool;
        let schema = def.parameters_schema();
        assert!(schema["properties"]["line"].is_object());
        assert!(schema["properties"]["character"].is_object());

        let refs = LspReferencesTool;
        let schema = refs.parameters_schema();
        assert!(schema["properties"]["include_declaration"].is_object());
    }

    #[test]
    fn test_normalize_path_no_dots() {
        let result = normalize_path(Path::new("/project/src/main.rs"));
        assert_eq!(result, Path::new("/project/src/main.rs"));
    }

    #[test]
    fn test_normalize_path_with_parent_dir() {
        let result = normalize_path(Path::new("/project/src/../etc/passwd"));
        assert_eq!(result, Path::new("/project/etc/passwd"));
    }

    #[test]
    fn test_normalize_path_with_current_dir() {
        let result = normalize_path(Path::new("/project/./src/./main.rs"));
        assert_eq!(result, Path::new("/project/src/main.rs"));
    }

    #[test]
    fn test_normalize_path_traversal_past_root() {
        let result = normalize_path(Path::new("/project/../../etc/passwd"));
        assert_eq!(result, Path::new("/etc/passwd"));
    }

    #[test]
    fn test_validate_path_boundary_inside() {
        let dir = tempfile::tempdir().expect("tempdir");
        let src_dir = dir.path().join("src");
        std::fs::create_dir_all(&src_dir).expect("mkdir");
        let file = src_dir.join("main.rs");
        std::fs::write(&file, "fn main() {}").expect("write");
        assert!(validate_path_boundary(&file, dir.path()).is_ok());
    }

    #[test]
    fn test_validate_path_boundary_outside() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = Path::new("/etc/passwd");
        assert!(validate_path_boundary(outside, dir.path()).is_err());
    }

    #[test]
    fn test_validate_path_boundary_traversal_nonexistent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let traversal = dir.path().join("src/../../etc/passwd");
        assert!(validate_path_boundary(&traversal, dir.path()).is_err());
    }

    #[test]
    fn test_path_to_uri_simple() {
        let uri = path_to_uri(Path::new("/project/src/main.rs"));
        assert!(uri.starts_with("file://"));
        assert!(uri.contains("main.rs"));
    }

    #[test]
    fn test_path_to_uri_with_spaces() {
        let uri = path_to_uri(Path::new("/my project/src/main.rs"));
        assert!(uri.starts_with("file://"));
        assert!(uri.contains("my%20project") || uri.contains("my+project"));
        assert!(!uri.contains("my project"));
    }
}
