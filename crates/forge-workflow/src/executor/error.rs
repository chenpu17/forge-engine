//! 执行错误类型

use thiserror::Error;

use crate::node::{HumanInputOption, HumanInputType};

/// 执行错误
#[derive(Debug, Error)]
pub enum ExecutionError {
    /// 节点未找到
    #[error("Node not found: {0}")]
    NodeNotFound(String),

    /// 工具未找到
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    /// Agent 执行错误
    #[error("Agent error: {0}")]
    AgentError(String),

    /// 工具执行错误
    #[error("Tool error: {0}")]
    ToolError(String),

    /// 模板渲染错误
    #[error("Template error: {0}")]
    TemplateError(String),

    /// 表达式求值错误
    #[error("Expression error: {0}")]
    ExpressionError(String),

    /// 节点超时
    #[error("Node timeout")]
    Timeout,

    /// 并行执行合并错误
    #[error("Join error: {0}")]
    JoinError(String),

    /// 等待人工输入
    #[error("Waiting for human input")]
    WaitingForHuman {
        /// 提示信息
        prompt: String,
        /// 输入类型
        input_type: HumanInputType,
        /// 选项列表
        options: Option<Vec<HumanInputOption>>,
    },

    /// 工作流已取消
    #[error("Workflow cancelled")]
    Cancelled,

    /// 图错误
    #[error("Graph error: {0}")]
    GraphError(#[from] crate::error::GraphError),

    /// 其他错误
    #[error("{0}")]
    Other(String),
}
