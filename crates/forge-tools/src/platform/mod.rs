//! Platform abstraction layer
//!
//! Provides cross-platform utilities for:
//! - Path handling (sensitive paths, temp directories)
//! - Process management (kill tree, signal handling)

mod paths;
mod process;

pub use paths::PlatformPaths;
pub use process::{KillTreeResult, ProcessManager};
