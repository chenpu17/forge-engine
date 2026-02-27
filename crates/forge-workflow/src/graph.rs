//! 工作流图结构

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::GraphError;
use crate::node::{Node, Position};

// ============================================================================
// Edge 类型
// ============================================================================

/// 边类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EdgeType {
    /// 直接连接（无条件）
    Direct,
    /// 条件连接
    Conditional {
        /// 条件表达式
        condition: String,
    },
}

/// 边
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    /// 唯一标识符
    pub id: String,
    /// 源节点 ID
    pub source: String,
    /// 目标节点 ID
    pub target: String,
    /// 边类型
    pub edge_type: EdgeType,
    /// 标签（用于 UI 显示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

// ============================================================================
// GraphMetadata
// ============================================================================

/// 图元数据
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphMetadata {
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
    /// 创建时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    /// 更新时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    /// 自定义属性
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, serde_json::Value>,
}

// ============================================================================
// Graph 结构
// ============================================================================

/// 工作流图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Graph {
    /// 唯一标识符
    id: String,
    /// 图名称
    name: String,
    /// 节点集合
    nodes: HashMap<String, Node>,
    /// 边集合
    edges: Vec<Edge>,
    /// 入口节点 ID
    entry_point: Option<String>,
    /// 元数据
    #[serde(default)]
    metadata: GraphMetadata,
}

// ============================================================================
// Graph 构造函数
// ============================================================================

impl Graph {
    /// 创建新的工作流图
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            nodes: HashMap::new(),
            edges: Vec::new(),
            entry_point: None,
            metadata: GraphMetadata::default(),
        }
    }

    /// 使用指定 ID 创建图
    pub fn with_id(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            nodes: HashMap::new(),
            edges: Vec::new(),
            entry_point: None,
            metadata: GraphMetadata::default(),
        }
    }

    /// 获取图 ID
    #[must_use] 
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 获取图名称
    #[must_use] 
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 获取元数据
    #[must_use] 
    pub const fn metadata(&self) -> &GraphMetadata {
        &self.metadata
    }

    /// 获取元数据（可变引用）
    pub fn metadata_mut(&mut self) -> &mut GraphMetadata {
        &mut self.metadata
    }
}

// ============================================================================
// 元数据设置（Builder 风格）
// ============================================================================

impl Graph {
    /// 设置描述
    #[must_use]
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.metadata.description = Some(desc.into());
        self
    }

    /// 设置版本
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.metadata.version = Some(version.into());
        self
    }

    /// 设置作者
    #[must_use]
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.metadata.author = Some(author.into());
        self
    }

    /// 添加标签
    #[must_use]
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.metadata.tags.push(tag.into());
        self
    }

    /// 设置自定义属性
    #[must_use]
    pub fn custom(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.metadata.custom.insert(key.into(), value.into());
        self
    }
}

// ============================================================================
// 节点操作
// ============================================================================

impl Graph {
    /// 添加节点
    ///
    /// 如果是第一个添加的节点，自动设为入口点
    pub fn add_node(&mut self, id: impl Into<String>, node: impl Into<Node>) -> &mut Self {
        let id = id.into();

        // 第一个节点自动成为入口点
        if self.entry_point.is_none() {
            self.entry_point = Some(id.clone());
        }

        self.nodes.insert(id, node.into());
        self
    }

    /// 批量添加节点
    pub fn add_nodes<I, S, N>(&mut self, nodes: I) -> &mut Self
    where
        I: IntoIterator<Item = (S, N)>,
        S: Into<String>,
        N: Into<Node>,
    {
        for (id, node) in nodes {
            self.add_node(id, node);
        }
        self
    }

