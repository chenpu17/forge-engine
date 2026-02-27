//! 工作流错误类型

use thiserror::Error;

/// Graph 操作错误
#[derive(Debug, Clone, Error)]
pub enum GraphError {
    /// 节点未找到
    #[error("Node not found: {0}")]
    NodeNotFound(String),

    /// 节点已存在
    #[error("Node already exists: {0}")]
    NodeAlreadyExists(String),

    /// 边未找到
    #[error("Edge not found: {0}")]
    EdgeNotFound(String),

    /// 无入口点
    #[error("No entry point defined")]
    NoEntryPoint,

    /// 无效入口点
    #[error("Invalid entry point: {0} does not exist")]
    InvalidEntryPoint(String),

    /// 悬空边
    #[error("Dangling edge {edge_id}: references missing node {missing_node}")]
    DanglingEdge {
        /// 边 ID
        edge_id: String,
        /// 缺失的节点
        missing_node: String,
    },

    /// 孤立节点
    #[error("Orphan node: {0} has no connections")]
    OrphanNode(String),

    /// 不可达节点
    #[error("Unreachable node: {0} cannot be reached from entry point")]
    UnreachableNode(String),

    /// 检测到循环
    #[error("Cycle detected: {0:?}")]
    CycleDetected(Vec<String>),

    /// 验证失败（多个错误）
    #[error("Validation failed with {} error(s)", .0.len())]
    ValidationFailed(Vec<Self>),
}
