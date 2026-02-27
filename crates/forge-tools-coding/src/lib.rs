//! Forge Tools Coding — Programming-specific tools
//!
//! This crate provides coding-focused tools migrated from `forge-tools`:
//! - LSP tools (diagnostics, go-to-definition, find-references)
//! - Symbol search tool (language-aware code symbol finder)
//!
//! # Usage
//!
//! ```ignore
//! use forge_tools::ToolRegistry;
//! use forge_tools_coding::register_coding_tools;
//!
//! let mut registry = ToolRegistry::new();
//! register_coding_tools(&mut registry);
//! ```

pub mod lsp;
pub mod symbols;

use std::sync::Arc;

use forge_tools::ToolRegistry;

/// Register all coding tools into the given registry.
///
/// This adds the following tools:
/// - `lsp_diagnostics` — get compiler errors/warnings via LSP
/// - `lsp_definition` — go-to-definition via LSP
/// - `lsp_references` — find all references via LSP
/// - `symbols` — search for code symbol definitions
pub fn register_coding_tools(registry: &mut ToolRegistry) {
    registry.register(Arc::new(lsp::LspDiagnosticsTool));
    registry.register(Arc::new(lsp::LspDefinitionTool));
    registry.register(Arc::new(lsp::LspReferencesTool));
    registry.register(Arc::new(symbols::SymbolsTool));
}
