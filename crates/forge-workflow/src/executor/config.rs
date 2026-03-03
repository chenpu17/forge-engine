//! 执行器配置

use serde::{Deserialize, Serialize};

/// 执行配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    /// 最大迭代次数
    pub max_iterations: usize,
    /// 单节点超时（秒）
    pub node_timeout_secs: u64,
    /// 总超时（秒）
    pub total_timeout_secs: u64,
    /// 子工作流最大递归深度
    pub max_subworkflow_depth: usize,
    /// 是否启用检查点
    pub enable_checkpoints: bool,
    /// 检查点间隔（节点数）
    pub checkpoint_interval: usize,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_iterations: 1000,
            node_timeout_secs: 300,
            total_timeout_secs: 1800,
            max_subworkflow_depth: 5,
            enable_checkpoints: true,
            checkpoint_interval: 5,
        }
    }
}

impl ExecutorConfig {
    /// 创建新的配置
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置最大迭代次数
    #[must_use]
    pub const fn max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    /// 设置节点超时
    #[must_use]
    pub const fn node_timeout(mut self, secs: u64) -> Self {
        self.node_timeout_secs = secs;
        self
    }

    /// 设置总超时
    #[must_use]
    pub const fn total_timeout(mut self, secs: u64) -> Self {
        self.total_timeout_secs = secs;
        self
    }

    /// 启用检查点
    #[must_use]
    pub const fn with_checkpoints(mut self, interval: usize) -> Self {
        self.enable_checkpoints = true;
        self.checkpoint_interval = interval;
        self
    }

    /// 禁用检查点
    #[must_use]
    pub const fn without_checkpoints(mut self) -> Self {
        self.enable_checkpoints = false;
        self
    }
}
