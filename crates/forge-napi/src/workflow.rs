//! Workflow NAPI bindings

use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use forge_workflow::definition::{
    EdgeDefinition, EdgeTypeDefinition, GraphDefinition, GraphMetadataDefinition, NodeDefinition,
};
use forge_workflow::persistence::{FileCheckpointStore, FileWorkflowStore, WorkflowStore};
use forge_workflow::{
    DefaultNodeExecutor, Graph, NodeConfig, Position, WorkflowEvent, WorkflowExecutor,
    WorkflowStatus,
};
use tokio::sync::RwLock;

use crate::sdk::ForgeSDK as JsForgeSDK;

// ========================
// Type Definitions
// ========================

/// Position in the workflow editor.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsPosition {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
}

impl From<Position> for JsPosition {
    fn from(p: Position) -> Self {
        Self { x: p.x, y: p.y }
    }
}

impl From<JsPosition> for Position {
    fn from(p: JsPosition) -> Self {
        Self { x: p.x, y: p.y }
    }
}

/// Graph metadata definition.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsGraphMetadata {
    /// Graph description.
    pub description: Option<String>,
    /// Graph version.
    pub version: Option<String>,
    /// Graph author.
    pub author: Option<String>,
    /// Graph tags.
    pub tags: Vec<String>,
}

impl From<GraphMetadataDefinition> for JsGraphMetadata {
    fn from(m: GraphMetadataDefinition) -> Self {
        Self { description: m.description, version: m.version, author: m.author, tags: m.tags }
    }
}

impl From<JsGraphMetadata> for GraphMetadataDefinition {
    fn from(m: JsGraphMetadata) -> Self {
        Self {
            description: m.description,
            version: m.version,
            author: m.author,
            tags: m.tags,
            custom: Default::default(),
        }
    }
}

/// Edge definition for workflow graph.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsEdgeDefinition {
    /// Edge ID.
    pub id: String,
    /// Source node ID.
    pub source: String,
    /// Target node ID.
    pub target: String,
    /// Edge type: "direct" or "conditional".
    pub edge_type: String,
    /// Condition expression (for conditional edges).
    pub condition: Option<String>,
}

impl From<EdgeDefinition> for JsEdgeDefinition {
    fn from(e: EdgeDefinition) -> Self {
        let (edge_type, condition) = match e.edge_type {
            EdgeTypeDefinition::Direct => ("direct".to_string(), None),
            EdgeTypeDefinition::Conditional { condition } => {
                ("conditional".to_string(), Some(condition))
            }
        };
        Self { id: e.id, source: e.source, target: e.target, edge_type, condition }
    }
}

impl From<JsEdgeDefinition> for EdgeDefinition {
    fn from(e: JsEdgeDefinition) -> Self {
        let edge_type = if e.edge_type == "conditional" {
            EdgeTypeDefinition::Conditional { condition: e.condition.unwrap_or_default() }
        } else {
            EdgeTypeDefinition::Direct
        };
        Self { id: e.id, source: e.source, target: e.target, edge_type }
    }
}

/// Node definition for workflow graph.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsNodeDefinition {
    /// Node ID.
    pub id: String,
    /// Node name.
    pub name: String,
    /// Node type string (for display).
    pub node_type: String,
    /// Node config JSON string.
    pub config: String,
    /// Position in the editor.
    pub position: Option<JsPosition>,
    /// Custom metadata.
    pub metadata: HashMap<String, String>,
}

impl From<NodeDefinition> for JsNodeDefinition {
    fn from(n: NodeDefinition) -> Self {
        let node_type = match &n.config {
            NodeConfig::Agent(_) => "agent",
            NodeConfig::Tool(_) => "tool",
            NodeConfig::Router(_) => "router",
            NodeConfig::Human(_) => "human",
            NodeConfig::Parallel(_) => "parallel",
            NodeConfig::SubWorkflow(_) => "sub_workflow",
        };
        Self {
            id: n.id,
            name: n.name,
            node_type: node_type.to_string(),
            config: serde_json::to_string(&n.config).unwrap_or_default(),
            position: n.position.map(Into::into),
            metadata: n.metadata,
        }
    }
}

