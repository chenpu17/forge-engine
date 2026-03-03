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
    /// Named proxy for this tool (None = direct connection).
    pub proxy_name: Option<String>,
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
            proxy_name: None,
        }
    }
}

/// Proxy authentication configuration.
#[napi(object)]
#[derive(Clone, Debug)]
pub struct JsProxyAuthConfig {
    /// Proxy username.
    pub username: Option<String>,
    /// Whether password is stored in keychain.
    pub password_from_keychain: bool,
}

/// Proxy configuration.
#[napi(object)]
#[derive(Clone, Debug)]
pub struct JsProxyConfig {
    /// Proxy mode: "none" | "system" | "environment" | "manual".
    pub mode: String,
    /// HTTP proxy URL.
    pub http_url: Option<String>,
    /// HTTPS proxy URL.
    pub https_url: Option<String>,
    /// Proxy authentication.
    pub auth: Option<JsProxyAuthConfig>,
    /// Addresses that bypass the proxy.
    pub no_proxy: Vec<String>,
    /// Disable TLS certificate validation (insecure).
    pub danger_accept_invalid_certs: bool,
}

impl From<forge_config::ProxyConfig> for JsProxyConfig {
    fn from(config: forge_config::ProxyConfig) -> Self {
        Self {
            mode: match config.mode {
                forge_config::ProxyMode::None => "none",
                forge_config::ProxyMode::System => "system",
                forge_config::ProxyMode::Environment => "environment",
                forge_config::ProxyMode::Manual => "manual",
            }
            .to_string(),
            http_url: config.http_url,
            https_url: config.https_url,
            auth: config.auth.map(|a| JsProxyAuthConfig {
                username: Some(a.username),
                password_from_keychain: a.password_from_keychain,
            }),
            no_proxy: config.no_proxy,
            danger_accept_invalid_certs: config.danger_accept_invalid_certs,
        }
    }
}

impl From<JsProxyConfig> for forge_config::ProxyConfig {
    fn from(config: JsProxyConfig) -> Self {
        let mode = match config.mode.to_lowercase().as_str() {
            "system" => forge_config::ProxyMode::System,
            "environment" | "env" => forge_config::ProxyMode::Environment,
            "manual" => forge_config::ProxyMode::Manual,
            _ => forge_config::ProxyMode::None,
        };
        Self {
            mode,
            http_url: config.http_url,
            https_url: config.https_url,
            auth: config.auth.and_then(|a| {
                a.username.map(|username| forge_config::ProxyAuth {
                    username,
                    password: None,
                    password_env: None,
                    password_from_keychain: a.password_from_keychain,
                    password_keychain_key: None,
                })
            }),
            no_proxy: config.no_proxy,
            danger_accept_invalid_certs: config.danger_accept_invalid_certs,
        }
    }
}

/// Named proxy entry.
#[napi(object)]
#[derive(Clone, Debug)]
pub struct JsProxyInfo {
    /// Proxy name.
    pub name: String,
    /// Proxy configuration.
    pub config: JsProxyConfig,
}

impl From<forge_sdk::ProxyInfo> for JsProxyInfo {
    fn from(info: forge_sdk::ProxyInfo) -> Self {
        Self { name: info.name, config: info.config.into() }
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