    /// 获取节点（不可变引用）
    #[must_use] 
    pub fn get_node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// 获取节点（可变引用）
    pub fn get_node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    /// 检查节点是否存在
    #[must_use] 
    pub fn has_node(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    /// 获取所有节点 ID
    pub fn node_ids(&self) -> impl Iterator<Item = &String> {
        self.nodes.keys()
    }

    /// 获取所有节点
    pub fn nodes(&self) -> impl Iterator<Item = (&String, &Node)> {
        self.nodes.iter()
    }

    /// 获取节点数量
    #[must_use] 
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 更新节点
    ///
    /// # Errors
    ///
    /// 当节点不存在时返回 `GraphError::NodeNotFound`。
    pub fn update_node(&mut self, id: &str, node: Node) -> Result<(), GraphError> {
        if !self.nodes.contains_key(id) {
            return Err(GraphError::NodeNotFound(id.to_string()));
        }
        self.nodes.insert(id.to_string(), node);
        Ok(())
    }

    /// 更新节点配置（使用闭包）
    ///
    /// # Errors
    ///
    /// 当节点不存在时返回 `GraphError::NodeNotFound`。
    pub fn update_node_with<F>(&mut self, id: &str, f: F) -> Result<(), GraphError>
    where
        F: FnOnce(&mut Node),
    {
        let node =
            self.nodes.get_mut(id).ok_or_else(|| GraphError::NodeNotFound(id.to_string()))?;
        f(node);
        Ok(())
    }

    /// 重命名节点
    ///
    /// # Errors
    ///
    /// 当旧节点不存在或新节点已存在时返回错误。
    pub fn rename_node(
        &mut self,
        old_id: &str,
        new_id: impl Into<String>,
    ) -> Result<(), GraphError> {
        let new_id = new_id.into();

        if !self.nodes.contains_key(old_id) {
            return Err(GraphError::NodeNotFound(old_id.to_string()));
        }

        if self.nodes.contains_key(&new_id) {
            return Err(GraphError::NodeAlreadyExists(new_id));
        }

        // 移动节点
        if let Some(node) = self.nodes.remove(old_id) {
            self.nodes.insert(new_id.clone(), node);
        }

        // 更新边
        for edge in &mut self.edges {
            if edge.source == old_id {
                edge.source.clone_from(&new_id);
            }
            if edge.target == old_id {
                edge.target.clone_from(&new_id);
            }
        }

        // 更新入口点
        if self.entry_point.as_deref() == Some(old_id) {
            self.entry_point = Some(new_id);
        }

        Ok(())
    }

    /// 删除节点（同时删除关联的边）
    ///
    /// # Errors
    ///
    /// 当节点不存在时返回 `GraphError::NodeNotFound`。
    pub fn remove_node(&mut self, id: &str) -> Result<Node, GraphError> {
        // 删除关联的边
        self.edges.retain(|e| e.source != id && e.target != id);

        // 如果删除的是入口点，清空入口点
        if self.entry_point.as_deref() == Some(id) {
            self.entry_point = None;
        }

        self.nodes.remove(id).ok_or_else(|| GraphError::NodeNotFound(id.to_string()))
    }

    /// 清空所有节点和边
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.edges.clear();
        self.entry_point = None;
    }
}

// ============================================================================
// 边操作
// ============================================================================

impl Graph {
    /// 添加直接边
    pub fn add_edge(&mut self, source: impl Into<String>, target: impl Into<String>) -> &mut Self {
        let edge = Edge {
            id: uuid::Uuid::new_v4().to_string(),
            source: source.into(),
            target: target.into(),
            edge_type: EdgeType::Direct,
            label: None,
        };
        self.edges.push(edge);
        self
    }

    /// 添加带标签的直接边
    pub fn add_edge_labeled(
        &mut self,
        source: impl Into<String>,
        target: impl Into<String>,
        label: impl Into<String>,
    ) -> &mut Self {
        let edge = Edge {
            id: uuid::Uuid::new_v4().to_string(),
            source: source.into(),
            target: target.into(),
            edge_type: EdgeType::Direct,
            label: Some(label.into()),
        };
        self.edges.push(edge);
        self
    }

    /// 添加条件边
    pub fn add_conditional_edge(
        &mut self,
        source: impl Into<String>,
        target: impl Into<String>,
        condition: impl Into<String>,
    ) -> &mut Self {
        let edge = Edge {
            id: uuid::Uuid::new_v4().to_string(),
            source: source.into(),
            target: target.into(),
            edge_type: EdgeType::Conditional { condition: condition.into() },
            label: None,
        };
        self.edges.push(edge);
        self
    }

