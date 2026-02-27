//! 文件检查点存储
//!
//! 基于文件系统的检查点持久化实现。

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::fs;

use crate::checkpoint::{Checkpoint, CheckpointError, CheckpointStore};

/// 文件检查点存储
pub struct FileCheckpointStore {
    /// 存储目录
    base_dir: PathBuf,
}

impl FileCheckpointStore {
    /// 创建新的文件存储
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self { base_dir: base_dir.into() }
    }

    /// 获取检查点文件路径
    fn checkpoint_path(&self, id: &str) -> PathBuf {
        self.base_dir.join(format!("{id}.json"))
    }

    /// 确保目录存在
    async fn ensure_dir(&self) -> Result<(), CheckpointError> {
        fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| CheckpointError::IoError(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl CheckpointStore for FileCheckpointStore {
    async fn save(&self, checkpoint: &Checkpoint) -> Result<(), CheckpointError> {
        self.ensure_dir().await?;
        let path = self.checkpoint_path(&checkpoint.id);
        let json = serde_json::to_string_pretty(checkpoint)
            .map_err(|e| CheckpointError::SerializationError(e.to_string()))?;
        fs::write(&path, json).await.map_err(|e| CheckpointError::IoError(e.to_string()))?;
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Checkpoint>, CheckpointError> {
        let path = self.checkpoint_path(id);

        if !path.exists() {
            return Ok(None);
        }

        let json =
            fs::read_to_string(&path).await.map_err(|e| CheckpointError::IoError(e.to_string()))?;

        let checkpoint: Checkpoint = serde_json::from_str(&json)
            .map_err(|e| CheckpointError::SerializationError(e.to_string()))?;

        Ok(Some(checkpoint))
    }

    async fn list(&self, workflow_id: &str) -> Result<Vec<Checkpoint>, CheckpointError> {
        self.ensure_dir().await?;

        let mut checkpoints = Vec::new();
        let mut entries = fs::read_dir(&self.base_dir)
            .await
            .map_err(|e| CheckpointError::IoError(e.to_string()))?;

        while let Some(entry) =
            entries.next_entry().await.map_err(|e| CheckpointError::IoError(e.to_string()))?
        {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(json) = fs::read_to_string(&path).await {
                    if let Ok(cp) = serde_json::from_str::<Checkpoint>(&json) {
                        if cp.workflow_id == workflow_id {
                            checkpoints.push(cp);
                        }
                    }
                }
            }
        }

        // 按创建时间排序
        checkpoints.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(checkpoints)
    }

    async fn delete(&self, id: &str) -> Result<(), CheckpointError> {
        let path = self.checkpoint_path(id);

        if path.exists() {
            fs::remove_file(&path).await.map_err(|e| CheckpointError::IoError(e.to_string()))?;
        }

        Ok(())
    }
}
