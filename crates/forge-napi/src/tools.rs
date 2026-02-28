//! Custom tool registration for NAPI

use napi_derive::napi;

/// Tool information for settings UI
#[napi(object)]
#[derive(Clone, Debug)]
pub struct JsToolInfo {
    pub name: String,
    pub description: String,
    pub builtin: bool,
    pub disabled: bool,
    pub category: String,
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

/// Tool execution result from JavaScript
#[napi(object)]
#[derive(Clone, Default)]
pub struct JsToolResult {
    pub output: String,
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

/// Tool execution context passed to JavaScript
#[napi(object)]
#[derive(Clone)]
pub struct JsToolContext {
    pub working_dir: String,
    pub timeout_secs: u32,
}

impl From<&forge_tools::ToolContext> for JsToolContext {
    fn from(ctx: &forge_tools::ToolContext) -> Self {
        Self {
            working_dir: ctx.working_dir.to_string_lossy().to_string(),
            timeout_secs: ctx.timeout_secs as u32,
        }
    }
}

/// Arguments for the execute callback
#[derive(Clone)]
pub struct JsToolExecuteArgs {
    pub params: String,
    pub ctx: JsToolContext,
}

/// Custom tool definition from JavaScript
#[napi(object)]
pub struct JsToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters_schema: String,
}
