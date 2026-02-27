//! 工作流运行时状态

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// WorkflowStatus
// ============================================================================

/// 工作流状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[derive(Default)]
pub enum WorkflowStatus {
    /// 待执行
    #[default]
    Pending,
    /// 运行中
    Running,
    /// 等待人工输入
    WaitingForHuman {
        /// 等待的节点 ID
        node: String,
        /// 提示信息
        prompt: String,
    },
    /// 已完成
    Completed,
    /// 失败
    Failed {
        /// 错误信息
        error: String,
        /// 失败的节点 ID
        node: String,
    },
    /// 已取消
    Cancelled,
}


// ============================================================================
// ExecutionStatus
// ============================================================================

/// 节点执行状态
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    /// 待执行
    Pending,
    /// 运行中
    Running,
    /// 已完成
    Completed,
    /// 失败
    Failed,
    /// 已跳过
    Skipped,
}

// ============================================================================
// NodeExecution
// ============================================================================

/// 节点执行记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeExecution {
    /// 节点 ID
    pub node_id: String,
    /// 开始时间
    pub started_at: DateTime<Utc>,
    /// 完成时间
    pub completed_at: Option<DateTime<Utc>>,
    /// 执行状态
    pub status: ExecutionStatus,
    /// 输出数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
    /// 错误信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl NodeExecution {
    /// 创建新的执行记录
    pub fn new(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            started_at: Utc::now(),
            completed_at: None,
            status: ExecutionStatus::Running,
            output: None,
            error: None,
        }
    }

    /// 标记为完成
    pub fn complete(&mut self, output: serde_json::Value) {
        self.completed_at = Some(Utc::now());
        self.status = ExecutionStatus::Completed;
        self.output = Some(output);
    }

    /// 标记为失败
    pub fn fail(&mut self, error: impl Into<String>) {
        self.completed_at = Some(Utc::now());
        self.status = ExecutionStatus::Failed;
        self.error = Some(error.into());
    }

    /// 获取执行时长（毫秒）
    #[must_use] 
    pub fn duration_ms(&self) -> Option<u64> {
        self.completed_at
            .map(|end| u64::try_from((end - self.started_at).num_milliseconds().max(0)).unwrap_or(0))
    }
}

// ============================================================================
// WorkflowState
// ============================================================================

/// 工作流运行时状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowState {
    /// 工作流状态
    pub status: WorkflowStatus,
    /// 当前节点 ID
    pub current_node: String,
    /// 状态数据
    pub data: HashMap<String, serde_json::Value>,
    /// 执行历史
    pub history: Vec<NodeExecution>,
    /// 创建时间
    pub created_at: DateTime<Utc>,
    /// 更新时间
    pub updated_at: DateTime<Utc>,
}

impl Default for WorkflowState {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkflowState {
    /// 创建新的状态
    #[must_use] 
    pub fn new() -> Self {
        let now = Utc::now();
        Self {
            status: WorkflowStatus::Pending,
            current_node: String::new(),
            data: HashMap::new(),
            history: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// 设置状态值
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) {
        self.data.insert(key.into(), value.into());
        self.updated_at = Utc::now();
    }

    /// 获取状态值
    #[must_use] 
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.data.get(key)
    }

    /// 获取状态值（可变引用）
    pub fn get_mut(&mut self, key: &str) -> Option<&mut serde_json::Value> {
        self.data.get_mut(key)
    }

    /// 删除状态值
    pub fn remove(&mut self, key: &str) -> Option<serde_json::Value> {
        let value = self.data.remove(key);
        if value.is_some() {
            self.updated_at = Utc::now();
        }
        value
    }

    /// 检查是否存在状态值
    #[must_use] 
    pub fn contains(&self, key: &str) -> bool {
        self.data.contains_key(key)
    }

    /// 获取所有状态键
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.data.keys()
    }

    /// 清空状态数据
    pub fn clear_data(&mut self) {
        self.data.clear();
        self.updated_at = Utc::now();
    }

    /// 添加执行记录
    pub fn push_execution(&mut self, execution: NodeExecution) {
        self.history.push(execution);
        self.updated_at = Utc::now();
    }

    /// 获取最后一次执行记录
    #[must_use] 
    pub fn last_execution(&self) -> Option<&NodeExecution> {
        self.history.last()
    }

    /// 获取最后一次执行记录（可变引用）
    pub fn last_execution_mut(&mut self) -> Option<&mut NodeExecution> {
        self.history.last_mut()
    }

    /// 获取指定节点的执行记录
    #[must_use] 
    pub fn get_node_executions(&self, node_id: &str) -> Vec<&NodeExecution> {
        self.history.iter().filter(|e| e.node_id == node_id).collect()
    }

    /// 获取执行的节点数量
    #[must_use] 
    pub fn executed_count(&self) -> usize {
        self.history.len()
    }

    /// 检查工作流是否已完成
    #[must_use] 
    pub const fn is_completed(&self) -> bool {
        matches!(self.status, WorkflowStatus::Completed)
    }

    /// 检查工作流是否失败
    #[must_use] 
    pub const fn is_failed(&self) -> bool {
        matches!(self.status, WorkflowStatus::Failed { .. })
    }

    /// 检查工作流是否正在运行
    #[must_use] 
    pub const fn is_running(&self) -> bool {
        matches!(self.status, WorkflowStatus::Running)
    }

    /// 检查工作流是否等待人工输入
    #[must_use] 
    pub const fn is_waiting_for_human(&self) -> bool {
        matches!(self.status, WorkflowStatus::WaitingForHuman { .. })
    }

    /// 检查工作流是否已取消
    #[must_use] 
    pub const fn is_cancelled(&self) -> bool {
        matches!(self.status, WorkflowStatus::Cancelled)
    }

    /// 检查工作流是否已结束（完成、失败或取消）
    #[must_use] 
    pub const fn is_finished(&self) -> bool {
        matches!(
            self.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed { .. } | WorkflowStatus::Cancelled
        )
    }
}
