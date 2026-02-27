//! Graph 和 Definition 之间的转换
//!
//! 提供 Graph 和 `GraphDefinition` 之间的双向转换。

use crate::definition::{
    EdgeDefinition, EdgeTypeDefinition, GraphDefinition, GraphMetadataDefinition, NodeDefinition,
};
use crate::error::GraphError;
use crate::graph::{EdgeType, Graph};
use crate::node::Node;
use std::collections::HashMap;

/// Graph 转换为 Definition
impl From<&Graph> for GraphDefinition {
    fn from(graph: &Graph) -> Self {
        let metadata = GraphMetadataDefinition {
            description: graph.metadata().description.clone(),
            version: graph.metadata().version.clone(),
            author: graph.metadata().author.clone(),
            tags: graph.metadata().tags.clone(),
            custom: HashMap::default(),
        };

        let nodes: Vec<NodeDefinition> = graph
            .nodes()
            .map(|(id, node)| NodeDefinition {
                id: id.clone(),
                name: node.name.clone(),
                config: node.config.clone(),
                position: node.position.clone(),
                metadata: node.metadata.clone(),
            })
            .collect();

        let edges: Vec<EdgeDefinition> = graph
            .edges()
            .iter()
            .enumerate()
            .map(|(idx, edge)| EdgeDefinition {
                id: format!("edge_{idx}"),
                source: edge.source.clone(),
                target: edge.target.clone(),
                edge_type: match &edge.edge_type {
                    EdgeType::Direct => EdgeTypeDefinition::Direct,
                    EdgeType::Conditional { condition } => {
                        EdgeTypeDefinition::Conditional { condition: condition.clone() }
                    }
                },
            })
            .collect();

        Self {
            id: graph.id().to_string(),
            name: graph.name().to_string(),
            metadata,
            nodes,
            edges,
            entry_point: graph.entry_point().map(std::string::ToString::to_string),
        }
    }
}

/// Definition 转换为 Graph
impl TryFrom<GraphDefinition> for Graph {
    type Error = GraphError;

    fn try_from(def: GraphDefinition) -> Result<Self, Self::Error> {
        let mut graph = Self::with_id(&def.id, &def.name);

        // 设置元数据
        if let Some(desc) = def.metadata.description {
            graph = graph.description(&desc);
        }
        if let Some(ver) = def.metadata.version {
            graph = graph.version(&ver);
        }
        if let Some(author) = def.metadata.author {
            graph = graph.author(&author);
        }
        for tag in def.metadata.tags {
            graph = graph.tag(&tag);
        }

        // 添加节点
        for node_def in def.nodes {
            let node = Node {
                name: node_def.name,
                config: node_def.config,
                position: node_def.position,
                metadata: node_def.metadata,
            };
            graph.add_node(&node_def.id, node);
        }

        // 添加边
        for edge_def in def.edges {
            match edge_def.edge_type {
                EdgeTypeDefinition::Direct => {
                    graph.add_edge(&edge_def.source, &edge_def.target);
                }
                EdgeTypeDefinition::Conditional { condition } => {
                    graph.add_conditional_edge(&edge_def.source, &edge_def.target, &condition);
                }
            }
        }

        // 设置入口点
        if let Some(entry) = def.entry_point {
            graph.set_entry(&entry);
        }

        Ok(graph)
    }
}
