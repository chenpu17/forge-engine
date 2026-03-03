//! 节点类型定义

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 节点位置（用于 UI 布局）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    /// X 坐标
    pub x: f64,
    /// Y 坐标
    pub y: f64,
}

/// 工作流节点
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
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

/// 节点配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeConfig {
    /// Agent 节点
    Agent(AgentNodeConfig),
    /// Tool 节点
    Tool(ToolNodeConfig),
    /// Router 节点
    Router(RouterNodeConfig),
    /// Human 节点
    Human(HumanNodeConfig),
    /// Parallel 节点
    Parallel(ParallelNodeConfig),
    /// `SubWorkflow` 节点
    SubWorkflow(SubWorkflowNodeConfig),
}

// ============================================================================
// Agent 节点
// ============================================================================

/// Agent 节点配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentNodeConfig {
    /// Agent 类型
    #[serde(default)]
    pub agent_type: SubAgentType,
    /// Prompt 模板
    pub prompt_template: String,
    /// 模型（可选，覆盖默认）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 可用工具列表
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    /// 最大迭代次数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,
    /// 超时时间（秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// `SubAgent` 类型
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentType {
    /// 探索型
    Explore,
    /// 规划型
    Plan,
    /// 研究型
    Research,
    /// 通用型
    #[default]
    GeneralPurpose,
    /// 内容创作型
    Writer,
    /// 数据分析型
    DataAnalyst,
}

// ============================================================================
// Tool 节点
// ============================================================================

/// Tool 节点配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolNodeConfig {
    /// 工具名称
    pub tool_name: String,
    /// 参数模板
    #[serde(default)]
    pub params_template: serde_json::Value,
    /// 是否需要确认
    #[serde(default)]
    pub require_confirmation: bool,
    /// 超时时间（秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

// ============================================================================
// Router 节点
// ============================================================================

/// Router 节点配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterNodeConfig {
    /// 路由表达式
    pub expression: String,
    /// 默认目标节点
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_target: Option<String>,
}

// ============================================================================
// Human 节点
// ============================================================================

/// Human 节点配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanNodeConfig {
    /// 提示信息
    pub prompt: String,
    /// 输入类型
    #[serde(default)]
    pub input_type: HumanInputType,
    /// 选项列表（用于 `select/multi_select`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<HumanInputOption>>,
    /// 超时时间（秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// 默认值
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<serde_json::Value>,
}

/// 人工输入类型
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanInputType {
    /// 文本输入
    #[default]
    Text,
    /// 确认（是/否）
    Confirm,
    /// 单选
    Select,
    /// 多选
    MultiSelect,
}

/// 人工输入选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanInputOption {
    /// 选项值
    pub value: String,
    /// 显示标签
    pub label: String,
    /// 描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ============================================================================
// Parallel 节点
// ============================================================================

/// Parallel 节点配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelNodeConfig {
    /// 并行分支节点 ID 列表
    pub branches: Vec<String>,
    /// 合并策略
    #[serde(default)]
    pub join_strategy: JoinStrategy,
    /// 失败策略
    #[serde(default)]
    pub failure_policy: ParallelFailurePolicy,
    /// 并发限制（None 表示不限制）
    #[serde(default)]
    pub concurrency_limit: Option<usize>,
    /// 超时时间（秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// 并行失败策略
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ParallelFailurePolicy {
    /// 任一分支失败即失败
    #[default]
    FailFast,
    /// 收集错误，满足合并条件即可继续
    CollectErrors,
}

/// 合并策略
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinStrategy {
    /// 等待所有分支完成
    #[default]
    All,
    /// 任意一个完成即可
    Any,
    /// 指定数量完成
    Count(usize),
}

// ============================================================================
// SubWorkflow 节点
// ============================================================================

/// `SubWorkflow` 节点配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubWorkflowNodeConfig {
    /// 子工作流 ID
    pub workflow_id: String,
    /// 输入映射
    #[serde(default)]
    pub input_mapping: HashMap<String, String>,
    /// 输出映射
    #[serde(default)]
    pub output_mapping: HashMap<String, String>,
    /// 最大递归深度（可选，覆盖全局设置）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<usize>,
}

// ============================================================================
// Node 实现
// ============================================================================

impl Node {
    /// 从配置创建节点
    pub fn from_config(name: impl Into<String>, config: NodeConfig) -> Self {
        Self { name: name.into(), config, position: None, metadata: HashMap::new() }
    }

    /// 创建 Agent 节点
    pub fn agent(name: impl Into<String>) -> AgentNodeBuilder {
        AgentNodeBuilder::new(name)
    }

    /// 创建 Tool 节点
    pub fn tool(name: impl Into<String>, tool_name: impl Into<String>) -> ToolNodeBuilder {
        ToolNodeBuilder::new(name, tool_name)
    }