/// Graph definition for workflow.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsGraphDefinition {
    /// Workflow ID.
    pub id: String,
    /// Workflow name.
    pub name: String,
    /// Metadata.
    pub metadata: JsGraphMetadata,
    /// Nodes.
    pub nodes: Vec<JsNodeDefinition>,
    /// Edges.
    pub edges: Vec<JsEdgeDefinition>,
    /// Entry point node ID.
    pub entry_point: Option<String>,
}

impl From<GraphDefinition> for JsGraphDefinition {
    fn from(g: GraphDefinition) -> Self {
        Self {
            id: g.id,
            name: g.name,
            metadata: g.metadata.into(),
            nodes: g.nodes.into_iter().map(Into::into).collect(),
            edges: g.edges.into_iter().map(Into::into).collect(),
            entry_point: g.entry_point,
        }
    }
}

/// Workflow info for listing.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsWorkflowInfo {
    /// Workflow ID.
    pub id: String,
    /// Workflow name.
    pub name: String,
    /// Workflow description.
    pub description: Option<String>,
    /// Workflow version.
    pub version: Option<String>,
}

/// Workflow event payload.
#[napi(object)]
#[derive(Debug, Clone)]
pub struct JsWorkflowEvent {
    /// Event type.
    pub event_type: String,
    /// Workflow ID.
    pub workflow_id: Option<String>,
    /// Workflow name.
    pub workflow_name: Option<String>,
    /// Node ID.
    pub node_id: Option<String>,
    /// Node name.
    pub node_name: Option<String>,
    /// Node type.
    pub node_type: Option<String>,
    /// Progress or error message.
    pub message: Option<String>,
    /// Node output (JSON string).
    pub output: Option<String>,
    /// Node execution duration (ms).
    pub duration_ms: Option<f64>,
    /// Error description.
    pub error: Option<String>,
    /// Next node ID (route decision).
    pub next_node: Option<String>,
    /// Matched condition expression.
    pub condition: Option<String>,
    /// Decision explanation (JSON string).
    pub explanation: Option<String>,
    /// Human input prompt.
    pub prompt: Option<String>,
    /// Human input type.
    pub input_type: Option<String>,
    /// Human input options (JSON string).
    pub options: Option<String>,
    /// Human input value (JSON string).
    pub value: Option<String>,
    /// State key updated.
    pub key: Option<String>,
    /// State value (JSON string).
    pub state_value: Option<String>,
    /// Checkpoint ID.
    pub checkpoint_id: Option<String>,
    /// Workflow result (JSON string).
    pub result: Option<String>,
    /// Total duration (ms).
    pub total_duration_ms: Option<f64>,
    /// Number of nodes executed.
    pub nodes_executed: Option<u32>,
}

impl JsWorkflowEvent {
    fn new(event_type: &str) -> Self {
        Self {
            event_type: event_type.to_string(),
            workflow_id: None,
            workflow_name: None,
            node_id: None,
            node_name: None,
            node_type: None,
            message: None,
            output: None,
            duration_ms: None,
            error: None,
            next_node: None,
            condition: None,
            explanation: None,
            prompt: None,
            input_type: None,
            options: None,
            value: None,
            key: None,
            state_value: None,
            checkpoint_id: None,
            result: None,
            total_duration_ms: None,
            nodes_executed: None,
        }
    }
}

