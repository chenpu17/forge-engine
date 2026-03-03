//! Built-in tool implementations.

pub mod ask_user;
pub mod batch;
pub mod edit;
pub mod enter_plan_mode;
pub mod exit_plan_mode;
pub mod git;
pub mod glob;
pub mod grep;
pub mod kill_shell;
pub mod memory_manage;
pub mod memory_read;
pub mod memory_write;
pub mod read;
pub mod shell;
pub mod skill;
pub mod task;
pub mod task_output;
pub mod todo;
pub mod web_fetch;
pub mod web_search;
pub mod write;

// Re-export commonly used types
pub use ask_user::{AskUserQuestionTool, Question, QuestionAnswer, QuestionOption};
pub use batch::BatchTool;
pub use enter_plan_mode::EnterPlanModeTool;
pub use exit_plan_mode::ExitPlanModeTool;
pub use git::{GitOperation, GitTool};
pub use kill_shell::KillShellTool;
pub use memory_manage::MemoryManageTool;
pub use memory_read::MemoryReadTool;
pub use memory_write::MemoryWriteTool;
pub use shell::{get_shell_tool, get_shell_tools, ShellAlias};
pub use skill::SkillTool;
pub use task::{
    MockTaskExecutor, ModelTier, SubAgentType, TaskExecutionError, TaskExecutionReport,
    TaskExecutor, TaskInstance, TaskState, TaskStatus, TaskTool,
};
pub use task_output::TaskOutputTool;
pub use todo::{TodoItem, TodoState, TodoStatus, TodoWriteTool};
pub use web_fetch::{FetchCache, WebFetchTool};
pub use web_search::{
    BraveSearchProvider, DuckDuckGoProvider, ExaSearchProvider, MockSearchProvider, SearchProvider,
    SearchProviderType, SearchResult, WebSearchTool,
};

// Legacy bash module - re-export from shell for backwards compatibility
/// Bash tool re-export for backwards compatibility
#[cfg(unix)]
pub mod bash {
    pub use super::shell::unix::BashTool;
}
