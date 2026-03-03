//! 工作流执行器模块

mod config;
mod error;
mod event;

pub use config::*;
pub use error::*;
pub use event::*;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::graph::{EdgeType, Graph};
use crate::node::{
    AgentNodeConfig, HumanNodeConfig, NodeConfig, ParallelFailurePolicy, ParallelNodeConfig,
    RouterNodeConfig, SubWorkflowNodeConfig, ToolNodeConfig,
};
use crate::persistence::WorkflowStore;
use crate::state::{NodeExecution, WorkflowState, WorkflowStatus};

// ============================================================================
// 节点执行 Trait
// ============================================================================

/// 节点执行器 trait
///
/// 由外部实现，提供具体的节点执行逻辑
#[async_trait]
pub trait NodeExecutor: Send + Sync {
    /// 执行 Agent 节点
    async fn execute_agent(
        &self,
        node_id: &str,
        config: &AgentNodeConfig,
        state: &WorkflowState,
    ) -> Result<serde_json::Value, ExecutionError>;

    /// 执行 Tool 节点
    async fn execute_tool(
        &self,
        node_id: &str,
        config: &ToolNodeConfig,
        state: &WorkflowState,
    ) -> Result<serde_json::Value, ExecutionError>;

    /// 渲染模板
    ///
    /// # Errors
    ///
    /// 当模板渲染失败时返回 `ExecutionError`。
    fn render_template(
        &self,
        template: &str,
        state: &WorkflowState,
    ) -> Result<String, ExecutionError>;

    /// 评估表达式
    ///
    /// # Errors
    ///
    /// 当表达式求值失败时返回 `ExecutionError`。
    fn evaluate_expression(
        &self,
        expression: &str,
        state: &WorkflowState,
    ) -> Result<serde_json::Value, ExecutionError>;
}

// ============================================================================
// WorkflowExecutor
// ============================================================================

/// 工作流执行器
pub struct WorkflowExecutor<E: NodeExecutor> {
    /// 工作流图（不可变）
    graph: Arc<Graph>,
    /// 运行时状态
    state: WorkflowState,
    /// 节点执行器
    executor: Arc<E>,
    /// 执行配置
    config: ExecutorConfig,
    /// 工作流存储（用于子工作流）
    workflow_store: Option<Arc<dyn WorkflowStore>>,
    /// 子工作流递归深度
    subworkflow_depth: usize,
    /// 取消令牌
    cancel_token: CancellationToken,
}

struct EventSink<'a> {
    events: &'a mut Vec<WorkflowEvent>,
    callback: Option<&'a mut (dyn FnMut(&WorkflowEvent) + Send)>,
}

impl<'a> EventSink<'a> {
    fn new(
        events: &'a mut Vec<WorkflowEvent>,
        callback: Option<&'a mut (dyn FnMut(&WorkflowEvent) + Send)>,
    ) -> Self {
        Self { events, callback }
    }

    fn emit(&mut self, event: WorkflowEvent) {
        if let Some(callback) = self.callback.as_mut() {
            callback(&event);
        }
        self.events.push(event);
    }
}

#[derive(Debug)]
struct BranchDef {
    index: usize,
    node_id: String,
    config: NodeConfig,
}

#[derive(Debug)]
struct BranchResult {
    index: usize,
    node_id: String,
    result: Result<DetachedOutcome, ExecutionError>,
    duration_ms: u64,
}

#[derive(Debug)]
struct DetachedOutcome {
    output: serde_json::Value,
    mapped_outputs: Vec<(String, serde_json::Value)>,
}