impl From<WorkflowEvent> for JsWorkflowEvent {
    fn from(event: WorkflowEvent) -> Self {
        match event {
            WorkflowEvent::Started { workflow_id, workflow_name } => {
                let mut e = Self::new("started");
                e.workflow_id = Some(workflow_id);
                e.workflow_name = Some(workflow_name);
                e
            }
            WorkflowEvent::NodeStarted { node_id, node_name, node_type } => {
                let mut e = Self::new("node_started");
                e.node_id = Some(node_id);
                e.node_name = Some(node_name);
                e.node_type = Some(node_type);
                e
            }
            WorkflowEvent::NodeProgress { node_id, message } => {
                let mut e = Self::new("node_progress");
                e.node_id = Some(node_id);
                e.message = Some(message);
                e
            }
            WorkflowEvent::NodeCompleted { node_id, output, duration_ms } => {
                let mut e = Self::new("node_completed");
                e.node_id = Some(node_id);
                e.output = Some(serde_json::to_string(&output).unwrap_or_default());
                e.duration_ms = Some(duration_ms as f64);
                e
            }
            WorkflowEvent::NodeFailed { node_id, error } => {
                let mut e = Self::new("node_failed");
                e.node_id = Some(node_id);
                e.error = Some(error);
                e
            }
            WorkflowEvent::RouteDecision { node_id, next_node, condition, explanation } => {
                let mut e = Self::new("route_decision");
                e.node_id = Some(node_id);
                e.next_node = Some(next_node);
                e.condition = condition;
                e.explanation =
                    explanation.map(|exp| serde_json::to_string(&exp).unwrap_or_default());
                e
            }
            WorkflowEvent::WaitingForHuman { node_id, prompt, input_type, options } => {
                let mut e = Self::new("waiting_for_human");
                e.node_id = Some(node_id);
                e.prompt = Some(prompt);
                e.input_type = Some(format!("{:?}", input_type).to_lowercase());
                e.options =
                    options.map(|opts| serde_json::to_string(&opts).unwrap_or_default());
                e
            }
            WorkflowEvent::HumanInputReceived { node_id, value } => {
                let mut e = Self::new("human_input_received");
                e.node_id = Some(node_id);
                e.value = Some(serde_json::to_string(&value).unwrap_or_default());
                e
            }
            WorkflowEvent::StateUpdated { key, value } => {
                let mut e = Self::new("state_updated");
                e.key = Some(key);
                e.state_value = Some(serde_json::to_string(&value).unwrap_or_default());
                e
            }
            WorkflowEvent::CheckpointSaved { checkpoint_id } => {
                let mut e = Self::new("checkpoint_saved");
                e.checkpoint_id = Some(checkpoint_id);
                e
            }
            WorkflowEvent::Completed { result, total_duration_ms, nodes_executed } => {
                let mut e = Self::new("completed");
                e.result = Some(serde_json::to_string(&result).unwrap_or_default());
                e.total_duration_ms = Some(total_duration_ms as f64);
                e.nodes_executed =
                    Some(u32::try_from(nodes_executed).unwrap_or(u32::MAX));
                e
            }
            WorkflowEvent::Failed { error, failed_node } => {
                let mut e = Self::new("failed");
                e.error = Some(error);
                e.node_id = failed_node;
                e
            }
            WorkflowEvent::Cancelled => Self::new("cancelled"),
        }
    }
}

// ========================
// WorkflowManager Class
// ========================

/// Workflow Manager — manages workflow CRUD and execution.
#[allow(missing_docs)]
#[napi]
pub struct WorkflowManager {
    store: Arc<FileWorkflowStore>,
    _checkpoint_store: Arc<FileCheckpointStore>,
    sdk: Option<Arc<RwLock<Option<forge_sdk::ForgeSDK>>>>,
}

#[allow(missing_docs)]
#[napi]
impl WorkflowManager {
    /// Create a new WorkflowManager.
    ///
    /// @param baseDir - Base directory for workflow storage.
    #[napi(constructor)]
    pub fn new(base_dir: String) -> napi::Result<Self> {
        let path = PathBuf::from(&base_dir);
        let store = Arc::new(FileWorkflowStore::new(path.join("workflows")));
        let checkpoint_store = Arc::new(FileCheckpointStore::new(path.join("checkpoints")));
        Ok(Self { store, _checkpoint_store: checkpoint_store, sdk: None })
    }

    /// Attach a ForgeSDK instance for workflow execution.
    #[napi]
    pub fn set_sdk(&mut self, sdk: &JsForgeSDK) {
        self.sdk = Some(sdk.inner_handle());
    }

    // ========================
    // CRUD Operations
    // ========================

    /// Save a workflow.
    #[napi]
    pub async fn save(&self, workflow: JsGraphDefinition) -> napi::Result<()> {
        let def = GraphDefinition {
            id: workflow.id,
            name: workflow.name,
            metadata: workflow.metadata.into(),
            nodes: {
                let mut nodes = Vec::with_capacity(workflow.nodes.len());
                for n in workflow.nodes {
                    let config: NodeConfig =
                        serde_json::from_str(&n.config).map_err(|e| {
                            napi::Error::from_reason(format!(
                                "Invalid config for node '{}': {e}",
                                n.id
                            ))
                        })?;
                    nodes.push(NodeDefinition {
                        id: n.id,
                        name: n.name,
                        config,
                        position: n.position.map(Into::into),
                        metadata: n.metadata,
                    });
                }
                nodes
            },
            edges: workflow.edges.into_iter().map(Into::into).collect(),
            entry_point: workflow.entry_point,
        };

        self.store
            .save(&def)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to save workflow: {e}")))
    }