    /// 添加路由边组
    pub fn add_router_edges<I, C, T>(&mut self, source: impl Into<String>, routes: I) -> &mut Self
    where
        I: IntoIterator<Item = (C, T)>,
        C: Into<String>,
        T: Into<String>,
    {
        let source = source.into();
        for (condition, target) in routes {
            self.add_conditional_edge(&source, target, condition);
        }
        self
    }

    /// 获取边
    #[must_use] 
    pub fn get_edge(&self, id: &str) -> Option<&Edge> {
        self.edges.iter().find(|e| e.id == id)
    }

    /// 获取边（可变引用）
    pub fn get_edge_mut(&mut self, id: &str) -> Option<&mut Edge> {
        self.edges.iter_mut().find(|e| e.id == id)
    }

    /// 获取所有边
    #[must_use] 
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// 获取边数量
    #[must_use] 
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// 获取节点的出边
    #[must_use] 
    pub fn outgoing_edges(&self, node_id: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.source == node_id).collect()
    }

    /// 获取节点的入边
    #[must_use] 
    pub fn incoming_edges(&self, node_id: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.target == node_id).collect()
    }

    /// 检查两个节点之间是否有边
    #[must_use] 
    pub fn has_edge_between(&self, source: &str, target: &str) -> bool {
        self.edges.iter().any(|e| e.source == source && e.target == target)
    }

    /// 获取两个节点之间的边
    #[must_use] 
    pub fn edges_between(&self, source: &str, target: &str) -> Vec<&Edge> {
        self.edges.iter().filter(|e| e.source == source && e.target == target).collect()
    }

    /// 更新边的条件
    ///
    /// # Errors
    ///
    /// 当边不存在时返回 `GraphError::EdgeNotFound`。
    pub fn update_edge_condition(
        &mut self,
        edge_id: &str,
        condition: Option<String>,
    ) -> Result<(), GraphError> {
        let edge = self
            .edges
            .iter_mut()
            .find(|e| e.id == edge_id)
            .ok_or_else(|| GraphError::EdgeNotFound(edge_id.to_string()))?;

        edge.edge_type = condition.map_or(EdgeType::Direct, |c| EdgeType::Conditional { condition: c });

        Ok(())
    }

    /// 更新边的标签
    ///
    /// # Errors
    ///
    /// 当边不存在时返回 `GraphError::EdgeNotFound`。
    pub fn update_edge_label(
        &mut self,
        edge_id: &str,
        label: Option<String>,
    ) -> Result<(), GraphError> {
        let edge = self
            .edges
            .iter_mut()
            .find(|e| e.id == edge_id)
            .ok_or_else(|| GraphError::EdgeNotFound(edge_id.to_string()))?;

        edge.label = label;
        Ok(())
    }

    /// 删除边
    ///
    /// # Errors
    ///
    /// 当边不存在时返回 `GraphError::EdgeNotFound`。
    pub fn remove_edge(&mut self, id: &str) -> Result<Edge, GraphError> {
        let idx = self
            .edges
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| GraphError::EdgeNotFound(id.to_string()))?;

        Ok(self.edges.remove(idx))
    }

    /// 删除两个节点之间的所有边
    pub fn remove_edges_between(&mut self, source: &str, target: &str) -> Vec<Edge> {
        let mut removed = Vec::new();
        self.edges.retain(|e| {
            if e.source == source && e.target == target {
                removed.push(e.clone());
                false
            } else {
                true
            }
        });
        removed
    }

    /// 删除节点的所有出边
    pub fn remove_outgoing_edges(&mut self, node_id: &str) -> Vec<Edge> {
        let mut removed = Vec::new();
        self.edges.retain(|e| {
            if e.source == node_id {
                removed.push(e.clone());
                false
            } else {
                true
            }
        });
        removed
    }
}

// ============================================================================
// 入口和结束点
// ============================================================================