    /// 创建 Router 节点
    pub fn router(name: impl Into<String>) -> RouterNodeBuilder {
        RouterNodeBuilder::new(name)
    }

    /// 创建 Human 节点
    pub fn human(name: impl Into<String>) -> HumanNodeBuilder {
        HumanNodeBuilder::new(name)
    }

    /// 创建 Parallel 节点
    pub fn parallel(name: impl Into<String>) -> ParallelNodeBuilder {
        ParallelNodeBuilder::new(name)
    }

    /// 创建 `SubWorkflow` 节点
    pub fn sub_workflow(
        name: impl Into<String>,
        workflow_id: impl Into<String>,
    ) -> SubWorkflowNodeBuilder {
        SubWorkflowNodeBuilder::new(name, workflow_id)
    }
}

// ============================================================================
// Agent Node Builder
// ============================================================================

/// Agent 节点构建器
#[derive(Debug)]
pub struct AgentNodeBuilder {
    name: String,
    config: AgentNodeConfig,
    position: Option<Position>,
    metadata: HashMap<String, String>,
}

impl AgentNodeBuilder {
    /// 创建新的构建器
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            config: AgentNodeConfig {
                agent_type: SubAgentType::GeneralPurpose,
                prompt_template: String::new(),
                model: None,
                tools: None,
                max_iterations: None,
                timeout_secs: None,
            },
            position: None,
            metadata: HashMap::new(),
        }
    }

    /// 设置 Agent 类型
    #[must_use]
    pub const fn agent_type(mut self, agent_type: SubAgentType) -> Self {
        self.config.agent_type = agent_type;
        self
    }

    /// 设置 Prompt 模板
    #[must_use]
    pub fn prompt(mut self, template: impl Into<String>) -> Self {
        self.config.prompt_template = template.into();
        self
    }

    /// 设置模型
    #[must_use]
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.config.model = Some(model.into());
        self
    }

    /// 设置可用工具
    #[must_use]
    pub fn tools(mut self, tools: Vec<String>) -> Self {
        self.config.tools = Some(tools);
        self
    }

    /// 构建节点
    #[must_use]
    pub fn build(self) -> Node {
        Node {
            name: self.name,
            config: NodeConfig::Agent(self.config),
            position: self.position,
            metadata: self.metadata,
        }
    }
}

impl From<AgentNodeBuilder> for Node {
    fn from(builder: AgentNodeBuilder) -> Self {
        builder.build()
    }
}

// ============================================================================
// Tool Node Builder
// ============================================================================

/// Tool 节点构建器
#[derive(Debug)]
pub struct ToolNodeBuilder {
    name: String,
    config: ToolNodeConfig,
    position: Option<Position>,
    metadata: HashMap<String, String>,
}

impl ToolNodeBuilder {
    /// 创建新的构建器
    pub fn new(name: impl Into<String>, tool_name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            config: ToolNodeConfig {
                tool_name: tool_name.into(),
                params_template: serde_json::Value::Null,
                require_confirmation: false,
                timeout_secs: None,
            },
            position: None,
            metadata: HashMap::new(),
        }
    }

    /// 设置参数模板
    #[must_use]
    pub fn params(mut self, params: serde_json::Value) -> Self {
        self.config.params_template = params;
        self
    }

    /// 设置需要确认
    #[must_use]
    pub const fn require_confirmation(mut self) -> Self {
        self.config.require_confirmation = true;
        self
    }

    /// 构建节点
    #[must_use]
    pub fn build(self) -> Node {
        Node {
            name: self.name,
            config: NodeConfig::Tool(self.config),
            position: self.position,
            metadata: self.metadata,
        }
    }
}

impl From<ToolNodeBuilder> for Node {
    fn from(builder: ToolNodeBuilder) -> Self {
        builder.build()
    }
}

// ============================================================================
// Router Node Builder
// ============================================================================

/// Router 节点构建器
#[derive(Debug)]
pub struct RouterNodeBuilder {
    name: String,
    config: RouterNodeConfig,
    position: Option<Position>,
    metadata: HashMap<String, String>,
}

impl RouterNodeBuilder {
    /// 创建新的构建器
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            config: RouterNodeConfig { expression: String::new(), default_target: None },
            position: None,
            metadata: HashMap::new(),
        }
    }

    /// 设置路由表达式
    #[must_use]
    pub fn expression(mut self, expr: impl Into<String>) -> Self {
        self.config.expression = expr.into();
        self
    }

    /// 设置默认目标
    #[must_use]
    pub fn default_target(mut self, target: impl Into<String>) -> Self {
        self.config.default_target = Some(target.into());
        self
    }

    /// 构建节点
    #[must_use]
    pub fn build(self) -> Node {
        Node {
            name: self.name,
            config: NodeConfig::Router(self.config),
            position: self.position,
            metadata: self.metadata,
        }
    }
}

