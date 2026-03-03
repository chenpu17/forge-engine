//! 默认节点执行器实现
//!
//! 提供基于模板渲染和表达式求值的默认实现。

use async_trait::async_trait;

use crate::executor::{ExecutionError, NodeExecutor};
use crate::expression::ExpressionEvaluator;
use crate::node::{AgentNodeConfig, ToolNodeConfig};
use crate::state::WorkflowState;
use crate::template::TemplateRenderer;

/// 默认节点执行器
pub struct DefaultNodeExecutor {
    /// 模板渲染器
    template_renderer: TemplateRenderer,
    /// 表达式求值器
    expression_evaluator: ExpressionEvaluator,
}

impl Default for DefaultNodeExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl DefaultNodeExecutor {
    /// 创建新的执行器
    #[must_use]
    pub fn new() -> Self {
        Self {
            template_renderer: TemplateRenderer::new(),
            expression_evaluator: ExpressionEvaluator::new(),
        }
    }
}

#[async_trait]
impl NodeExecutor for DefaultNodeExecutor {
    async fn execute_agent(
        &self,
        _node_id: &str,
        config: &AgentNodeConfig,
        state: &WorkflowState,
    ) -> Result<serde_json::Value, ExecutionError> {
        let prompt = self.render_template(&config.prompt_template, state)?;
        Ok(serde_json::json!({
            "prompt": prompt,
            "agent_type": format!("{:?}", config.agent_type),
            "status": "mock_execution",
        }))
    }

    async fn execute_tool(
        &self,
        _node_id: &str,
        config: &ToolNodeConfig,
        _state: &WorkflowState,
    ) -> Result<serde_json::Value, ExecutionError> {
        Ok(serde_json::json!({
            "tool": config.tool_name,
            "params": config.params_template,
            "status": "mock_execution",
        }))
    }

    fn render_template(
        &self,
        template: &str,
        state: &WorkflowState,
    ) -> Result<String, ExecutionError> {
        self.template_renderer
            .render(template, state)
            .map_err(|e| ExecutionError::TemplateError(e.to_string()))
    }

    fn evaluate_expression(
        &self,
        expression: &str,
        state: &WorkflowState,
    ) -> Result<serde_json::Value, ExecutionError> {
        self.expression_evaluator
            .evaluate(expression, state)
            .map_err(|e| ExecutionError::ExpressionError(e.to_string()))
    }
}
