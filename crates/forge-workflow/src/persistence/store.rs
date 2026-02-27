//! 工作流存储 trait
//!
//! 定义工作流持久化的抽象接口。

use async_trait::async_trait;

use crate::definition::GraphDefinition;

/// 存储错误
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// IO 错误
    #[error("IO error: {0}")]
    IoError(String),

    /// 序列化错误
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// 工作流未找到
    #[error("Workflow not found: {0}")]
    NotFound(String),

    /// 工作流已存在
    #[error("Workflow already exists: {0}")]
    AlreadyExists(String),
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::IoError(e.to_string())
    }
}

/// 工作流存储 trait
#[async_trait]
pub trait WorkflowStore: Send + Sync {
    /// 保存工作流
    async fn save(&self, workflow: &GraphDefinition) -> Result<(), StoreError>;

    /// 加载工作流
    async fn load(&self, id: &str) -> Result<Option<GraphDefinition>, StoreError>;

    /// 列出所有工作流
    async fn list(&self) -> Result<Vec<WorkflowInfo>, StoreError>;

    /// 删除工作流
    async fn delete(&self, id: &str) -> Result<(), StoreError>;

    /// 检查工作流是否存在
    async fn exists(&self, id: &str) -> Result<bool, StoreError>;
}

/// 工作流信息（列表用）
#[derive(Debug, Clone)]
pub struct WorkflowInfo {
    /// 工作流 ID
    pub id: String,
    /// 工作流名称
    pub name: String,
    /// 描述
    pub description: Option<String>,
    /// 版本
    pub version: Option<String>,
}
