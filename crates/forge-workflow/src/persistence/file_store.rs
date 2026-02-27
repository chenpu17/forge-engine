//! 文件工作流存储
//!
//! 基于文件系统的工作流持久化实现。

use std::path::PathBuf;

use async_trait::async_trait;
use tokio::fs;

use crate::definition::GraphDefinition;
use crate::persistence::{StoreError, WorkflowInfo, WorkflowStore};

/// 文件工作流存储
pub struct FileWorkflowStore {
    /// 存储目录
    base_dir: PathBuf,
}

impl FileWorkflowStore {
    /// 创建新的文件存储
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self { base_dir: base_dir.into() }
    }

    /// 获取工作流文件路径（对 ID 做字符过滤防止路径穿越）
    fn workflow_path(&self, id: &str) -> PathBuf {
        let safe_id: String = id
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.base_dir.join(format!("{safe_id}.json"))
    }

    /// 确保目录存在
    async fn ensure_dir(&self) -> Result<(), StoreError> {
        fs::create_dir_all(&self.base_dir).await?;
        Ok(())
    }
}

#[async_trait]
impl WorkflowStore for FileWorkflowStore {
    async fn save(&self, workflow: &GraphDefinition) -> Result<(), StoreError> {
        self.ensure_dir().await?;
        let path = self.workflow_path(&workflow.id);
        let json = serde_json::to_string_pretty(workflow)
            .map_err(|e| StoreError::SerializationError(e.to_string()))?;
        fs::write(&path, json).await?;
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<GraphDefinition>, StoreError> {
        let path = self.workflow_path(id);

        if !path.exists() {
            return Ok(None);
        }

        let json = fs::read_to_string(&path).await?;
        let workflow: GraphDefinition = serde_json::from_str(&json)
            .map_err(|e| StoreError::SerializationError(e.to_string()))?;

        Ok(Some(workflow))
    }

    async fn list(&self) -> Result<Vec<WorkflowInfo>, StoreError> {
        self.ensure_dir().await?;

        let mut workflows = Vec::new();
        let mut entries = fs::read_dir(&self.base_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(json) = fs::read_to_string(&path).await {
                    if let Ok(def) = serde_json::from_str::<GraphDefinition>(&json) {
                        workflows.push(WorkflowInfo {
                            id: def.id,
                            name: def.name,
                            description: def.metadata.description,
                            version: def.metadata.version,
                        });
                    }
                }
            }
        }

        Ok(workflows)
    }

    async fn delete(&self, id: &str) -> Result<(), StoreError> {
        let path = self.workflow_path(id);

        if path.exists() {
            fs::remove_file(&path).await?;
        }

        Ok(())
    }

    async fn exists(&self, id: &str) -> Result<bool, StoreError> {
        let path = self.workflow_path(id);
        Ok(path.exists())
    }
}
