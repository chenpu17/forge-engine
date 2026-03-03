//! 检查点持久化
//!
//! 支持工作流状态的保存和恢复。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::state::WorkflowState;

/// 检查点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// 检查点 ID
    pub id: String,
    /// 工作流 ID
    pub workflow_id: String,
    /// 工作流状态
    pub state: WorkflowState,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 元数据
    pub metadata: CheckpointMetadata,
}

/// 检查点元数据
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CheckpointMetadata {
    /// 当前节点
    pub current_node: String,
    /// 已执行节点数
    pub nodes_executed: usize,
    /// 描述
    pub description: Option<String>,
}

impl Checkpoint {
    /// 创建新的检查点
    pub fn new(workflow_id: impl Into<String>, state: WorkflowState) -> Self {
        let workflow_id = workflow_id.into();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            workflow_id,
            metadata: CheckpointMetadata {
                current_node: state.current_node.clone(),
                nodes_executed: state.history.len(),
                description: None,
            },
            state,
            created_at: Utc::now(),
        }
    }

    /// 设置描述
    #[must_use]
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.metadata.description = Some(desc.into());
        self
    }
}

/// 检查点存储 trait
#[async_trait::async_trait]
pub trait CheckpointStore: Send + Sync {
    /// 保存检查点
    async fn save(&self, checkpoint: &Checkpoint) -> Result<(), CheckpointError>;

    /// 加载检查点
    async fn load(&self, id: &str) -> Result<Option<Checkpoint>, CheckpointError>;

    /// 列出工作流的所有检查点
    async fn list(&self, workflow_id: &str) -> Result<Vec<Checkpoint>, CheckpointError>;

    /// 删除检查点
    async fn delete(&self, id: &str) -> Result<(), CheckpointError>;
}

/// 检查点错误
#[derive(Debug, thiserror::Error)]
pub enum CheckpointError {
    /// IO 错误
    #[error("IO error: {0}")]
    IoError(String),

    /// 序列化错误
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// 检查点未找到
    #[error("Checkpoint not found: {0}")]
    NotFound(String),
}

/// 内存检查点存储（用于测试）
#[derive(Debug, Default)]
pub struct MemoryCheckpointStore {
    checkpoints: std::sync::RwLock<std::collections::HashMap<String, Checkpoint>>,
}

impl MemoryCheckpointStore {
    /// 创建新的内存存储
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl CheckpointStore for MemoryCheckpointStore {
    async fn save(&self, checkpoint: &Checkpoint) -> Result<(), CheckpointError> {
        self.checkpoints
            .write()
            .map_err(|e| CheckpointError::IoError(e.to_string()))?
            .insert(checkpoint.id.clone(), checkpoint.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        let store = self.checkpoints.read().map_err(|e| CheckpointError::IoError(e.to_string()))?;
        Ok(store.get(id).cloned())
    }

    async fn list(&self, workflow_id: &str) -> Result<Vec<Checkpoint>, CheckpointError> {
        let checkpoints: Vec<_> = self
            .checkpoints
            .read()
            .map_err(|e| CheckpointError::IoError(e.to_string()))?
            .values()
            .filter(|c| c.workflow_id == workflow_id)
            .cloned()
            .collect();
        Ok(checkpoints)
    }

    async fn delete(&self, id: &str) -> Result<(), CheckpointError> {
        self.checkpoints.write().map_err(|e| CheckpointError::IoError(e.to_string()))?.remove(id);
        Ok(())
    }
}
