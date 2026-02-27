//! 工作流执行事件

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::node::{HumanInputOption, HumanInputType};

/// 工作流执行事件
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowEvent {
    /// 工作流开始
    Started {
        /// 工作流 ID
        workflow_id: String,
        /// 工作流名称
        workflow_name: String,
    },

    /// 节点开始执行
    NodeStarted {
        /// 节点 ID
        node_id: String,
        /// 节点名称
        node_name: String,
        /// 节点类型
        node_type: String,
    },

    /// 节点执行中（进度更新）
    NodeProgress {
        /// 节点 ID
        node_id: String,
        /// 进度消息
        message: String,
    },

    /// 节点完成
    NodeCompleted {
        /// 节点 ID
        node_id: String,
        /// 输出数据
        output: serde_json::Value,
        /// 执行时长（毫秒）
        duration_ms: u64,
    },

    /// 节点失败
    NodeFailed {
        /// 节点 ID
        node_id: String,
        /// 错误信息
        error: String,
    },

    /// 路由决策
    RouteDecision {
        /// 节点 ID
        node_id: String,
        /// 下一个节点
        next_node: String,
        /// 匹配的条件
        condition: Option<String>,
        /// 决策解释
        explanation: Option<RouteExplanation>,
    },

    /// 等待人工输入
    WaitingForHuman {
        /// 节点 ID
        node_id: String,
        /// 提示信息
        prompt: String,
        /// 输入类型
        input_type: HumanInputType,
        /// 选项列表
        options: Option<Vec<HumanInputOption>>,
    },

    /// 人工输入已接收
    HumanInputReceived {
        /// 节点 ID
        node_id: String,
        /// 输入值
        value: serde_json::Value,
    },

    /// 状态更新
    StateUpdated {
        /// 状态键
        key: String,
        /// 状态值
        value: serde_json::Value,
    },

    /// 检查点已保存
    CheckpointSaved {
        /// 检查点 ID
        checkpoint_id: String,
    },

    /// 工作流完成
    Completed {
        /// 结果数据
        result: serde_json::Value,
        /// 总执行时长（毫秒）
        total_duration_ms: u64,
        /// 执行的节点数
        nodes_executed: usize,
    },

    /// 工作流失败
    Failed {
        /// 错误信息
        error: String,
        /// 失败的节点
        failed_node: Option<String>,
    },

    /// 工作流取消
    Cancelled,
}

/// 路由决策解释
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteExplanation {
    /// 评估的表达式
    pub expression: String,
    /// 评估时的相关状态值
    pub evaluated_values: HashMap<String, serde_json::Value>,
    /// 所有候选分支
    pub candidates: Vec<RouteCandidate>,
    /// 选中的分支索引
    pub selected_index: usize,
}

/// 路由候选分支
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteCandidate {
    /// 目标节点 ID
    pub target_node: String,
    /// 分支条件表达式
    pub condition: Option<String>,
    /// 条件评估结果
    pub evaluation_result: bool,
    /// 是否为默认分支
    pub is_default: bool,
}
