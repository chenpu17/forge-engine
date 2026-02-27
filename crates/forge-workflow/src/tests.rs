//! 单元测试

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod node_tests {
    use crate::node::*;

    #[test]
    fn test_agent_node_builder() {
        let node = Node::agent("test_agent")
            .agent_type(SubAgentType::Explore)
            .prompt("Test prompt: {{state.input}}")
            .model("claude-3-opus")
            .tools(vec!["Read".to_string(), "Write".to_string()])
            .build();

        assert_eq!(node.name, "test_agent");
        match &node.config {
            NodeConfig::Agent(config) => {
                assert_eq!(config.agent_type, SubAgentType::Explore);
                assert_eq!(config.prompt_template, "Test prompt: {{state.input}}");
                assert_eq!(config.model, Some("claude-3-opus".to_string()));
                assert_eq!(config.tools, Some(vec!["Read".to_string(), "Write".to_string()]));
            }
            _ => panic!("Expected Agent node config"),
        }
    }

    #[test]
    fn test_tool_node_builder() {
        let node = Node::tool("test_tool", "Bash")
            .params(serde_json::json!({"command": "ls -la"}))
            .require_confirmation()
            .build();

        assert_eq!(node.name, "test_tool");
        match &node.config {
            NodeConfig::Tool(config) => {
                assert_eq!(config.tool_name, "Bash");
                assert_eq!(config.params_template, serde_json::json!({"command": "ls -la"}));
                assert!(config.require_confirmation);
            }
            _ => panic!("Expected Tool node config"),
        }
    }

    #[test]
    fn test_router_node_builder() {
        let node =
            Node::router("test_router").expression("state.type").default_target("fallback").build();

        assert_eq!(node.name, "test_router");
        match &node.config {
            NodeConfig::Router(config) => {
                assert_eq!(config.expression, "state.type");
                assert_eq!(config.default_target, Some("fallback".to_string()));
            }
            _ => panic!("Expected Router node config"),
        }
    }

    #[test]
    fn test_human_node_builder() {
        let node = Node::human("test_human")
            .prompt("Please confirm:")
            .input_type(HumanInputType::Confirm)
            .build();

        assert_eq!(node.name, "test_human");
        match &node.config {
            NodeConfig::Human(config) => {
                assert_eq!(config.prompt, "Please confirm:");
                assert_eq!(config.input_type, HumanInputType::Confirm);
            }
            _ => panic!("Expected Human node config"),
        }
    }

    #[test]
    fn test_parallel_node_builder() {
        let node = Node::parallel("test_parallel")
            .branch("branch_a")
            .branch("branch_b")
            .join(JoinStrategy::Any)
            .build();

        assert_eq!(node.name, "test_parallel");
        match &node.config {
            NodeConfig::Parallel(config) => {
                assert_eq!(config.branches, vec!["branch_a", "branch_b"]);
                assert!(matches!(config.join_strategy, JoinStrategy::Any));
            }
            _ => panic!("Expected Parallel node config"),
        }
    }

    #[test]
    fn test_sub_workflow_node_builder() {
        let node = Node::sub_workflow("test_sub", "child_workflow")
            .map_input("parent_input", "child_input")
            .map_output("child_output", "parent_output")
            .build();

        assert_eq!(node.name, "test_sub");
        match &node.config {
            NodeConfig::SubWorkflow(config) => {
                assert_eq!(config.workflow_id, "child_workflow");
                assert_eq!(
                    config.input_mapping.get("parent_input"),
                    Some(&"child_input".to_string())
                );
                assert_eq!(
                    config.output_mapping.get("child_output"),
                    Some(&"parent_output".to_string())
                );
            }
            _ => panic!("Expected SubWorkflow node config"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod graph_tests {
    use crate::error::GraphError;
    use crate::graph::*;
    use crate::node::*;

    #[test]
    fn test_graph_creation() {
        let graph = Graph::new("test_workflow")
            .description("A test workflow")
            .version("1.0.0")
            .author("Test Author")
            .tag("test");

        assert_eq!(graph.name(), "test_workflow");
        assert_eq!(graph.metadata().description, Some("A test workflow".to_string()));
        assert_eq!(graph.metadata().version, Some("1.0.0".to_string()));
        assert_eq!(graph.metadata().author, Some("Test Author".to_string()));
        assert_eq!(graph.metadata().tags, vec!["test"]);
    }

    #[test]
    fn test_add_nodes() {
        let mut graph = Graph::new("test");

        graph
            .add_node("start", Node::tool("Start", "init").build())
            .add_node("process", Node::agent("Process").prompt("Do something").build())
            .add_node("end", Node::tool("End", "cleanup").build());

        assert_eq!(graph.node_count(), 3);
        assert!(graph.has_node("start"));
        assert!(graph.has_node("process"));
        assert!(graph.has_node("end"));

        // First node should be entry point
        assert_eq!(graph.entry_point(), Some("start"));
    }

    #[test]
    fn test_add_edges() {
        let mut graph = Graph::new("test");

        graph
            .add_node("a", Node::tool("A", "tool_a").build())
            .add_node("b", Node::tool("B", "tool_b").build())
            .add_node("c", Node::tool("C", "tool_c").build());

        graph.add_edge("a", "b").add_edge("b", "c");

        assert_eq!(graph.edge_count(), 2);
        assert!(graph.has_edge_between("a", "b"));
        assert!(graph.has_edge_between("b", "c"));
        assert!(!graph.has_edge_between("a", "c"));
    }

    #[test]
    fn test_conditional_edges() {
        let mut graph = Graph::new("test");

        graph
            .add_node("router", Node::router("Router").expression("state.type").build())
            .add_node("path_a", Node::tool("Path A", "tool_a").build())
            .add_node("path_b", Node::tool("Path B", "tool_b").build());

        // condition 是值标签，不是表达式
        // Router expression 求值后与 condition 做字符串匹配
        graph
            .add_conditional_edge("router", "path_a", "a")
            .add_conditional_edge("router", "path_b", "b");

        let outgoing = graph.outgoing_edges("router");
        assert_eq!(outgoing.len(), 2);
    }

    #[test]
    fn test_validate_valid_graph() {
        let mut graph = Graph::new("test");

        graph
            .add_node("start", Node::tool("Start", "init").build())
            .add_node("end", Node::tool("End", "cleanup").build());

        graph.add_edge("start", "end");

        assert!(graph.validate().is_ok());
    }

    #[test]
    fn test_validate_no_entry_point() {
        let graph = Graph::new("test");

        let result = graph.validate();
        assert!(matches!(result, Err(GraphError::NoEntryPoint)));
    }

    #[test]
    fn test_validate_orphan_node() {
        let mut graph = Graph::new("test");

        graph
            .add_node("start", Node::tool("Start", "init").build())
            .add_node("orphan", Node::tool("Orphan", "orphan").build());

        // No edges, orphan node has no connections
        let result = graph.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_node() {
        let mut graph = Graph::new("test");

        graph
            .add_node("a", Node::tool("A", "tool_a").build())
            .add_node("b", Node::tool("B", "tool_b").build())
            .add_node("c", Node::tool("C", "tool_c").build());

        graph.add_edge("a", "b").add_edge("b", "c");

        // Remove node b
        let removed = graph.remove_node("b");
        assert!(removed.is_ok());
        assert_eq!(graph.node_count(), 2);

        // Edges involving b should be removed
        assert!(!graph.has_edge_between("a", "b"));
        assert!(!graph.has_edge_between("b", "c"));
    }

    #[test]
    fn test_rename_node() {
        let mut graph = Graph::new("test");

        graph
            .add_node("old_name", Node::tool("Test", "tool").build())
            .add_node("other", Node::tool("Other", "other").build());

        graph.add_edge("old_name", "other");

        let result = graph.rename_node("old_name", "new_name");
        assert!(result.is_ok());

        assert!(!graph.has_node("old_name"));
        assert!(graph.has_node("new_name"));
        assert!(graph.has_edge_between("new_name", "other"));
    }

    #[test]
    fn test_end_nodes() {
        let mut graph = Graph::new("test");

        graph
            .add_node("start", Node::tool("Start", "init").build())
            .add_node("middle", Node::tool("Middle", "process").build())
            .add_node("end1", Node::tool("End1", "cleanup1").build())
            .add_node("end2", Node::tool("End2", "cleanup2").build());

        graph.add_edge("start", "middle").add_edge("middle", "end1").add_edge("middle", "end2");

        let end_nodes = graph.end_nodes();
        assert_eq!(end_nodes.len(), 2);
        assert!(end_nodes.contains(&"end1"));
        assert!(end_nodes.contains(&"end2"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod state_tests {
    use crate::state::*;

    #[test]
    fn test_state_creation() {
        let state = WorkflowState::new();

        assert!(matches!(state.status, WorkflowStatus::Pending));
        assert!(state.current_node.is_empty());
        assert!(state.data.is_empty());
        assert!(state.history.is_empty());
    }

    #[test]
    fn test_state_data_operations() {
        let mut state = WorkflowState::new();

        // Set values
        state.set("key1", serde_json::json!("value1"));
        state.set("key2", serde_json::json!(42));

        // Get values
        assert_eq!(state.get("key1"), Some(&serde_json::json!("value1")));
        assert_eq!(state.get("key2"), Some(&serde_json::json!(42)));
        assert_eq!(state.get("nonexistent"), None);

        // Contains
        assert!(state.contains("key1"));
        assert!(!state.contains("nonexistent"));

        // Remove
        let removed = state.remove("key1");
        assert_eq!(removed, Some(serde_json::json!("value1")));
        assert!(!state.contains("key1"));
    }

    #[test]
    fn test_workflow_status_checks() {
        let mut state = WorkflowState::new();

        // Pending
        assert!(!state.is_running());
        assert!(!state.is_completed());
        assert!(!state.is_failed());
        assert!(!state.is_finished());

        // Running
        state.status = WorkflowStatus::Running;
        assert!(state.is_running());
        assert!(!state.is_finished());

        // Completed
        state.status = WorkflowStatus::Completed;
        assert!(state.is_completed());
        assert!(state.is_finished());

        // Failed
        state.status = WorkflowStatus::Failed {
            error: "Test error".to_string(),
            node: "test_node".to_string(),
        };
        assert!(state.is_failed());
        assert!(state.is_finished());

        // Cancelled
        state.status = WorkflowStatus::Cancelled;
        assert!(state.is_cancelled());
        assert!(state.is_finished());

        // Waiting for human
        state.status = WorkflowStatus::WaitingForHuman {
            node: "human_node".to_string(),
            prompt: "Please confirm".to_string(),
        };
        assert!(state.is_waiting_for_human());
        assert!(!state.is_finished());
    }

    #[test]
    fn test_node_execution() {
        let mut execution = NodeExecution::new("test_node");

        assert_eq!(execution.node_id, "test_node");
        assert!(matches!(execution.status, ExecutionStatus::Running));
        assert!(execution.completed_at.is_none());

        // Complete
        execution.complete(serde_json::json!({"result": "success"}));
        assert!(matches!(execution.status, ExecutionStatus::Completed));
        assert!(execution.completed_at.is_some());
        assert_eq!(execution.output, Some(serde_json::json!({"result": "success"})));
    }

    #[test]
    fn test_node_execution_failure() {
        let mut execution = NodeExecution::new("test_node");

        execution.fail("Something went wrong");
        assert!(matches!(execution.status, ExecutionStatus::Failed));
        assert!(execution.completed_at.is_some());
        assert_eq!(execution.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_execution_history() {
        let mut state = WorkflowState::new();

        let mut exec1 = NodeExecution::new("node1");
        exec1.complete(serde_json::json!({}));
        state.push_execution(exec1);

        let mut exec2 = NodeExecution::new("node2");
        exec2.complete(serde_json::json!({}));
        state.push_execution(exec2);

        assert_eq!(state.executed_count(), 2);
        assert_eq!(state.last_execution().map(|e| e.node_id.as_str()), Some("node2"));

        let node1_execs = state.get_node_executions("node1");
        assert_eq!(node1_execs.len(), 1);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod executor_tests {
    use crate::definition::GraphDefinition;
    use crate::executor::*;
    use crate::graph::Graph;
    use crate::node::*;
    use crate::persistence::{FileWorkflowStore, WorkflowStore};
    use crate::state::WorkflowState;
    use async_trait::async_trait;
    use std::sync::Arc;

    /// 测试用的简单执行器
    struct MockExecutor;

    #[async_trait]
    impl NodeExecutor for MockExecutor {
        async fn execute_agent(
            &self,
            _node_id: &str,
            config: &AgentNodeConfig,
            _state: &WorkflowState,
        ) -> Result<serde_json::Value, ExecutionError> {
            Ok(serde_json::json!({
                "response": format!("Executed agent with prompt: {}", config.prompt_template),
            }))
        }

        async fn execute_tool(
            &self,
            _node_id: &str,
            config: &ToolNodeConfig,
            _state: &WorkflowState,
        ) -> Result<serde_json::Value, ExecutionError> {
            Ok(serde_json::json!({
                "success": true,
                "tool": config.tool_name,
            }))
        }

        fn render_template(
            &self,
            template: &str,
            _state: &WorkflowState,
        ) -> Result<String, ExecutionError> {
            Ok(template.to_string())
        }

        fn evaluate_expression(
            &self,
            expression: &str,
            state: &WorkflowState,
        ) -> Result<serde_json::Value, ExecutionError> {
            // 简单实现：从状态中获取值
            state.get(expression).map_or_else(
                || Ok(serde_json::json!(expression)),
                |value| Ok(value.clone()),
            )
        }
    }

    struct FailingExecutor {
        fail_tool: String,
    }

    #[async_trait]
    impl NodeExecutor for FailingExecutor {
        async fn execute_agent(
            &self,
            _node_id: &str,
            config: &AgentNodeConfig,
            _state: &WorkflowState,
        ) -> Result<serde_json::Value, ExecutionError> {
            Ok(serde_json::json!({
                "response": format!("Executed agent with prompt: {}", config.prompt_template),
            }))
        }

        async fn execute_tool(
            &self,
            _node_id: &str,
            config: &ToolNodeConfig,
            _state: &WorkflowState,
        ) -> Result<serde_json::Value, ExecutionError> {
            if config.tool_name == self.fail_tool {
                return Err(ExecutionError::ToolError("forced failure".to_string()));
            }
            Ok(serde_json::json!({
                "success": true,
                "tool": config.tool_name,
            }))
        }

        fn render_template(
            &self,
            template: &str,
            _state: &WorkflowState,
        ) -> Result<String, ExecutionError> {
            Ok(template.to_string())
        }

        fn evaluate_expression(
            &self,
            expression: &str,
            state: &WorkflowState,
        ) -> Result<serde_json::Value, ExecutionError> {
            state.get(expression).map_or_else(
                || Ok(serde_json::json!(expression)),
                |value| Ok(value.clone()),
            )
        }
    }

    #[tokio::test]
    async fn test_simple_workflow_execution() {
        let mut graph = Graph::new("test_workflow");

        graph
            .add_node("start", Node::tool("Start", "init").build())
            .add_node("end", Node::tool("End", "cleanup").build());

        graph.add_edge("start", "end");

        let executor = MockExecutor;
        let mut workflow = WorkflowExecutor::new(graph, executor);

        let events = workflow.run(serde_json::json!({"test": "input"})).await;

        // 验证事件序列
        assert!(events.iter().any(|e| matches!(e, WorkflowEvent::Started { .. })));
        assert!(events.iter().any(|e| matches!(e, WorkflowEvent::Completed { .. })));

        // 验证状态
        assert!(workflow.state().is_completed());
        assert_eq!(workflow.state().executed_count(), 2);
    }

    #[tokio::test]
    async fn test_workflow_with_agent_node() {
        let mut graph = Graph::new("agent_workflow");

        graph.add_node("agent", Node::agent("TestAgent").prompt("Process: {{input}}").build());

        let executor = MockExecutor;
        let mut workflow = WorkflowExecutor::new(graph, executor);

        let events = workflow.run(serde_json::json!({"input": "test"})).await;

        assert!(workflow.state().is_completed());
        assert!(events.iter().any(|e| matches!(e, WorkflowEvent::NodeCompleted { .. })));
    }

    #[tokio::test]
    async fn test_workflow_cancellation() {
        let mut graph = Graph::new("cancel_test");
        graph.add_node("node", Node::tool("Node", "tool").build());

        let executor = MockExecutor;
        let mut workflow = WorkflowExecutor::new(graph, executor);

        // 取消工作流
        workflow.cancel();

        let events = workflow.run(serde_json::json!({})).await;

        assert!(events.iter().any(|e| matches!(e, WorkflowEvent::Cancelled)));
    }

    #[tokio::test]
    async fn test_parallel_node_execution() {
        let mut graph = Graph::new("parallel_workflow");
        graph.add_node(
            "parallel",
            Node::parallel("Parallel").branch("branch_a").branch("branch_b").build(),
        );
        graph.add_node("branch_a", Node::tool("BranchA", "tool_a").build());
        graph.add_node("branch_b", Node::tool("BranchB", "tool_b").build());
        graph.add_node("end", Node::tool("End", "done").build());
        graph.add_edge("parallel", "end");

        let executor = MockExecutor;
        let mut workflow = WorkflowExecutor::new(graph, executor);

        let events = workflow.run(serde_json::json!({})).await;

        assert!(workflow.state().is_completed());
        assert!(workflow.state().get("branch_a_output").is_some());
        assert!(workflow.state().get("branch_b_output").is_some());
        assert!(events.iter().any(|e| matches!(e, WorkflowEvent::NodeCompleted { .. })));
    }

    #[tokio::test]
    async fn test_parallel_collect_errors_any_strategy() {
        let mut graph = Graph::new("parallel_collect_errors");
        graph.add_node(
            "parallel",
            Node::parallel("Parallel")
                .branch("ok_branch")
                .branch("fail_branch")
                .join(JoinStrategy::Any)
                .failure_policy(ParallelFailurePolicy::CollectErrors)
                .build(),
        );
        graph.add_node("ok_branch", Node::tool("Ok", "ok_tool").build());
        graph.add_node("fail_branch", Node::tool("Fail", "fail_tool").build());

        let executor = FailingExecutor { fail_tool: "fail_tool".to_string() };
        let mut workflow = WorkflowExecutor::new(graph, executor);

        let events = workflow.run(serde_json::json!({})).await;

        assert!(workflow.state().is_completed());
        assert!(events.iter().any(|e| matches!(e, WorkflowEvent::Completed { .. })));
    }

    #[tokio::test]
    async fn test_parallel_failfast() {
        let mut graph = Graph::new("parallel_failfast");
        graph.add_node(
            "parallel",
            Node::parallel("Parallel").branch("ok_branch").branch("fail_branch").build(),
        );
        graph.add_node("ok_branch", Node::tool("Ok", "ok_tool").build());
        graph.add_node("fail_branch", Node::tool("Fail", "fail_tool").build());

        let executor = FailingExecutor { fail_tool: "fail_tool".to_string() };
        let mut workflow = WorkflowExecutor::new(graph, executor);

        let events = workflow.run(serde_json::json!({})).await;

        assert!(workflow.state().is_failed());
        assert!(events.iter().any(|e| matches!(e, WorkflowEvent::Failed { .. })));
    }

    #[tokio::test]
    async fn test_subworkflow_execution() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileWorkflowStore::new(temp_dir.path().join("workflows")));

        // Child workflow
        let mut child = Graph::with_id("child_workflow", "Child");
        child.add_node("child_node", Node::tool("ChildNode", "child_tool").build());

        let child_def: GraphDefinition = (&child).into();
        store.save(&child_def).await.unwrap();

        // Parent workflow with subworkflow node
        let mut parent = Graph::new("parent_workflow");
        parent.add_node(
            "sub",
            Node::sub_workflow("Sub", "child_workflow")
                .map_output("child_node_output", "child_result")
                .build(),
        );

        let executor = MockExecutor;
        let mut workflow = WorkflowExecutor::with_store(parent, executor, store);

        let events = workflow.run(serde_json::json!({})).await;

        assert!(workflow.state().is_completed());
        assert!(workflow.state().get("child_result").is_some());
        assert!(events.iter().any(|e| matches!(e, WorkflowEvent::Completed { .. })));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod template_tests {
    use crate::state::WorkflowState;
    use crate::template::TemplateRenderer;

    #[test]
    fn test_simple_template() {
        let renderer = TemplateRenderer::new();
        let mut state = WorkflowState::new();
        state.set("name", serde_json::json!("World"));

        let result = renderer.render("Hello, {{name}}!", &state).unwrap();
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_nested_path() {
        let renderer = TemplateRenderer::new();
        let mut state = WorkflowState::new();
        state.set("user", serde_json::json!({"name": "Alice", "age": 30}));

        let result = renderer.render("Name: {{user.name}}", &state).unwrap();
        assert_eq!(result, "Name: Alice");
    }

    #[test]
    fn test_missing_variable_non_strict() {
        let renderer = TemplateRenderer::new();
        let state = WorkflowState::new();

        let result = renderer.render("Hello, {{missing}}!", &state).unwrap();
        assert_eq!(result, "Hello, {{missing}}!");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod expression_tests {
    use crate::expression::ExpressionEvaluator;
    use crate::state::WorkflowState;

    #[test]
    fn test_literal_values() {
        let evaluator = ExpressionEvaluator::new();
        let state = WorkflowState::new();

        assert_eq!(evaluator.evaluate("true", &state).unwrap(), serde_json::json!(true));
        assert_eq!(evaluator.evaluate("42", &state).unwrap(), serde_json::json!(42));
        assert_eq!(evaluator.evaluate("'hello'", &state).unwrap(), serde_json::json!("hello"));
    }

    #[test]
    fn test_variable_resolution() {
        let evaluator = ExpressionEvaluator::new();
        let mut state = WorkflowState::new();
        state.set("count", serde_json::json!(10));

        let result = evaluator.evaluate("count", &state).unwrap();
        assert_eq!(result, serde_json::json!(10));
    }

    #[test]
    fn test_comparison() {
        let evaluator = ExpressionEvaluator::new();
        let mut state = WorkflowState::new();
        state.set("x", serde_json::json!(5));

        assert_eq!(evaluator.evaluate("x == 5", &state).unwrap(), serde_json::json!(true));
        assert_eq!(evaluator.evaluate("x > 3", &state).unwrap(), serde_json::json!(true));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod checkpoint_tests {
    use crate::checkpoint::{Checkpoint, CheckpointStore, MemoryCheckpointStore};
    use crate::state::WorkflowState;

    #[tokio::test]
    async fn test_checkpoint_save_load() {
        let store = MemoryCheckpointStore::new();
        let state = WorkflowState::new();
        let checkpoint = Checkpoint::new("workflow_1", state);
        let id = checkpoint.id.clone();

        store.save(&checkpoint).await.unwrap();

        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().workflow_id, "workflow_1");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod definition_tests {
    use crate::definition::{GraphChange, GraphChanges, GraphDefinition, GraphMetadataDefinition, NodeDefinition};
    use crate::graph::Graph;
    use crate::node::{Node, NodeConfig, ToolNodeConfig};
    use std::collections::HashMap;

    #[test]
    fn test_graph_to_definition() {
        let mut graph = Graph::new("test_workflow");
        graph
            .add_node("start", Node::tool("Start", "init").build())
            .add_node("end", Node::tool("End", "cleanup").build());
        graph.add_edge("start", "end");

        let def: GraphDefinition = (&graph).into();

        assert_eq!(def.name, "test_workflow");
        assert_eq!(def.nodes.len(), 2);
        assert_eq!(def.edges.len(), 1);
    }

    #[test]
    fn test_definition_to_graph() {
        let def = GraphDefinition {
            id: "test_id".to_string(),
            name: "test_workflow".to_string(),
            metadata: GraphMetadataDefinition::default(),
            nodes: vec![NodeDefinition {
                id: "node1".to_string(),
                name: "Node 1".to_string(),
                config: NodeConfig::Tool(ToolNodeConfig {
                    tool_name: "test_tool".to_string(),
                    params_template: serde_json::json!({}),
                    require_confirmation: false,
                    timeout_secs: None,
                }),
                position: None,
                metadata: HashMap::default(),
            }],
            edges: vec![],
            entry_point: Some("node1".to_string()),
        };

        let graph: Graph = def.try_into().unwrap();

        assert_eq!(graph.id(), "test_id");
        assert_eq!(graph.name(), "test_workflow");
        assert_eq!(graph.node_count(), 1);
    }

    #[test]
    fn test_graph_changes_apply() {
        let mut graph = Graph::new("test");
        graph.add_node("start", Node::tool("Start", "init").build());

        let mut changes = GraphChanges::new();
        changes.push(GraphChange::AddNode {
            node: NodeDefinition {
                id: "new_node".to_string(),
                name: "New Node".to_string(),
                config: NodeConfig::Tool(ToolNodeConfig {
                    tool_name: "new_tool".to_string(),
                    params_template: serde_json::json!({}),
                    require_confirmation: false,
                    timeout_secs: None,
                }),
                position: None,
                metadata: HashMap::default(),
            },
        });

        changes.apply(&mut graph).unwrap();

        assert_eq!(graph.node_count(), 2);
        assert!(graph.has_node("new_node"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod persistence_tests {
    use crate::checkpoint::{Checkpoint, CheckpointStore};
    use crate::definition::{GraphDefinition, GraphMetadataDefinition};
    use crate::persistence::{FileCheckpointStore, FileWorkflowStore, WorkflowStore};
    use crate::state::WorkflowState;

    #[tokio::test]
    async fn test_file_workflow_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileWorkflowStore::new(temp_dir.path());

        let def = GraphDefinition {
            id: "test_workflow".to_string(),
            name: "Test Workflow".to_string(),
            metadata: GraphMetadataDefinition::default(),
            nodes: vec![],
            edges: vec![],
            entry_point: None,
        };

        // Save
        store.save(&def).await.unwrap();

        // Exists
        assert!(store.exists("test_workflow").await.unwrap());

        // Load
        let loaded = store.load("test_workflow").await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().name, "Test Workflow");

        // List
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);

        // Delete
        store.delete("test_workflow").await.unwrap();
        assert!(!store.exists("test_workflow").await.unwrap());
    }

    #[tokio::test]
    async fn test_file_checkpoint_store() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileCheckpointStore::new(temp_dir.path());

        let state = WorkflowState::new();
        let checkpoint = Checkpoint::new("workflow_1", state);
        let id = checkpoint.id.clone();

        // Save
        store.save(&checkpoint).await.unwrap();

        // Load
        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().workflow_id, "workflow_1");

        // List
        let list = store.list("workflow_1").await.unwrap();
        assert_eq!(list.len(), 1);

        // Delete
        store.delete(&id).await.unwrap();
        let loaded = store.load(&id).await.unwrap();
        assert!(loaded.is_none());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod router_tests {
    use crate::executor::{WorkflowEvent, WorkflowExecutor};
    use crate::graph::Graph;
    use crate::state::WorkflowState;
    use crate::ExecutionError;

    /// Mock executor that returns configurable route values
    struct MockRouterExecutor {
        route_value: Option<String>,
    }

    impl MockRouterExecutor {
        fn new(route_value: Option<&str>) -> Self {
            Self { route_value: route_value.map(std::string::ToString::to_string) }
        }
    }

    #[async_trait::async_trait]
    impl crate::executor::NodeExecutor for MockRouterExecutor {
        async fn execute_agent(
            &self,
            _node_id: &str,
            _config: &crate::node::AgentNodeConfig,
            _state: &WorkflowState,
        ) -> Result<serde_json::Value, ExecutionError> {
            Ok(serde_json::json!({"result": "ok"}))
        }

        async fn execute_tool(
            &self,
            _node_id: &str,
            _config: &crate::node::ToolNodeConfig,
            _state: &WorkflowState,
        ) -> Result<serde_json::Value, ExecutionError> {
            Ok(serde_json::json!({"result": "ok"}))
        }

        fn render_template(
            &self,
            template: &str,
            _state: &WorkflowState,
        ) -> Result<String, ExecutionError> {
            Ok(template.to_string())
        }

        fn evaluate_expression(
            &self,
            _expression: &str,
            _state: &WorkflowState,
        ) -> Result<serde_json::Value, ExecutionError> {
            // 返回配置的 route 值
            self.route_value.as_ref().map_or_else(
                || Ok(serde_json::Value::Null),
                |v| Ok(serde_json::json!(v)),
            )
        }
    }

    /// 从事件列表中查找指定节点的 `RouteDecision` 事件
    fn find_route_decision_for_node<'a>(
        events: &'a [WorkflowEvent],
        node_id: &str,
    ) -> Option<&'a str> {
        for event in events {
            if let WorkflowEvent::RouteDecision { node_id: event_node_id, next_node, .. } = event {
                if event_node_id == node_id {
                    return Some(next_node.as_str());
                }
            }
        }
        None
    }

    #[tokio::test]
    async fn test_router_default_target_fallback() {
        // 测试：当 route_value 不匹配任何 condition 时，使用 default_target
        let mut graph = Graph::new("test");

        let router = crate::node::Node::router("router")
            .expression("state.route")
            .default_target("fallback")
            .build();
        graph.add_node("router", router);

        let target1 = crate::node::Node::tool("target1", "tool1").build();
        let fallback = crate::node::Node::tool("fallback", "tool2").build();
        graph.add_node("target1", target1);
        graph.add_node("fallback", fallback);

        graph.add_conditional_edge("router", "target1", "option1");
        graph.set_entry("router");

        // route_value = None，不匹配 "option1"，应该走 default_target
        let executor = MockRouterExecutor::new(None);
        let mut workflow = WorkflowExecutor::new(graph, executor);
        let events = workflow.run(serde_json::json!({})).await;

        // 验证路由到了 fallback
        let next = find_route_decision_for_node(&events, "router");
        assert_eq!(
            next,
            Some("fallback"),
            "Should route to default_target when no condition matches"
        );
    }

    #[tokio::test]
    async fn test_router_condition_match() {
        // 测试：route_value 匹配 condition 时，路由到对应目标
        let mut graph = Graph::new("test");

        let router = crate::node::Node::router("router")
            .expression("state.route")
            .default_target("fallback")
            .build();
        graph.add_node("router", router);

        let target_a = crate::node::Node::tool("target_a", "tool_a").build();
        let target_b = crate::node::Node::tool("target_b", "tool_b").build();
        let fallback = crate::node::Node::tool("fallback", "tool_fallback").build();
        graph.add_node("target_a", target_a);
        graph.add_node("target_b", target_b);
        graph.add_node("fallback", fallback);

        // condition 是值标签，不是表达式
        graph.add_conditional_edge("router", "target_a", "a");
        graph.add_conditional_edge("router", "target_b", "b");
        graph.set_entry("router");

        // route_value = "a"，应该匹配 condition "a"，路由到 target_a
        let executor = MockRouterExecutor::new(Some("a"));
        let mut workflow = WorkflowExecutor::new(graph, executor);
        let events = workflow.run(serde_json::json!({})).await;

        let next = find_route_decision_for_node(&events, "router");
        assert_eq!(
            next,
            Some("target_a"),
            "Should route to target_a when route_value matches condition 'a'"
        );
    }

    #[tokio::test]
    async fn test_router_condition_match_b() {
        // 测试：route_value = "b" 时路由到 target_b
        let mut graph = Graph::new("test");

        let router = crate::node::Node::router("router").expression("state.route").build();
        graph.add_node("router", router);

        let target_a = crate::node::Node::tool("target_a", "tool_a").build();
        let target_b = crate::node::Node::tool("target_b", "tool_b").build();
        graph.add_node("target_a", target_a);
        graph.add_node("target_b", target_b);

        graph.add_conditional_edge("router", "target_a", "a");
        graph.add_conditional_edge("router", "target_b", "b");
        graph.set_entry("router");

        // route_value = "b"
        let executor = MockRouterExecutor::new(Some("b"));
        let mut workflow = WorkflowExecutor::new(graph, executor);
        let events = workflow.run(serde_json::json!({})).await;

        let next = find_route_decision_for_node(&events, "router");
        assert_eq!(
            next,
            Some("target_b"),
            "Should route to target_b when route_value matches condition 'b'"
        );
    }

    #[tokio::test]
    async fn test_router_direct_edge_fallback() {
        // 测试：当没有条件边匹配且没有 default_target 时，使用 Direct 边
        let mut graph = Graph::new("test");

        let router = crate::node::Node::router("router").expression("state.route").build();
        graph.add_node("router", router);

        let target_a = crate::node::Node::tool("target_a", "tool_a").build();
        let direct_target = crate::node::Node::tool("direct_target", "tool_direct").build();
        graph.add_node("target_a", target_a);
        graph.add_node("direct_target", direct_target);

        graph.add_conditional_edge("router", "target_a", "a");
        graph.add_edge("router", "direct_target"); // Direct edge
        graph.set_entry("router");

        // route_value = "no_match"，不匹配 "a"，应该走 Direct 边
        let executor = MockRouterExecutor::new(Some("no_match"));
        let mut workflow = WorkflowExecutor::new(graph, executor);
        let events = workflow.run(serde_json::json!({})).await;

        let next = find_route_decision_for_node(&events, "router");
        assert_eq!(
            next,
            Some("direct_target"),
            "Should route to direct edge when no condition matches"
        );
    }
}
