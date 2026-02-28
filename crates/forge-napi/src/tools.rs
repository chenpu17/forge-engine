//! Custom tool registration for NAPI

use napi_derive::napi;

/// Tool information for settings UI.
#[napi(object)]
#[derive(Clone, Debug)]
pub struct JsToolInfo {
    /// Tool name (unique identifier).
    pub name: String,
    /// Tool description.
    pub description: String,
    /// Whether this is a built-in tool.
    pub builtin: bool,
    /// Whether this tool is currently disabled.
    pub disabled: bool,
    /// Tool category (e.g. "file_system", "shell").
    pub category: String,
    /// Whether this tool requires network access.
    pub requires_network: bool,
}

impl From<forge_sdk::ToolInfo> for JsToolInfo {
    fn from(info: forge_sdk::ToolInfo) -> Self {
        let category = match info.category {
            forge_sdk::ToolCategory::FileSystem => "file_system",
            forge_sdk::ToolCategory::Shell => "shell",
            forge_sdk::ToolCategory::Search => "search",
            forge_sdk::ToolCategory::Task => "task",
            forge_sdk::ToolCategory::Interactive => "interactive",
            forge_sdk::ToolCategory::Planning => "planning",
            forge_sdk::ToolCategory::Mcp => "mcp",
            forge_sdk::ToolCategory::Other => "other",
        };
        Self {
            name: info.name,
            description: info.description,
            builtin: info.builtin,
            disabled: info.disabled,
            category: category.to_string(),
            requires_network: info.requires_network,
        }
    }
}

/// Tool execution result from JavaScript.
#[napi(object)]
#[derive(Clone, Default)]
pub struct JsToolResult {
    /// Output text from tool execution.
    pub output: String,
    /// Whether the result is an error.
    pub is_error: Option<bool>,
}

impl From<JsToolResult> for forge_tools::ToolOutput {
    fn from(result: JsToolResult) -> Self {
        if result.is_error.unwrap_or(false) {
            forge_tools::ToolOutput::error(result.output)
        } else {
            forge_tools::ToolOutput::success(result.output)
        }
    }
}

/// Tool execution context passed to JavaScript.
#[napi(object)]
#[derive(Clone)]
pub struct JsToolContext {
    /// Working directory path.
    pub working_dir: String,
    /// Timeout in seconds.
    pub timeout_secs: u32,
}

impl From<&forge_tools::ToolContext> for JsToolContext {
    fn from(ctx: &forge_tools::ToolContext) -> Self {
        Self {
            working_dir: ctx.working_dir.to_string_lossy().to_string(),
            timeout_secs: u32::try_from(ctx.timeout_secs).unwrap_or(u32::MAX),
        }
    }
}

/// Arguments for the execute callback.
#[derive(Clone)]
pub struct JsToolExecuteArgs {
    /// Tool parameters (JSON string).
    pub params: String,
    /// Execution context.
    pub ctx: JsToolContext,
}

/// Custom tool definition from JavaScript.
#[napi(object)]
pub struct JsToolDefinition {
    /// Tool name.
    pub name: String,
    /// Tool description.
    pub description: String,
    /// JSON Schema for tool parameters.
    pub parameters_schema: String,
}
