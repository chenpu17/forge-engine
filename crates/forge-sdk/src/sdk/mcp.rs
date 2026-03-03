//! MCP server management for ForgeSDK

use super::*;

impl ForgeSDK {
    fn collect_mcp_config_paths(
        explicit_path: Option<std::path::PathBuf>,
        working_dir: &std::path::Path,
    ) -> Vec<std::path::PathBuf> {
        let mut paths = Vec::new();

        if let Some(path) = explicit_path {
            if path.exists() {
                paths.push(path);
            }
            return paths;
        }

        let project_path = working_dir.join(".forge/mcp.toml");
        if project_path.exists() {
            paths.push(project_path);
        }

        let user_path = forge_infra::data_dir().join("mcp.toml");
        if user_path.exists() {
            paths.push(user_path);
        }

        paths
    }

    /// Load MCP tools from configuration.
    ///
    /// # Errors
    ///
    /// Returns error if loading fails.
    pub async fn load_mcp_tools(&self) -> Result<usize> {
        use forge_mcp::McpServerConfig;

        let config = self.config.read().await;
        if !config.tools.mcp.mcp_enabled {
            return Ok(0);
        }

        let config_paths = Self::collect_mcp_config_paths(
            config.tools.mcp.mcp_config_path.clone(),
            &config.working_dir,
        );

        if config_paths.is_empty() {
            return Ok(0);
        }

        let mut merged_servers: HashMap<String, McpServerConfig> = HashMap::new();
        let merged_proxy: Option<forge_config::ProxyConfig> = Some(config.tools.proxy.clone());

        for config_path in config_paths.iter().rev() {
            match McpConfig::load_from_file(config_path) {
                Ok(mcp_config) => {
                    // Note: mcp_config.proxy is forge_mcp::ProxyConfig, not forge_config::ProxyConfig.
                    // We use the global proxy from forge_config instead.
                    for server in mcp_config.servers {
                        merged_servers.insert(server.name.clone(), server);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to load MCP config from {:?}: {}", config_path, e);
                }
            }
        }

        let enabled_servers: Vec<_> = merged_servers
            .into_iter()
            .filter(|(_, server)| server.enabled)
            .map(|(_, server)| server)
            .collect();

        if enabled_servers.is_empty() {
            return Ok(0);
        }

        let mcp_security = McpSecurity::default();
        let mcp_proxy = merged_proxy.map(|p| {
            use forge_mcp::transport::ProxyMode as McpProxyMode;
            forge_mcp::ProxyConfig {
                mode: match p.mode {
                    forge_config::ProxyMode::None => McpProxyMode::None,
                    forge_config::ProxyMode::System => McpProxyMode::System,
                    forge_config::ProxyMode::Environment => McpProxyMode::Environment,
                    forge_config::ProxyMode::Manual => McpProxyMode::Manual,
                },
                http_url: p.http_url,
                https_url: p.https_url,
                no_proxy: if p.no_proxy.is_empty() { None } else { Some(p.no_proxy.join(",")) },
            }
        });
        let mut manager = McpManager::with_proxy(mcp_proxy);
        let mut server_infos: Vec<McpServerInfo> = Vec::new();

        for server in &enabled_servers {
            let transport = match server.transport {
                ToolsTransportType::Stdio => McpTransportType::Stdio,
                ToolsTransportType::Sse => McpTransportType::Sse,
                ToolsTransportType::StreamableHttp => McpTransportType::StreamableHttp,
            };

            let mut server_info = McpServerInfo {
                name: server.name.clone(),
                transport,
                command: server.command.clone(),
                args: server.args.clone(),
                url: server.url.clone(),
                status: McpServerStatus::Disconnected,
                error: None,
                tools: Vec::new(),
            };

            let origin_to_validate = if server.is_sse() {
                server.url.as_deref().unwrap_or_default()
            } else {
                &server.command
            };

            if let Err(e) = mcp_security.validate_origin(origin_to_validate) {
                tracing::warn!(server = %server.name, "MCP security: untrusted origin - {}", e);
            }

            match manager.connect(server).await {
                Ok(()) => {
                    server_info.status = McpServerStatus::Connected;
                }
                Err(e) => {
                    server_info.status = McpServerStatus::Error;
                    server_info.error = Some(e.clone());
                    tracing::warn!("Failed to connect to MCP server '{}': {}", server.name, e);
                }
            }

            server_infos.push(server_info);
        }

        let all_tools = manager.list_all_tools().await;

        for (server_name, tools) in &all_tools {
            if let Some(server_info) = server_infos.iter_mut().find(|s| &s.name == server_name) {
                server_info.tools = tools
                    .iter()
                    .map(|t| McpToolInfo {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        server_name: server_name.clone(),
                    })
                    .collect();
            }
        }

        *self.mcp_servers.write().await = server_infos;

        let tools = manager.create_tool_wrappers().await;
        let tool_count = tools.len();

        let registry = self.tool_registry.read().await;
        let mut all_tool_names: Vec<String> =
            registry.list_names().into_iter().map(|s| s.to_string()).collect();
        drop(registry);

        all_tool_names.extend(tools.iter().map(|t| t.name().to_string()));

        if let Err(e) = mcp_security.check_tool_combinations(&all_tool_names) {
            tracing::warn!("MCP security: dangerous tool combination - {}", e);
        }

        let mut registry = self.tool_registry.write().await;
        for tool in tools {
            registry.register(tool);
        }

        Ok(tool_count)
    }

    /// List MCP servers with merged runtime/config state.
    pub async fn list_mcp_servers(&self) -> McpStatus {
        let runtime_servers = self.mcp_servers.read().await.clone();
        let config_servers = self.read_mcp_config_servers().await;

        let mut merged: HashMap<String, McpServerInfo> = HashMap::new();

        for server in config_servers {
            merged.insert(server.name.clone(), server);
        }

        for server in runtime_servers {
            if let Some(existing) = merged.get_mut(&server.name) {
                existing.status = server.status;
                existing.tools = server.tools;
                existing.error = server.error;
            } else {
                merged.insert(server.name.clone(), server);
            }
        }

        let servers: Vec<McpServerInfo> = merged.into_values().collect();
        let total_tools: usize = servers.iter().map(|s| s.tools.len()).sum();
        let connected_count =
            servers.iter().filter(|s| s.status == McpServerStatus::Connected).count();

        McpStatus { servers, total_tools, connected_count }
    }

    async fn read_mcp_config_servers(&self) -> Vec<McpServerInfo> {
        let config = self.config.read().await;
        if !config.tools.mcp.mcp_enabled {
            return Vec::new();
        }

        let config_paths = Self::collect_mcp_config_paths(
            config.tools.mcp.mcp_config_path.clone(),
            &config.working_dir,
        );

        if config_paths.is_empty() {
            return Vec::new();
        }

        let mut merged_servers: HashMap<String, forge_mcp::McpServerConfig> = HashMap::new();

        for config_path in config_paths.iter().rev() {
            if let Ok(mcp_config) = McpConfig::load_from_file(config_path) {
                for server in mcp_config.servers {
                    merged_servers.insert(server.name.clone(), server);
                }
            }
        }

        merged_servers
            .into_values()
            .map(|server| {
                let transport = match server.transport {
                    ToolsTransportType::Stdio => McpTransportType::Stdio,
                    ToolsTransportType::Sse => McpTransportType::Sse,
                    ToolsTransportType::StreamableHttp => McpTransportType::StreamableHttp,
                };
                McpServerInfo {
                    name: server.name.clone(),
                    transport,
                    command: server.command.clone(),
                    args: server.args.clone(),
                    url: server.url.clone(),
                    status: if server.enabled {
                        McpServerStatus::Configured
                    } else {
                        McpServerStatus::Disconnected
                    },
                    error: None,
                    tools: Vec::new(),
                }
            })
            .collect()
    }

    /// Get a single MCP server configuration by name.
    pub async fn get_mcp_server(&self, name: &str) -> Option<crate::types::McpServerManageConfig> {
        use crate::types::{McpServerManageConfig, McpTransportType};

        let config = self.config.read().await;
        if !config.tools.mcp.mcp_enabled {
            return None;
        }

        let config_paths = Self::collect_mcp_config_paths(
            config.tools.mcp.mcp_config_path.clone(),
            &config.working_dir,
        );

        for config_path in config_paths.iter().rev() {
            if let Ok(mcp_config) = McpConfig::load_from_file(config_path) {
                for server in mcp_config.servers {
                    if server.name == name {
                        let transport = match server.transport {
                            ToolsTransportType::Stdio => McpTransportType::Stdio,
                            ToolsTransportType::Sse => McpTransportType::Sse,
                            ToolsTransportType::StreamableHttp => McpTransportType::StreamableHttp,
                        };
                        return Some(McpServerManageConfig {
                            name: server.name,
                            transport,
                            enabled: server.enabled,
                            command: server.command,
                            args: server.args,
                            env: server.env,
                            url: server.url,
                            api_key: server.api_key,
                            api_key_from_keychain: server.api_key_from_keychain,
                            api_key_auth: Some(
                                match server.api_key_auth {
                                    forge_mcp::ApiKeyAuth::Bearer => "bearer",
                                    forge_mcp::ApiKeyAuth::Header => "header",
                                }
                                .to_string(),
                            ),
                            api_key_header: server.api_key_header,
                            api_key_prefix: server.api_key_prefix,
                            proxy_name: server.proxy_name,
                        });
                    }
                }
            }
        }

        None
    }

    /// Add a new MCP server configuration.
    ///
    /// # Errors
    ///
    /// Returns error if server already exists or save fails.
    pub async fn add_mcp_server(
        &self,
        server_config: crate::types::McpServerManageConfig,
    ) -> Result<()> {
        use crate::types::McpTransportType;
        use forge_mcp::{McpServerConfig, McpTransportType as ToolsTransport};

        if server_config.name.is_empty() {
            return Err(ForgeError::ConfigError("Server name cannot be empty".to_string()));
        }
        if server_config.name.contains("__") {
            return Err(ForgeError::ConfigError("Server name cannot contain '__'".to_string()));
        }

        let config_path = forge_infra::config_dir().join("mcp.toml");

        let mut mcp_config = if config_path.exists() {
            McpConfig::load_from_file(&config_path).map_err(ForgeError::ConfigError)?
        } else {
            McpConfig::default()
        };

        if mcp_config.servers.iter().any(|s| s.name == server_config.name) {
            return Err(ForgeError::ConfigError(format!(
                "Server '{}' already exists",
                server_config.name
            )));
        }

        let transport = match server_config.transport {
            McpTransportType::Stdio => ToolsTransport::Stdio,
            McpTransportType::Sse => ToolsTransport::Sse,
            McpTransportType::StreamableHttp => ToolsTransport::StreamableHttp,
        };

        let api_key_auth = match server_config.api_key_auth.as_deref() {
            Some("header") => forge_mcp::ApiKeyAuth::Header,
            _ => forge_mcp::ApiKeyAuth::Bearer,
        };

        let new_server = McpServerConfig {
            name: server_config.name,
            transport,
            enabled: server_config.enabled,
            command: server_config.command,
            args: server_config.args,
            env: server_config.env,
            url: server_config.url,
            api_key: server_config.api_key,
            api_key_from_keychain: server_config.api_key_from_keychain,
            api_key_auth,
            api_key_header: server_config.api_key_header,
            api_key_prefix: server_config.api_key_prefix,
            proxy_name: server_config.proxy_name,
            oauth: None,
        };

        new_server.validate().map_err(ForgeError::ConfigError)?;
        mcp_config.servers.push(new_server);
        self.save_mcp_config(&config_path, &mcp_config)
    }

    /// Update an existing MCP server configuration.
    ///
    /// # Errors
    ///
    /// Returns error if server not found or save fails.
    pub async fn update_mcp_server(
        &self,
        name: &str,
        server_config: crate::types::McpServerManageConfig,
    ) -> Result<()> {
        use crate::types::McpTransportType;
        use forge_mcp::{McpServerConfig, McpTransportType as ToolsTransport};

        let config = self.config.read().await;
        let config_paths = Self::collect_mcp_config_paths(
            config.tools.mcp.mcp_config_path.clone(),
            &config.working_dir,
        );

        let mut found_path: Option<std::path::PathBuf> = None;
        let mut found_idx: Option<usize> = None;

        for config_path in config_paths.iter() {
            if let Ok(mcp_config) = McpConfig::load_from_file(config_path) {
                if let Some(idx) = mcp_config.servers.iter().position(|s| s.name == name) {
                    found_path = Some(config_path.clone());
                    found_idx = Some(idx);
                    break;
                }
            }
        }

        let config_path = found_path
            .ok_or_else(|| ForgeError::ConfigError(format!("Server '{name}' not found")))?;
        let server_idx = found_idx
            .ok_or_else(|| ForgeError::ConfigError(format!("Server '{name}' not found")))?;
        drop(config);

        let mut mcp_config =
            McpConfig::load_from_file(&config_path).map_err(ForgeError::ConfigError)?;

        if server_config.name != name {
            if server_config.name.contains("__") {
                return Err(ForgeError::ConfigError("Server name cannot contain '__'".to_string()));
            }
            if mcp_config.servers.iter().any(|s| s.name == server_config.name) {
                return Err(ForgeError::ConfigError(format!(
                    "Server '{}' already exists",
                    server_config.name
                )));
            }
        }

        let transport = match server_config.transport {
            McpTransportType::Stdio => ToolsTransport::Stdio,
            McpTransportType::Sse => ToolsTransport::Sse,
            McpTransportType::StreamableHttp => ToolsTransport::StreamableHttp,
        };

        let api_key_auth = match server_config.api_key_auth.as_deref() {
            Some("header") => forge_mcp::ApiKeyAuth::Header,
            _ => forge_mcp::ApiKeyAuth::Bearer,
        };
        let existing_oauth = mcp_config.servers.get(server_idx).and_then(|s| s.oauth.clone());

        let updated_server = McpServerConfig {
            name: server_config.name.clone(),
            transport,
            enabled: server_config.enabled,
            command: server_config.command,
            args: server_config.args,
            env: server_config.env,
            url: server_config.url,
            api_key: server_config.api_key,
            api_key_from_keychain: server_config.api_key_from_keychain,
            api_key_auth,
            api_key_header: server_config.api_key_header,
            api_key_prefix: server_config.api_key_prefix,
            proxy_name: server_config.proxy_name,
            oauth: existing_oauth,
        };

        updated_server.validate().map_err(ForgeError::ConfigError)?;
        mcp_config.servers[server_idx] = updated_server;
        self.save_mcp_config(&config_path, &mcp_config)?;

        if server_config.name != name {
            if let Ok(Some(api_key)) = crate::extensions::keychain::get_mcp_api_key(name) {
                let _ = crate::extensions::keychain::set_mcp_api_key(&server_config.name, &api_key);
                let _ = crate::extensions::keychain::delete_mcp_api_key(name);
            }
            if let Ok(Some(proxy_pwd)) = crate::extensions::keychain::get_mcp_proxy_password(name) {
                let _ = crate::extensions::keychain::set_mcp_proxy_password(
                    &server_config.name,
                    &proxy_pwd,
                );
                let _ = crate::extensions::keychain::delete_mcp_proxy_password(name);
            }
        }

        Ok(())
    }

    /// Remove an MCP server configuration.
    ///
    /// # Errors
    ///
    /// Returns error if server not found or save fails.
    pub async fn remove_mcp_server(&self, name: &str) -> Result<()> {
        let config = self.config.read().await;
        let config_paths = Self::collect_mcp_config_paths(
            config.tools.mcp.mcp_config_path.clone(),
            &config.working_dir,
        );

        let mut found_path: Option<std::path::PathBuf> = None;

        for config_path in config_paths.iter() {
            if let Ok(mcp_config) = McpConfig::load_from_file(config_path) {
                if mcp_config.servers.iter().any(|s| s.name == name) {
                    found_path = Some(config_path.clone());
                    break;
                }
            }
        }

        let config_path = found_path
            .ok_or_else(|| ForgeError::ConfigError(format!("Server '{name}' not found")))?;
        drop(config);

        let mut mcp_config =
            McpConfig::load_from_file(&config_path).map_err(ForgeError::ConfigError)?;

        let original_len = mcp_config.servers.len();
        mcp_config.servers.retain(|s| s.name != name);

        if mcp_config.servers.len() == original_len {
            return Err(ForgeError::ConfigError(format!("Server '{name}' not found")));
        }

        self.save_mcp_config(&config_path, &mcp_config)?;

        let _ = crate::extensions::keychain::delete_mcp_api_key(name);
        let _ = crate::extensions::keychain::delete_mcp_proxy_password(name);

        Ok(())
    }

    /// Set the API key for an MCP server in keychain.
    ///
    /// # Errors
    ///
    /// Returns error if keychain storage fails.
    pub async fn set_mcp_api_key(&self, server_name: &str, api_key: &str) -> Result<()> {
        let normalized = Self::normalize_mcp_api_key(api_key);
        crate::extensions::keychain::set_mcp_api_key(server_name, &normalized)
            .map_err(|e| ForgeError::ConfigError(format!("Failed to store API key: {e}")))
    }

    /// Set the proxy password for an MCP server in keychain.
    ///
    /// # Errors
    ///
    /// Returns error if keychain storage fails.
    pub async fn set_mcp_proxy_password(&self, server_name: &str, password: &str) -> Result<()> {
        crate::extensions::keychain::set_mcp_proxy_password(server_name, password)
            .map_err(|e| ForgeError::ConfigError(format!("Failed to store proxy password: {e}")))
    }

    /// Test connection to an MCP server.
    pub async fn test_mcp_connection(&self, name: &str) -> crate::types::McpConnectionTestResult {
        use crate::types::{McpConnectionTestResult, McpToolInfo, McpTransportType};
        use forge_mcp::{McpServerConfig, McpTransportType as ToolsTransport};

        let server_config = match self.get_mcp_server(name).await {
            Some(config) => config,
            None => {
                return McpConnectionTestResult {
                    success: false,
                    error: Some(format!("Server '{name}' not found")),
                    tools: Vec::new(),
                    protocol_version: None,
                };
            }
        };

        let transport = match server_config.transport {
            McpTransportType::Stdio => ToolsTransport::Stdio,
            McpTransportType::Sse => ToolsTransport::Sse,
            McpTransportType::StreamableHttp => ToolsTransport::StreamableHttp,
        };

        let keychain_key =
            crate::extensions::keychain::get_mcp_api_key(&server_config.name).ok().flatten();
        let api_key = if server_config.api_key_from_keychain {
            keychain_key.or(server_config.api_key)
        } else {
            server_config.api_key.or(keychain_key)
        };

        let api_key_auth = match server_config.api_key_auth.as_deref() {
            Some("header") => forge_mcp::ApiKeyAuth::Header,
            _ => forge_mcp::ApiKeyAuth::Bearer,
        };

        let test_config = McpServerConfig {
            name: server_config.name.clone(),
            transport,
            enabled: true,
            command: server_config.command,
            args: server_config.args,
            env: server_config.env,
            url: server_config.url,
            api_key,
            api_key_from_keychain: server_config.api_key_from_keychain,
            api_key_auth,
            api_key_header: server_config.api_key_header,
            api_key_prefix: server_config.api_key_prefix,
            proxy_name: None,
            oauth: None,
        };

        let mut manager = McpManager::new();
        match manager.connect(&test_config).await {
            Ok(()) => {
                let all_tools = manager.list_all_tools().await;
                let tools: Vec<McpToolInfo> = all_tools
                    .into_iter()
                    .flat_map(|(server_name, tools)| {
                        tools.into_iter().map(move |tool| McpToolInfo {
                            name: tool.name,
                            description: tool.description,
                            server_name: server_name.clone(),
                        })
                    })
                    .collect();

                McpConnectionTestResult {
                    success: true,
                    error: None,
                    tools,
                    protocol_version: Some("2024-11-05".to_string()),
                }
            }
            Err(e) => McpConnectionTestResult {
                success: false,
                error: Some(e),
                tools: Vec::new(),
                protocol_version: None,
            },
        }
    }

    fn save_mcp_config(&self, path: &std::path::Path, config: &McpConfig) -> Result<()> {
        let content = toml::to_string_pretty(config)
            .map_err(|e| ForgeError::ConfigError(format!("Failed to serialize config: {e}")))?;
        Self::write_string_atomic(path, &content, "mcp config")
    }

    fn normalize_mcp_api_key(api_key: &str) -> String {
        let trimmed = api_key.trim();
        let mut parts = trimmed.split_whitespace();
        if let Some(first) = parts.next() {
            if first.eq_ignore_ascii_case("bearer") {
                if let Some(token) = parts.next() {
                    return token.to_string();
                }
            }
        }
        trimmed.to_string()
    }
}
