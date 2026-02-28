//! Keychain stub for MCP API key and proxy password management.
//!
//! These are no-op stubs. The full keychain integration will be added
//! when the platform-specific keychain backend is migrated.

/// Get the API key for an MCP server from keychain.
pub fn get_mcp_api_key(_server_name: &str) -> Result<Option<String>, String> {
    Ok(None)
}

/// Store the API key for an MCP server in keychain.
pub fn set_mcp_api_key(_server_name: &str, _api_key: &str) -> Result<(), String> {
    Ok(())
}

/// Delete the API key for an MCP server from keychain.
pub fn delete_mcp_api_key(_server_name: &str) -> Result<(), String> {
    Ok(())
}

/// Get the proxy password for an MCP server from keychain.
pub fn get_mcp_proxy_password(_server_name: &str) -> Result<Option<String>, String> {
    Ok(None)
}

/// Store the proxy password for an MCP server in keychain.
pub fn set_mcp_proxy_password(_server_name: &str, _password: &str) -> Result<(), String> {
    Ok(())
}

/// Delete the proxy password for an MCP server from keychain.
pub fn delete_mcp_proxy_password(_server_name: &str) -> Result<(), String> {
    Ok(())
}

/// Store the global proxy password in keychain.
pub fn set_global_proxy_password(_password: &str) -> Result<(), String> {
    Ok(())
}
