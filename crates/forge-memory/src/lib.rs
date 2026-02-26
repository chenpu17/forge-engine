#![allow(
    clippy::manual_let_else,
    clippy::option_if_let_else,
    clippy::missing_errors_doc,
    clippy::cast_possible_truncation
)]
//! Memory System
//!
//! Structured memory storage for Forge, replacing the flat `memory.md` model
//! with a directory-based system (`memory/`) using `index.md` + topic files.
//!
//! # Architecture
//!
//! - [`MemoryLoader`] — read operations (load index, read files, list, resolve references)
//! - [`MemoryWriter`] — write operations (write/delete files, auto-maintain index)
//! - `IndexManager` — internal `index.md` maintenance (add/remove/update references)
//!
//! # Directory Structure
//!
//! ```text
//! ~/.forge/memory/           # User scope
//!   index.md                 # Memory index with @path references
//!   preferences.md           # Topic file
//!   projects/
//!     forge.md               # Project-specific memory
//! {working_dir}/.forge/memory/  # Project scope
//!   index.md
//!   ...
//! ```

mod error;
mod index_manager;
mod loader;
mod migration;
mod types;
mod writer;

pub use error::MemoryError;
pub use loader::MemoryLoader;
pub use migration::{MemoryMigration, MigrationResult};
pub use types::{
    IndexEntry, MemoryFile, MemoryIndex, MemoryMeta, MemoryScope, MoveResult, WriteMode,
};
pub use writer::MemoryWriter;

#[cfg(test)]
mod tests;