    /// Load a workflow by ID.
    #[napi]
    pub async fn load(&self, id: String) -> napi::Result<Option<JsGraphDefinition>> {
        let result = self
            .store
            .load(&id)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to load workflow: {e}")))?;
        Ok(result.map(Into::into))
    }

    /// List all workflows.
    #[napi]
    pub async fn list(&self) -> napi::Result<Vec<JsWorkflowInfo>> {
        let workflows = self
            .store
            .list()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to list workflows: {e}")))?;

        Ok(workflows
            .into_iter()
            .map(|w| JsWorkflowInfo {
                id: w.id,
                name: w.name,
                description: w.description,
                version: w.version,
            })
            .collect())
    }

    /// Delete a workflow by ID.
    #[napi]
    pub async fn delete(&self, id: String) -> napi::Result<()> {
        self.store
            .delete(&id)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to delete workflow: {e}")))
    }

    /// Check if a workflow exists.
    #[napi]
    pub async fn exists(&self, id: String) -> napi::Result<bool> {
        self.store
            .exists(&id)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to check workflow: {e}")))
    }

    // ========================
    // Execution API
    // ========================

    /// Execute a workflow and stream events to a callback.
    #[napi]
    pub async fn execute_stream(
        &self,
        workflow_id: String,
        input: String,
        callback: ThreadsafeFunction<JsWorkflowEvent>,
    ) -> napi::Result<()> {
        let workflow = self
            .store
            .load(&workflow_id)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to load workflow: {e}")))?
            .ok_or_else(|| {
                napi::Error::from_reason(format!("Workflow not found: {workflow_id}"))
            })?;

        let executor = DefaultNodeExecutor::new();
        let graph = Graph::try_from(workflow)
            .map_err(|e| napi::Error::from_reason(format!("Invalid workflow graph: {e}")))?;
        let mut exec = WorkflowExecutor::with_store(graph, executor, self.store.clone());

        let input_value = parse_input_value(input);
        let tsfn = callback;
        let mut emit = |event: &WorkflowEvent| {
            let js_event: JsWorkflowEvent = event.clone().into();
            let _ = tsfn.call(Ok(js_event), ThreadsafeFunctionCallMode::NonBlocking);
        };

        exec.run_with_callback(input_value, &mut emit).await;
        Ok(())
    }

    /// Execute a workflow and return aggregated result as JSON.
    #[napi]
    pub async fn execute(&self, workflow_id: String, input: String) -> napi::Result<String> {
        let workflow = self
            .store
            .load(&workflow_id)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to load workflow: {e}")))?
            .ok_or_else(|| {
                napi::Error::from_reason(format!("Workflow not found: {workflow_id}"))
            })?;

        let executor = DefaultNodeExecutor::new();
        let graph = Graph::try_from(workflow)
            .map_err(|e| napi::Error::from_reason(format!("Invalid workflow graph: {e}")))?;
        let mut exec = WorkflowExecutor::with_store(graph, executor, self.store.clone());

        let input_value = parse_input_value(input);
        exec.run(input_value).await;

        let state = exec.state_mut();
        let (status, error, failed_node) = match &state.status {
            WorkflowStatus::Completed => ("completed", None, None),
            WorkflowStatus::WaitingForHuman { node, .. } => {
                ("waiting", None, Some(node.clone()))
            }
            WorkflowStatus::Cancelled => ("cancelled", None, None),
            WorkflowStatus::Failed { error, node } => {
                ("failed", Some(error.clone()), Some(node.clone()))
            }
            WorkflowStatus::Pending => ("pending", None, None),
            WorkflowStatus::Running => ("running", None, None),
        };

        Ok(serde_json::json!({
            "status": status,
            "workflow_id": workflow_id,
            "data": state.data.clone(),
            "error": error,
            "failed_node": failed_node,
        })
        .to_string())
    }
}

fn parse_input_value(input: String) -> serde_json::Value {
    if input.trim().is_empty() {
        return serde_json::json!({});
    }
    serde_json::from_str(&input).unwrap_or(serde_json::Value::String(input))
}
