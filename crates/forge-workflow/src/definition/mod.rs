//! 工作流定义模块
//!
//! 提供工作流的可序列化表示，用于持久化和 UI 交互。

mod changes;
mod convert;
mod types;

pub use changes::*;
pub use types::*;
