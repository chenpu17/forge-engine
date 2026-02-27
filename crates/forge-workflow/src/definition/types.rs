//! 工作流定义类型
//!
//! 提供可序列化的工作流定义结构。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::node::{NodeConfig, Position};

/// 工作流定义
///
/// 工作流的完整可序列化表示，用于持久化和 UI 交互。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphDefinition {
    /// 工作流 ID
    pub id: String,
    /// 工作流名称
    pub name: String,
    /// 元数据
    #[serde(default)]
    pub metadata: GraphMetadataDefinition,
    /// 节点列表
    #[serde(default)]
    pub nodes: Vec<NodeDefinition>,
    /// 边列表
    #[serde(default)]
    pub edges: Vec<EdgeDefinition>,
    /// 入口节点 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_point: Option<String>,
}

/// 图元数据定义
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphMetadataDefinition {
    /// 描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 版本
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// 作者
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// 标签
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// 自定义属性
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, serde_json::Value>,
}

/// 节点定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDefinition {
    /// 节点 ID
    pub id: String,
    /// 节点名称
    pub name: String,
    /// 节点配置
    pub config: NodeConfig,
    /// UI 位置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
    /// 自定义元数据
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

/// 边定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeDefinition {
    /// 边 ID
    pub id: String,
    /// 源节点 ID
    pub source: String,
    /// 目标节点 ID
    pub target: String,
    /// 边类型
    #[serde(default)]
    pub edge_type: EdgeTypeDefinition,
}

/// 边类型定义
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EdgeTypeDefinition {
    /// 直接连接
    #[default]
    Direct,
    /// 条件连接
    Conditional {
        /// 条件表达式
        condition: String,
    },
}