impl From<RouterNodeBuilder> for Node {
    fn from(builder: RouterNodeBuilder) -> Self {
        builder.build()
    }
}

// ============================================================================
// Human Node Builder
// ============================================================================

/// Human 节点构建器
#[derive(Debug)]
pub struct HumanNodeBuilder {
    name: String,
    config: HumanNodeConfig,
    position: Option<Position>,
    metadata: HashMap<String, String>,
}

impl HumanNodeBuilder {
    /// 创建新的构建器
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            config: HumanNodeConfig {
                prompt: String::new(),
                input_type: HumanInputType::Text,
                options: None,
                timeout_secs: None,
                default_value: None,
            },
            position: None,
            metadata: HashMap::new(),
        }
    }

    /// 设置提示信息
    #[must_use]
    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.config.prompt = prompt.into();
        self
    }

    /// 设置输入类型
    #[must_use]
    pub const fn input_type(mut self, input_type: HumanInputType) -> Self {
        self.config.input_type = input_type;
        self
    }

    /// 构建节点
    #[must_use]
    pub fn build(self) -> Node {
        Node {
            name: self.name,
            config: NodeConfig::Human(self.config),
            position: self.position,
            metadata: self.metadata,
        }
    }
}

impl From<HumanNodeBuilder> for Node {
    fn from(builder: HumanNodeBuilder) -> Self {
        builder.build()
    }
}

// ============================================================================
// Parallel Node Builder
// ============================================================================

/// Parallel 节点构建器
#[derive(Debug)]
pub struct ParallelNodeBuilder {
    name: String,
    config: ParallelNodeConfig,
    position: Option<Position>,
    metadata: HashMap<String, String>,
}

impl ParallelNodeBuilder {
    /// 创建新的构建器
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            config: ParallelNodeConfig {
                branches: Vec::new(),
                join_strategy: JoinStrategy::All,
                failure_policy: ParallelFailurePolicy::default(),
                concurrency_limit: None,
                timeout_secs: None,
            },
            position: None,
            metadata: HashMap::new(),
        }
    }

    /// 添加分支
    #[must_use]
    pub fn branch(mut self, node_id: impl Into<String>) -> Self {
        self.config.branches.push(node_id.into());
        self
    }

    /// 设置合并策略
    #[must_use]
    pub const fn join(mut self, strategy: JoinStrategy) -> Self {
        self.config.join_strategy = strategy;
        self
    }

    /// 设置失败策略
    #[must_use]
    pub const fn failure_policy(mut self, policy: ParallelFailurePolicy) -> Self {
        self.config.failure_policy = policy;
        self
    }

    /// 设置并发限制
    #[must_use]
    pub const fn concurrency_limit(mut self, limit: usize) -> Self {
        self.config.concurrency_limit = Some(limit);
        self
    }

    /// 构建节点
    #[must_use]
    pub fn build(self) -> Node {
        Node {
            name: self.name,
            config: NodeConfig::Parallel(self.config),
            position: self.position,
            metadata: self.metadata,
        }
    }
}

impl From<ParallelNodeBuilder> for Node {
    fn from(builder: ParallelNodeBuilder) -> Self {
        builder.build()
    }
}

// ============================================================================
// SubWorkflow Node Builder
// ============================================================================

/// `SubWorkflow` 节点构建器
#[derive(Debug)]
pub struct SubWorkflowNodeBuilder {
    name: String,
    config: SubWorkflowNodeConfig,
    position: Option<Position>,
    metadata: HashMap<String, String>,
}

impl SubWorkflowNodeBuilder {
    /// 创建新的构建器
    pub fn new(name: impl Into<String>, workflow_id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            config: SubWorkflowNodeConfig {
                workflow_id: workflow_id.into(),
                input_mapping: HashMap::new(),
                output_mapping: HashMap::new(),
                max_depth: None,
            },
            position: None,
            metadata: HashMap::new(),
        }
    }

    /// 添加输入映射
    #[must_use]
    pub fn map_input(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.config.input_mapping.insert(from.into(), to.into());
        self
    }

    /// 添加输出映射
    #[must_use]
    pub fn map_output(mut self, from: impl Into<String>, to: impl Into<String>) -> Self {
        self.config.output_mapping.insert(from.into(), to.into());
        self
    }

    /// 设置最大递归深度
    #[must_use]
    pub const fn max_depth(mut self, depth: usize) -> Self {
        self.config.max_depth = Some(depth);
        self
    }

    /// 构建节点
    #[must_use]
    pub fn build(self) -> Node {
        Node {
            name: self.name,
            config: NodeConfig::SubWorkflow(self.config),
            position: self.position,
            metadata: self.metadata,
        }
    }
}

impl From<SubWorkflowNodeBuilder> for Node {
    fn from(builder: SubWorkflowNodeBuilder) -> Self {
        builder.build()
    }
}