#[allow(clippy::too_many_arguments)]
async fn execute_node_detached<E: NodeExecutor>(
    graph: &Graph,
    executor: &Arc<E>,
    store: Option<&Arc<dyn WorkflowStore>>,
    exec_config: &ExecutorConfig,
    depth: usize,
    node_id: &str,
    config: &NodeConfig,
    state: &WorkflowState,
) -> Result<DetachedOutcome, ExecutionError> {
    match config {
        NodeConfig::Agent(cfg) => Ok(DetachedOutcome {
            output: executor.execute_agent(node_id, cfg, state).await?,
            mapped_outputs: Vec::new(),
        }),
        NodeConfig::Tool(cfg) => Ok(DetachedOutcome {
            output: executor.execute_tool(node_id, cfg, state).await?,
            mapped_outputs: Vec::new(),
        }),
        NodeConfig::Router(cfg) => {
            let route_value = executor.evaluate_expression(&cfg.expression, state)?;
            Ok(DetachedOutcome {
                output: serde_json::json!({ "route": route_value }),
                mapped_outputs: Vec::new(),
            })
        }
        NodeConfig::Human(cfg) => {
            let prompt = executor.render_template(&cfg.prompt, state)?;
            Err(ExecutionError::WaitingForHuman {
                prompt,
                input_type: cfg.input_type.clone(),
                options: cfg.options.clone(),
            })
        }
        NodeConfig::Parallel(cfg) => {
            execute_parallel_detached(graph, executor, store, exec_config, depth, cfg, state).await
        }
        NodeConfig::SubWorkflow(cfg) => {
            execute_subworkflow_detached(executor, store, exec_config, depth, cfg, state).await
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn execute_parallel_detached<E: NodeExecutor>(
    graph: &Graph,
    executor: &Arc<E>,
    store: Option<&Arc<dyn WorkflowStore>>,
    exec_config: &ExecutorConfig,
    depth: usize,
    config: &ParallelNodeConfig,
    state: &WorkflowState,
) -> Result<DetachedOutcome, ExecutionError> {
    if config.branches.is_empty() {
        return Err(ExecutionError::JoinError("Parallel node has no branches".to_string()));
    }

    let mut branch_defs = Vec::new();
    for (idx, branch_id) in config.branches.iter().enumerate() {
        let node = graph
            .get_node(branch_id)
            .ok_or_else(|| ExecutionError::NodeNotFound(branch_id.clone()))?;
        branch_defs.push(BranchDef {
            index: idx,
            node_id: branch_id.clone(),
            config: node.config.clone(),
        });
    }

    let limit = config.concurrency_limit.unwrap_or(config.branches.len()).max(1);
    let semaphore = Arc::new(Semaphore::new(limit));

    let graph = Arc::new((*graph).clone());
    let mut tasks = FuturesUnordered::new();
    for branch in branch_defs {
        let semaphore = semaphore.clone();
        let graph = graph.clone();
        let executor = executor.clone();
        let store = store.cloned();
        let exec_config = exec_config.clone();
        let state = state.clone();
        tasks.push(async move {
            let result = match semaphore
                .acquire_owned()
                .await
                .map_err(|_| ExecutionError::Other("Parallel semaphore closed".to_string()))
            {
                Ok(_permit) => {
                    execute_node_detached(
                        &graph,
                        &executor,
                        store.as_ref(),
                        &exec_config,
                        depth,
                        &branch.node_id,
                        &branch.config,
                        &state,
                    )
                    .await
                }
                Err(err) => Err(err),
            };

            BranchResult { index: branch.index, node_id: branch.node_id, result, duration_ms: 0 }
        });
    }

    let mut results = Vec::new();
    while let Some(result) = tasks.next().await {
        results.push(result);
    }

    results.sort_by_key(|r| r.index);

    let mut successes = 0usize;
    let mut failures = Vec::new();
    let mut outputs = serde_json::Map::new();
    let mut errors = serde_json::Map::new();
    let mut mapped_outputs = Vec::new();

    for result in results {
        match result.result {
            Ok(outcome) => {
                successes += 1;
                outputs.insert(result.node_id.clone(), outcome.output.clone());
                mapped_outputs.push((format!("{}_output", result.node_id), outcome.output.clone()));
                mapped_outputs.extend(outcome.mapped_outputs);
            }
            Err(err) => {
                failures.push(result.node_id.clone());
                errors.insert(result.node_id.clone(), serde_json::Value::String(err.to_string()));
            }
        }
    }

    let join_success = match config.join_strategy {
        crate::node::JoinStrategy::All => successes == config.branches.len(),
        crate::node::JoinStrategy::Any => successes >= 1,
        crate::node::JoinStrategy::Count(count) => successes >= count,
    };

    if matches!(config.failure_policy, ParallelFailurePolicy::FailFast) && !errors.is_empty() {
        return Err(ExecutionError::JoinError(format!(
            "Parallel node failed: {} errors",
            errors.len()
        )));
    }

    if !join_success {
        return Err(ExecutionError::JoinError(
            "Parallel node did not meet join strategy".to_string(),
        ));
    }

    Ok(DetachedOutcome {
        output: serde_json::json!({
            "status": "completed",
            "branches": outputs,
            "errors": errors,
            "successes": successes,
            "failures": failures,
        }),
        mapped_outputs,
    })
}

async fn execute_subworkflow_detached<E: NodeExecutor>(
    executor: &Arc<E>,
    store: Option<&Arc<dyn WorkflowStore>>,
    exec_config: &ExecutorConfig,
    depth: usize,
    config: &SubWorkflowNodeConfig,
    state: &WorkflowState,
) -> Result<DetachedOutcome, ExecutionError> {
    let store =
        store.ok_or_else(|| ExecutionError::Other("Workflow store not configured".to_string()))?;

    let max_depth = config.max_depth.unwrap_or(exec_config.max_subworkflow_depth);
    if depth >= max_depth {
        return Err(ExecutionError::Other(format!("Subworkflow depth exceeded (max {max_depth})")));
    }

    let def = store
        .load(&config.workflow_id)
        .await
        .map_err(|e| ExecutionError::Other(format!("Failed to load workflow: {e}")))?
        .ok_or_else(|| {
            ExecutionError::Other(format!("Subworkflow not found: {}", config.workflow_id))
        })?;

    let graph = Graph::try_from(def)?;
    let input = build_subworkflow_input(state, config);

    let mut sub_executor = build_subworkflow_executor(
        graph,
        executor.clone(),
        exec_config.clone(),
        Some(store.clone()),
        depth + 1,
    );

    let _events = sub_executor.run(input).await;

    if sub_executor.state().is_waiting_for_human() {
        return Err(ExecutionError::Other("Subworkflow is waiting for human input".to_string()));
    }
    if sub_executor.state().is_failed() {
        let detail = match &sub_executor.state().status {
            WorkflowStatus::Failed { error, node } => {
                format!("{error} (node: {node})")
            }
            _ => "unknown error".to_string(),
        };
        return Err(ExecutionError::Other(format!(
            "Subworkflow {} failed: {}",
            config.workflow_id, detail
        )));
    }
    if sub_executor.state().is_cancelled() {
        return Err(ExecutionError::Other(format!("Subworkflow {} cancelled", config.workflow_id)));
    }

    let mut mapped_outputs = Vec::new();
    for (from, to) in &config.output_mapping {
        if let Some(value) = sub_executor.state().get(from) {
            mapped_outputs.push((to.clone(), value.clone()));
        }
    }

    Ok(DetachedOutcome {
        output: serde_json::json!({
            "workflow_id": config.workflow_id,
            "status": "completed",
            "result": sub_executor.state().data.clone(),
            "nodes_executed": sub_executor.state().executed_count(),
        }),
        mapped_outputs,
    })
}

fn build_subworkflow_executor<E: NodeExecutor>(
    graph: Graph,
    executor: Arc<E>,
    config: ExecutorConfig,
    store: Option<Arc<dyn WorkflowStore>>,
    depth: usize,
) -> WorkflowExecutor<E> {
    WorkflowExecutor {
        graph: Arc::new(graph),
        state: WorkflowState::new(),
        executor,
        config,
        workflow_store: store,
        subworkflow_depth: depth,
        cancel_token: CancellationToken::new(),
    }
}

fn build_subworkflow_input(
    state: &WorkflowState,
    config: &SubWorkflowNodeConfig,
) -> serde_json::Value {
    if config.input_mapping.is_empty() {
        let mut map = serde_json::Map::new();
        for (k, v) in &state.data {
            map.insert(k.clone(), v.clone());
        }
        return serde_json::Value::Object(map);
    }

    let mut map = serde_json::Map::new();
    for (from, to) in &config.input_mapping {
        if let Some(value) = state.get(from) {
            map.insert(to.clone(), value.clone());
        }
    }
    serde_json::Value::Object(map)
}

impl<E: NodeExecutor> WorkflowExecutor<E> {
    /// 创建新的执行器
    pub fn new(graph: Graph, executor: E) -> Self {
        Self {
            graph: Arc::new(graph),
            state: WorkflowState::new(),
            executor: Arc::new(executor),
            config: ExecutorConfig::default(),
            workflow_store: None,
            subworkflow_depth: 0,
            cancel_token: CancellationToken::new(),
        }
    }

    /// 使用配置创建执行器
    pub fn with_config(graph: Graph, executor: E, config: ExecutorConfig) -> Self {
        Self {
            graph: Arc::new(graph),
            state: WorkflowState::new(),
            executor: Arc::new(executor),
            config,
            workflow_store: None,
            subworkflow_depth: 0,
            cancel_token: CancellationToken::new(),
        }
    }

    /// 使用存储创建执行器（用于子工作流加载）
    pub fn with_store(graph: Graph, executor: E, store: Arc<dyn WorkflowStore>) -> Self {
        let mut exec = Self::new(graph, executor);
        exec.workflow_store = Some(store);
        exec
    }

    /// 使用配置与存储创建执行器
    pub fn with_config_and_store(
        graph: Graph,
        executor: E,
        config: ExecutorConfig,
        store: Arc<dyn WorkflowStore>,
    ) -> Self {
        let mut exec = Self::with_config(graph, executor, config);
        exec.workflow_store = Some(store);
        exec
    }

    /// 获取图引用
    #[must_use]
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// 获取状态引用
    #[must_use]
    pub const fn state(&self) -> &WorkflowState {
        &self.state
    }

    /// 获取状态可变引用
    pub fn state_mut(&mut self) -> &mut WorkflowState {
        &mut self.state
    }

    /// 取消执行
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// 获取取消令牌（用于外部取消）
    #[must_use]
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// 检查是否已取消
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// 检查工作流是否结束
    const fn is_finished(&self) -> bool {
        matches!(
            self.state.status,
            WorkflowStatus::Completed | WorkflowStatus::Failed { .. } | WorkflowStatus::Cancelled
        )
    }

    /// 执行工作流
    pub async fn run(&mut self, initial_input: serde_json::Value) -> Vec<WorkflowEvent> {
        let mut events = Vec::new();
        let mut sink = EventSink::new(&mut events, None);
        self.run_with_sink(initial_input, &mut sink).await;
        events
    }

    /// 执行工作流（支持事件回调）
    pub async fn run_with_callback<F>(
        &mut self,
        initial_input: serde_json::Value,
        callback: &mut F,
    ) -> Vec<WorkflowEvent>
    where
        F: FnMut(&WorkflowEvent) + Send,
    {
        let mut events = Vec::new();
        let mut sink = EventSink::new(&mut events, Some(callback));
        self.run_with_sink(initial_input, &mut sink).await;
        events
    }

    async fn run_with_sink(&mut self, initial_input: serde_json::Value, sink: &mut EventSink<'_>) {
        let start_time = Instant::now();

        // 初始化状态
        self.state.set("input", initial_input);
        self.state.status = WorkflowStatus::Running;

        // 设置当前节点为入口点
        if let Some(entry) = self.graph.entry_point() {
            self.state.current_node = entry.to_string();
        } else {
            let error = "No entry point found in workflow graph".to_string();
            self.state.status =
                WorkflowStatus::Failed { error: error.clone(), node: String::new() };
            sink.emit(WorkflowEvent::Failed { error, failed_node: None });
            return;
        }

        sink.emit(WorkflowEvent::Started {
            workflow_id: self.graph.id().to_string(),
            workflow_name: self.graph.name().to_string(),
        });

        // 执行主循环
        self.execute_loop(start_time, sink).await;
    }

    /// 执行主循环
    async fn execute_loop(&mut self, start_time: Instant, sink: &mut EventSink<'_>) {
        let mut iterations = 0;

        while !self.is_finished() {
            // 检查取消
            if self.cancel_token.is_cancelled() {
                self.state.status = WorkflowStatus::Cancelled;
                sink.emit(WorkflowEvent::Cancelled);
                return;
            }

            // 检查迭代限制
            iterations += 1;
            if iterations > self.config.max_iterations {
                let error = format!("Max iterations ({}) exceeded", self.config.max_iterations);
                self.state.status = WorkflowStatus::Failed {
                    error: error.clone(),
                    node: self.state.current_node.clone(),
                };
                sink.emit(WorkflowEvent::Failed {
                    error,
                    failed_node: Some(self.state.current_node.clone()),
                });
                return;
            }

            // 检查总超时
            if start_time.elapsed().as_secs() > self.config.total_timeout_secs {
                let error = "Workflow timeout".to_string();
                self.state.status = WorkflowStatus::Failed {
                    error: error.clone(),
                    node: self.state.current_node.clone(),
                };
                sink.emit(WorkflowEvent::Failed {
                    error,
                    failed_node: Some(self.state.current_node.clone()),
                });
                return;
            }

            // 执行当前节点
            self.execute_current_node(sink).await;

            // 检查是否需要等待人工输入
            if self.state.is_waiting_for_human() {
                return;
            }
        }

        // 工作流完成
        if self.state.is_completed() {
            sink.emit(WorkflowEvent::Completed {
                result: serde_json::json!(self.state.data.clone()),
                total_duration_ms: u64::try_from(start_time.elapsed().as_millis())
                    .unwrap_or(u64::MAX),
                nodes_executed: self.state.history.len(),
            });
        }
    }

    /// 执行当前节点
    async fn execute_current_node(&mut self, sink: &mut EventSink<'_>) {
        let node_id = self.state.current_node.clone();

        // 获取节点
        let node = if let Some(n) = self.graph.get_node(&node_id) {
            n.clone()
        } else {
            let error = format!("Node not found: {node_id}");
            self.state.status =
                WorkflowStatus::Failed { error: error.clone(), node: node_id.clone() };
            sink.emit(WorkflowEvent::Failed { error, failed_node: Some(node_id) });
            return;
        };

        // 获取节点类型名称
        let node_type = Self::node_type_name(&node.config);

        sink.emit(WorkflowEvent::NodeStarted {
            node_id: node_id.clone(),
            node_name: node.name.clone(),
            node_type: node_type.to_string(),
        });

        let node_start = Instant::now();

        // 记录执行开始
        let execution = NodeExecution::new(&node_id);
        self.state.push_execution(execution);

        // 执行节点
        let result = self.execute_node(&node_id, &node.config, sink).await;

        match result {
            Ok(output) => {
                // 更新执行记录
                if let Some(exec) = self.state.last_execution_mut() {
                    exec.complete(output.clone());
                }

                // 保存输出到状态
                let output_key = format!("{node_id}_output");
                self.state.set(&output_key, output.clone());

                sink.emit(WorkflowEvent::NodeCompleted {
                    node_id: node_id.clone(),
                    output,
                    duration_ms: u64::try_from(node_start.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                });

                // 确定下一个节点
                self.route_to_next(&node_id, sink);
            }
            Err(ExecutionError::WaitingForHuman { prompt, input_type, options }) => {
                self.state.status = WorkflowStatus::WaitingForHuman {
                    node: node_id.clone(),
                    prompt: prompt.clone(),
                };

                sink.emit(WorkflowEvent::WaitingForHuman { node_id, prompt, input_type, options });
            }
            Err(e) => {
                // 更新执行记录
                if let Some(exec) = self.state.last_execution_mut() {
                    exec.fail(e.to_string());
                }

                self.state.status =
                    WorkflowStatus::Failed { error: e.to_string(), node: node_id.clone() };

                sink.emit(WorkflowEvent::NodeFailed {
                    node_id: node_id.clone(),
                    error: e.to_string(),
                });

                sink.emit(WorkflowEvent::Failed {
                    error: e.to_string(),
                    failed_node: Some(node_id),
                });
            }
        }
    }

    /// 执行节点
    async fn execute_node(
        &mut self,
        node_id: &str,
        config: &NodeConfig,
        sink: &mut EventSink<'_>,
    ) -> Result<serde_json::Value, ExecutionError> {
        match config {
            NodeConfig::Agent(cfg) => self.executor.execute_agent(node_id, cfg, &self.state).await,
            NodeConfig::Tool(cfg) => self.executor.execute_tool(node_id, cfg, &self.state).await,
            NodeConfig::Router(cfg) => self.execute_router(node_id, cfg),
            NodeConfig::Human(cfg) => self.execute_human(node_id, cfg),
            NodeConfig::Parallel(cfg) => self.execute_parallel(node_id, cfg, sink).await,
            NodeConfig::SubWorkflow(cfg) => self.execute_subworkflow(node_id, cfg).await,
        }
    }

    /// 执行 Router 节点
    fn execute_router(
        &self,
        _node_id: &str,
        config: &RouterNodeConfig,
    ) -> Result<serde_json::Value, ExecutionError> {
        let route_value = self.executor.evaluate_expression(&config.expression, &self.state)?;
        Ok(serde_json::json!({
            "route": route_value,
        }))
    }

    /// 执行 Human 节点
    fn execute_human(
        &self,
        _node_id: &str,
        config: &HumanNodeConfig,
    ) -> Result<serde_json::Value, ExecutionError> {
        let prompt = self.executor.render_template(&config.prompt, &self.state)?;
        Err(ExecutionError::WaitingForHuman {
            prompt,
            input_type: config.input_type.clone(),
            options: config.options.clone(),
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn execute_parallel(
        &mut self,
        node_id: &str,
        config: &ParallelNodeConfig,
        sink: &mut EventSink<'_>,
    ) -> Result<serde_json::Value, ExecutionError> {
        if config.branches.is_empty() {
            return Err(ExecutionError::JoinError("Parallel node has no branches".to_string()));
        }

        let mut branch_defs = Vec::new();
        for (idx, branch_id) in config.branches.iter().enumerate() {
            let node = self
                .graph
                .get_node(branch_id)
                .ok_or_else(|| ExecutionError::NodeNotFound(branch_id.clone()))?;
            let node_type = Self::node_type_name(&node.config);

            sink.emit(WorkflowEvent::NodeStarted {
                node_id: branch_id.clone(),
                node_name: node.name.clone(),
                node_type: node_type.to_string(),
            });

            branch_defs.push(BranchDef {
                index: idx,
                node_id: branch_id.clone(),
                config: node.config.clone(),
            });
        }

        let limit = config.concurrency_limit.unwrap_or(config.branches.len()).max(1);
        let semaphore = Arc::new(Semaphore::new(limit));

        let state_snapshot = self.state.clone();
        let graph = self.graph.clone();
        let executor = self.executor.clone();
        let store = self.workflow_store.clone();
        let exec_config = self.config.clone();
        let depth = self.subworkflow_depth;

        let mut tasks = FuturesUnordered::new();
        for branch in branch_defs {
            let semaphore = semaphore.clone();
            let graph = graph.clone();
            let executor = executor.clone();
            let store = store.clone();
            let exec_config = exec_config.clone();
            let state = state_snapshot.clone();
            tasks.push(async move {
                let start = Instant::now();

                let result = match semaphore
                    .acquire_owned()
                    .await
                    .map_err(|_| ExecutionError::Other("Parallel semaphore closed".to_string()))
                {
                    Ok(_permit) => {
                        execute_node_detached(
                            &graph,
                            &executor,
                            store.as_ref(),
                            &exec_config,
                            depth,
                            &branch.node_id,
                            &branch.config,
                            &state,
                        )
                        .await
                    }
                    Err(err) => Err(err),
                };

                BranchResult {
                    index: branch.index,
                    node_id: branch.node_id,
                    result,
                    duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
                }
            });
        }

        let mut results = Vec::new();
        while let Some(result) = tasks.next().await {
            results.push(result);
        }

        results.sort_by_key(|r| r.index);

        let mut successes = 0usize;
        let mut failures = Vec::new();
        let mut outputs = serde_json::Map::new();
        let mut errors = serde_json::Map::new();
        let mut waiting_for_human = false;

        for result in results {
            match result.result {
                Ok(outcome) => {
                    successes += 1;
                    outputs.insert(result.node_id.clone(), outcome.output.clone());

                    // Store branch output in parent state
                    self.state.set(format!("{}_output", result.node_id), outcome.output.clone());
                    for (key, value) in outcome.mapped_outputs {
                        self.state.set(key, value);
                    }

                    let mut exec = NodeExecution::new(&result.node_id);
                    exec.complete(outcome.output.clone());
                    self.state.push_execution(exec);

                    sink.emit(WorkflowEvent::NodeCompleted {
                        node_id: result.node_id.clone(),
                        output: outcome.output,
                        duration_ms: result.duration_ms,
                    });
                }
                Err(err) => {
                    if matches!(err, ExecutionError::WaitingForHuman { .. }) {
                        waiting_for_human = true;
                    }
                    failures.push(result.node_id.clone());
                    errors
                        .insert(result.node_id.clone(), serde_json::Value::String(err.to_string()));

                    let mut exec = NodeExecution::new(&result.node_id);
                    exec.fail(err.to_string());
                    self.state.push_execution(exec);

                    sink.emit(WorkflowEvent::NodeFailed {
                        node_id: result.node_id.clone(),
                        error: err.to_string(),
                    });
                }
            }
        }

        let join_success = match config.join_strategy {
            crate::node::JoinStrategy::All => successes == config.branches.len(),
            crate::node::JoinStrategy::Any => successes >= 1,
            crate::node::JoinStrategy::Count(count) => successes >= count,
        };

        if waiting_for_human {
            return Err(ExecutionError::Other(
                "Parallel branches cannot wait for human input".to_string(),
            ));
        }

        if matches!(config.failure_policy, ParallelFailurePolicy::FailFast) && !errors.is_empty() {
            return Err(ExecutionError::JoinError(format!(
                "Parallel node {} failed: {} errors",
                node_id,
                errors.len()
            )));
        }

        if !join_success {
            return Err(ExecutionError::JoinError(format!(
                "Parallel node {node_id} did not meet join strategy"
            )));
        }

        Ok(serde_json::json!({
            "status": "completed",
            "branches": outputs,
            "errors": errors,
            "successes": successes,
            "failures": failures,
        }))
    }

    async fn execute_subworkflow(
        &mut self,
        node_id: &str,
        config: &SubWorkflowNodeConfig,
    ) -> Result<serde_json::Value, ExecutionError> {
        let store = self.workflow_store.as_ref().ok_or_else(|| {
            ExecutionError::Other(format!("Workflow store not configured (node {node_id})"))
        })?;

        let max_depth = config.max_depth.unwrap_or(self.config.max_subworkflow_depth);
        if self.subworkflow_depth >= max_depth {
            return Err(ExecutionError::Other(format!(
                "Subworkflow depth exceeded at node {node_id} (max {max_depth})"
            )));
        }

        let def = store
            .load(&config.workflow_id)
            .await
            .map_err(|e| ExecutionError::Other(format!("Failed to load workflow: {e}")))?
            .ok_or_else(|| {
                ExecutionError::Other(format!("Subworkflow not found: {}", config.workflow_id))
            })?;

        let graph = Graph::try_from(def)?;
        let input = build_subworkflow_input(&self.state, config);

        let mut sub_executor = self.spawn_subworkflow_executor(graph);
        let _events = Box::pin(sub_executor.run(input)).await;

        if sub_executor.state().is_waiting_for_human() {
            return Err(ExecutionError::Other(
                "Subworkflow is waiting for human input".to_string(),
            ));
        }
        if sub_executor.state().is_failed() {
            let detail = match &sub_executor.state().status {
                WorkflowStatus::Failed { error, node } => {
                    format!("{error} (node: {node})")
                }
                _ => "unknown error".to_string(),
            };
            return Err(ExecutionError::Other(format!(
                "Subworkflow {} failed: {}",
                config.workflow_id, detail
            )));
        }
        if sub_executor.state().is_cancelled() {
            return Err(ExecutionError::Other(format!(
                "Subworkflow {} cancelled",
                config.workflow_id
            )));
        }

        // Apply output mapping to parent state
        for (from, to) in &config.output_mapping {
            if let Some(value) = sub_executor.state().get(from) {
                self.state.set(to, value.clone());
            }
        }

        Ok(serde_json::json!({
            "workflow_id": config.workflow_id,
            "status": "completed",
            "result": sub_executor.state().data.clone(),
            "nodes_executed": sub_executor.state().executed_count(),
        }))
    }

    fn spawn_subworkflow_executor(&self, graph: Graph) -> Self {
        build_subworkflow_executor(
            graph,
            self.executor.clone(),
            self.config.clone(),
            self.workflow_store.clone(),
            self.subworkflow_depth + 1,
        )
    }

    const fn node_type_name(config: &NodeConfig) -> &'static str {
        match config {
            NodeConfig::Agent(_) => "agent",
            NodeConfig::Tool(_) => "tool",
            NodeConfig::Router(_) => "router",
            NodeConfig::Human(_) => "human",
            NodeConfig::Parallel(_) => "parallel",
            NodeConfig::SubWorkflow(_) => "sub_workflow",
        }
    }

    /// 路由到下一个节点
    fn route_to_next(&mut self, current_node: &str, sink: &mut EventSink<'_>) {
        let (next_node, explanation) = self.determine_next_node(current_node);

        match next_node {
            Some(next) => {
                let condition = explanation.as_ref().and_then(|exp| {
                    exp.candidates.get(exp.selected_index).and_then(|c| c.condition.clone())
                });

                sink.emit(WorkflowEvent::RouteDecision {
                    node_id: current_node.to_string(),
                    next_node: next.clone(),
                    condition,
                    explanation,
                });

                self.state.current_node = next;
            }
            None => {
                self.state.status = WorkflowStatus::Completed;
            }
        }
    }

    /// 确定下一个节点
    fn determine_next_node(
        &self,
        current_node: &str,
    ) -> (Option<String>, Option<RouteExplanation>) {
        let outgoing = self.graph.outgoing_edges(current_node);

        // 获取 Router 配置（如果有）
        let node = self.graph.get_node(current_node);
        let router_config = node.and_then(|n| match &n.config {
            NodeConfig::Router(cfg) => Some(cfg),
            _ => None,
        });
        let default_target = router_config.and_then(|cfg| cfg.default_target.clone());

        // 无出边时，尝试使用 default_target
        if outgoing.is_empty() {
            return default_target.map_or((None, None), |target| {
                let explanation = RouteExplanation {
                    expression: String::new(),
                    evaluated_values: HashMap::new(),
                    candidates: vec![RouteCandidate {
                        target_node: target.clone(),
                        condition: None,
                        evaluation_result: true,
                        is_default: true,
                    }],
                    selected_index: 0,
                };
                (Some(target), Some(explanation))
            });
        }

        // 单条直接边，无需解释
        if outgoing.len() == 1 && matches!(outgoing[0].edge_type, EdgeType::Direct) {
            return (Some(outgoing[0].target.clone()), None);
        }

        // 构建路由决策
        self.build_route_decision(current_node, &outgoing, router_config, default_target)
    }

    /// 构建路由决策
    #[allow(clippy::needless_pass_by_value)]
    fn build_route_decision(
        &self,
        current_node: &str,
        outgoing: &[&crate::graph::Edge],
        router_config: Option<&RouterNodeConfig>,
        default_target: Option<String>,
    ) -> (Option<String>, Option<RouteExplanation>) {
        let expression = router_config.map_or_else(String::new, |cfg| cfg.expression.clone());

        // 获取路由值
        let route_value =
            self.state.get(&format!("{current_node}_output")).and_then(|v| v.get("route")).cloned();

        // 构建候选分支
        let mut candidates = Vec::new();
        let mut selected_index = None;

        for (idx, edge) in outgoing.iter().enumerate() {
            let (condition, eval_result, is_default) = match &edge.edge_type {
                EdgeType::Direct => (None, true, true),
                EdgeType::Conditional { condition } => {
                    let result = route_value.as_ref().is_some_and(|rv| {
                        // 支持两种匹配方式：
                        // 1. route_value 匹配 condition
                        // 2. route_value 直接返回目标节点 ID
                        Self::matches_condition(rv, condition)
                            || Self::matches_condition(rv, &edge.target)
                    });
                    (Some(condition.clone()), result, false)
                }
            };

            candidates.push(RouteCandidate {
                target_node: edge.target.clone(),
                condition,
                evaluation_result: eval_result,
                is_default,
            });

            if selected_index.is_none() && eval_result && !is_default {
                selected_index = Some(idx);
            }
        }

        // 如果有 default_target，将其作为候选项添加
        if let Some(ref target) = default_target {
            candidates.push(RouteCandidate {
                target_node: target.clone(),
                condition: None,
                evaluation_result: true,
                is_default: true,
            });
        }

        // 确定最终选择
        let final_selection = Self::select_route(&candidates);

        final_selection.map_or((None, None), |idx| {
            let next_node = candidates[idx].target_node.clone();

            // 构建 evaluated_values，包含路由相关的状态值
            let mut evaluated_values = HashMap::new();
            if let Some(ref rv) = route_value {
                evaluated_values.insert("route".to_string(), rv.clone());
            }
            // 添加当前节点的完整输出
            let output_key = format!("{current_node}_output");
            if let Some(output) = self.state.get(&output_key) {
                evaluated_values.insert(output_key, output.clone());
            }

            let explanation =
                RouteExplanation { expression, evaluated_values, candidates, selected_index: idx };
            (Some(next_node), Some(explanation))
        })
    }

    /// 选择路由分支
    fn select_route(candidates: &[RouteCandidate]) -> Option<usize> {
        // 优先选择匹配的条件分支
        if let Some(idx) = candidates.iter().position(|c| c.evaluation_result && !c.is_default) {
            return Some(idx);
        }

        // 使用默认分支（Direct 边或 Router 配置的 default_target）
        if let Some(idx) = candidates.iter().position(|c| c.is_default) {
            return Some(idx);
        }

        None
    }

    /// 检查路由值是否匹配条件
    fn matches_condition(route_value: &serde_json::Value, condition: &str) -> bool {
        match route_value {
            serde_json::Value::String(s) => s == condition,
            serde_json::Value::Number(n) => n.to_string() == condition,
            serde_json::Value::Bool(b) => b.to_string() == condition,
            _ => false,
        }
    }

    /// 恢复等待人工输入的工作流
    pub async fn resume(&mut self, human_input: serde_json::Value) -> Vec<WorkflowEvent> {
        let mut events = Vec::new();
        let mut sink = EventSink::new(&mut events, None);
        self.resume_with_sink(human_input, &mut sink).await;
        events
    }

    /// 恢复等待人工输入的工作流（支持事件回调）
    pub async fn resume_with_callback<F>(
        &mut self,
        human_input: serde_json::Value,
        callback: &mut F,
    ) -> Vec<WorkflowEvent>
    where
        F: FnMut(&WorkflowEvent) + Send,
    {
        let mut events = Vec::new();
        let mut sink = EventSink::new(&mut events, Some(callback));
        self.resume_with_sink(human_input, &mut sink).await;
        events
    }

    async fn resume_with_sink(&mut self, human_input: serde_json::Value, sink: &mut EventSink<'_>) {
        // 验证状态
        let node_id = if let WorkflowStatus::WaitingForHuman { node, .. } = &self.state.status {
            node.clone()
        } else {
            sink.emit(WorkflowEvent::Failed {
                error: "Workflow is not waiting for human input".to_string(),
                failed_node: None,
            });
            return;
        };

        // 保存人工输入
        self.state.set("human_input", human_input.clone());
        self.state.set(
            format!("{node_id}_output"),
            serde_json::json!({
                "human_input": human_input.clone(),
            }),
        );

        // 更新执行记录
        if let Some(exec) = self.state.last_execution_mut() {
            exec.complete(serde_json::json!({"human_input": human_input.clone()}));
        }

        sink.emit(WorkflowEvent::HumanInputReceived {
            node_id: node_id.clone(),
            value: human_input,
        });

        // 更新状态
        self.state.status = WorkflowStatus::Running;

        // 路由到下一个节点
        self.route_to_next(&node_id, sink);

        // 继续执行
        if !self.is_finished() {
            self.execute_loop(Instant::now(), sink).await;
        }
    }
}