impl Graph {
    /// 设置入口点
    pub fn set_entry(&mut self, node_id: impl Into<String>) -> &mut Self {
        self.entry_point = Some(node_id.into());
        self
    }

    /// 获取入口点
    #[must_use] 
    pub fn entry_point(&self) -> Option<&str> {
        self.entry_point.as_deref()
    }

    /// 获取结束节点（无出边的节点）
    #[must_use] 
    pub fn end_nodes(&self) -> Vec<&str> {
        self.nodes
            .keys()
            .filter(|id| self.outgoing_edges(id).is_empty())
            .map(std::string::String::as_str)
            .collect()
    }

    /// 检查节点是否为结束节点
    #[must_use] 
    pub fn is_end_node(&self, node_id: &str) -> bool {
        self.outgoing_edges(node_id).is_empty()
    }
}

// ============================================================================
// UI 支持
// ============================================================================

impl Graph {
    /// 设置节点位置（用于 UI 布局）
    ///
    /// # Errors
    ///
    /// 当节点不存在时返回 `GraphError::NodeNotFound`。
    pub fn set_node_position(&mut self, node_id: &str, x: f64, y: f64) -> Result<(), GraphError> {
        let node = self
            .nodes
            .get_mut(node_id)
            .ok_or_else(|| GraphError::NodeNotFound(node_id.to_string()))?;

        node.position = Some(Position { x, y });
        Ok(())
    }

    /// 获取节点位置
    #[must_use] 
    pub fn get_node_position(&self, node_id: &str) -> Option<&Position> {
        self.nodes.get(node_id)?.position.as_ref()
    }
}

// ============================================================================
// 验证
// ============================================================================

impl Graph {
    /// 验证图的完整性
    ///
    /// # Errors
    ///
    /// 当图存在结构问题（无入口点、悬空边、孤立节点等）时返回 `GraphError`。
    pub fn validate(&self) -> Result<(), GraphError> {
        let mut errors = Vec::new();

        // 1. 检查入口点
        match &self.entry_point {
            None => errors.push(GraphError::NoEntryPoint),
            Some(entry) if !self.nodes.contains_key(entry) => {
                errors.push(GraphError::InvalidEntryPoint(entry.clone()));
            }
            _ => {}
        }

        // 2. 检查边引用的节点
        for edge in &self.edges {
            if !self.nodes.contains_key(&edge.source) {
                errors.push(GraphError::DanglingEdge {
                    edge_id: edge.id.clone(),
                    missing_node: edge.source.clone(),
                });
            }
            if !self.nodes.contains_key(&edge.target) {
                errors.push(GraphError::DanglingEdge {
                    edge_id: edge.id.clone(),
                    missing_node: edge.target.clone(),
                });
            }
        }

        // 3. 检查孤立节点
        for node_id in self.nodes.keys() {
            let has_incoming = !self.incoming_edges(node_id).is_empty();
            let has_outgoing = !self.outgoing_edges(node_id).is_empty();
            let is_entry = self.entry_point.as_deref() == Some(node_id);

            if !has_incoming && !has_outgoing && !is_entry {
                errors.push(GraphError::OrphanNode(node_id.clone()));
            }
        }

        // 4. 检查可达性
        if let Some(entry) = &self.entry_point {
            if self.nodes.contains_key(entry) {
                let reachable = self.reachable_from(entry);
                for node_id in self.nodes.keys() {
                    if !reachable.contains(node_id) {
                        errors.push(GraphError::UnreachableNode(node_id.clone()));
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else if errors.len() == 1 {
            Err(errors.into_iter().next().unwrap_or(GraphError::NoEntryPoint))
        } else {
            Err(GraphError::ValidationFailed(errors))
        }
    }

    /// 获取从指定节点可达的所有节点
    fn reachable_from(&self, start: &str) -> HashSet<String> {
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(start.to_string());

        while let Some(node) = queue.pop_front() {
            if visited.contains(&node) {
                continue;
            }
            visited.insert(node.clone());

            for edge in self.outgoing_edges(&node) {
                queue.push_back(edge.target.clone());
            }
        }

        visited
    }
}
