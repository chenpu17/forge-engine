//! Keychain integration for MCP API key and proxy password management.
//!
//! Delegates to [`forge_infra::secret`] for platform-native credential storage.

/// Get the API key for an MCP server from keychain.
pub fn get_mcp_api_key(server_name: &str) -> Result<Option<String>, String> {
    forge_infra::secret::get_mcp_api_key(server_name).map_err(|e| e.to_string())
}

/// Store the API key for an MCP server in keychain.
pub fn set_mcp_api_key(server_name: &str, api_key: &str) -> Result<(), String> {
    forge_infra::secret::set_mcp_api_key(server_name, api_key).map_err(|e| e.to_string())
}

/// Delete the API key for an MCP server from keychain.
pub fn delete_mcp_api_key(server_name: &str) -> Result<(), String> {
    forge_infra::secret::delete_mcp_api_key(server_name).map_err(|e| e.to_string())
}

/// Get the proxy password for an MCP server from keychain.
pub fn get_mcp_proxy_password(server_name: &str) -> Result<Option<String>, String> {
    forge_infra::secret::get_mcp_proxy_password(server_name).map_err(|e| e.to_string())
}

/// Store the proxy password for an MCP server in keychain.
pub fn set_mcp_proxy_password(server_name: &str, password: &str) -> Result<(), String> {
    forge_infra::secret::set_mcp_proxy_password(server_name, password).map_err(|e| e.to_string())
}

/// Delete the proxy password for an MCP server from keychain.
pub fn delete_mcp_proxy_password(server_name: &str) -> Result<(), String> {
    forge_infra::secret::delete_mcp_proxy_password(server_name).map_err(|e| e.to_string())
}

/// Store the global proxy password in keychain.
pub fn set_global_proxy_password(password: &str) -> Result<(), String> {
    forge_infra::secret::set_global_proxy_password(password).map_err(|e| e.to_string())
}
