//! 图变更（增量更新）
//!
//! 支持对工作流图的增量修改，用于 UI 编辑器的实时更新。

use serde::{Deserialize, Serialize};

use crate::definition::{EdgeDefinition, NodeDefinition};
use crate::error::GraphError;
use crate::graph::Graph;
use crate::node::{Node, Position};

/// 图变更操作
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GraphChange {
    /// 添加节点
    AddNode {
        /// 节点定义
        node: NodeDefinition,
    },
    /// 移除节点
    RemoveNode {
        /// 节点 ID
        node_id: String,
    },
    /// 更新节点
    UpdateNode {
        /// 节点 ID
        node_id: String,
        /// 更新内容
        update: NodeUpdate,
    },
    /// 添加边
    AddEdge {
        /// 边定义
        edge: EdgeDefinition,
    },
    /// 移除边
    RemoveEdge {
        /// 源节点 ID
        source: String,
        /// 目标节点 ID
        target: String,
    },
    /// 设置入口点
    SetEntryPoint {
        /// 入口节点 ID
        node_id: String,
    },
    /// 更新元数据
    UpdateMetadata {
        /// 更新内容
        update: MetadataUpdate,
    },
}

/// 节点更新
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeUpdate {
    /// 新名称
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 新位置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
}

/// 元数据更新
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataUpdate {
    /// 新描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 新版本
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// 新作者
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

/// 图变更集合
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphChanges {
    /// 变更列表
    pub changes: Vec<GraphChange>,
}

impl GraphChanges {
    /// 创建空的变更集合
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加变更
    pub fn push(&mut self, change: GraphChange) {
        self.changes.push(change);
    }

    /// 是否为空
    #[must_use] 
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// 变更数量
    #[must_use] 
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// 应用变更到图
    ///
    /// # Errors
    ///
    /// 当变更操作失败（如节点不存在）时返回 `GraphError`。
    pub fn apply(&self, graph: &mut Graph) -> Result<(), GraphError> {
        for change in &self.changes {
            Self::apply_change(graph, change)?;
        }
        Ok(())
    }

    /// 应用单个变更
    fn apply_change(graph: &mut Graph, change: &GraphChange) -> Result<(), GraphError> {
        match change {
            GraphChange::AddNode { node } => {
                let n = Node {
                    name: node.name.clone(),
                    config: node.config.clone(),
                    position: node.position.clone(),
                    metadata: node.metadata.clone(),
                };
                graph.add_node(&node.id, n);
            }
            GraphChange::RemoveNode { node_id } => {
                graph.remove_node(node_id)?;
            }
            GraphChange::UpdateNode { node_id, update } => {
                if let Some(node) = graph.get_node_mut(node_id) {
                    if let Some(name) = &update.name {
                        name.clone_into(&mut node.name);
                    }
                    if let Some(pos) = &update.position {
                        node.position = Some(pos.clone());
                    }
                }
            }
            GraphChange::AddEdge { edge } => match &edge.edge_type {
                crate::definition::EdgeTypeDefinition::Direct => {
                    graph.add_edge(&edge.source, &edge.target);
                }
                crate::definition::EdgeTypeDefinition::Conditional { condition } => {
                    graph.add_conditional_edge(&edge.source, &edge.target, condition);
                }
            },
            GraphChange::RemoveEdge { source, target } => {
                graph.remove_edges_between(source, target);
            }
            GraphChange::SetEntryPoint { node_id } => {
                graph.set_entry(node_id);
            }
            GraphChange::UpdateMetadata { update } => {
                if let Some(desc) = &update.description {
                    graph.metadata_mut().description = Some(desc.clone());
                }
                if let Some(ver) = &update.version {
                    graph.metadata_mut().version = Some(ver.clone());
                }
                if let Some(author) = &update.author {
                    graph.metadata_mut().author = Some(author.clone());
                }
            }
        }
        Ok(())
    }
}
