//! Tool management for ForgeSDK

use super::*;

impl ForgeSDK {
    /// Register a tool in the registry.
    pub async fn register_tool(&self, tool: Arc<dyn forge_tools::Tool>) {
        self.tool_registry.write().await.register(tool);
    }

    /// Unregister a tool by name.
    pub async fn unregister_tool(&self, name: &str) -> bool {
        self.tool_registry.write().await.unregister(name)
    }

    /// Get list of registered tool names.
    pub async fn list_tools(&self) -> Vec<String> {
        self.tool_registry.read().await.list_names().into_iter().map(|s| s.to_string()).collect()
    }

    /// List all built-in tools with their status.
    pub async fn list_builtin_tools(&self) -> Vec<ToolInfo> {
        let registry = self.tool_registry.read().await;
        let config = self.config.read().await;
        let disabled_tools = config.tools.disabled.clone();
        let _tool_proxy_map = config.tools.tool_proxy.clone();
        drop(config);

        let mut tools: Vec<ToolInfo> = registry
            .list_all()
            .iter()
            .filter(|tool| !tool.name().starts_with("mcp__"))
            .map(|tool| {
                let name = tool.name().to_string();
                let is_disabled = disabled_tools.contains(&name);
                let category = ToolInfo::category_for_builtin(&name);
                let requires_network = tool.requires_network();
                ToolInfo {
                    name,
                    description: tool.description().to_string(),
                    builtin: true,
                    disabled: is_disabled,
                    category,
                    requires_network,
                }
            })
            .collect();

        tools.sort_by(|a, b| {
            let cat_cmp = format!("{:?}", a.category).cmp(&format!("{:?}", b.category));
            if cat_cmp == std::cmp::Ordering::Equal {
                a.name.cmp(&b.name)
            } else {
                cat_cmp
            }
        });

        tools
    }

    /// Get a snapshot of the tool registry with disabled tools filtered out.
    pub async fn tool_registry_snapshot(&self) -> ToolRegistry {
        let disabled = { self.config.read().await.tools.disabled.clone() };
        let registry = self.tool_registry.read().await;
        if disabled.is_empty() {
            registry.clone()
        } else {
            let mut filtered = registry.clone();
            for tool_name in &disabled {
                filtered.unregister(tool_name);
            }
            filtered
        }
    }

    /// Build a ToolContext suitable for workflow execution.
    pub async fn tool_context_for_workflow(&self) -> ToolContext {
        let (working_dir, timeout_secs) = {
            let config = self.config.read().await;
            (
                config.working_dir.clone(),
                config.tools.bash_timeout,
            )
        };
        let confirmed_paths = self.confirmed_paths.read().await.clone();
        ToolContext {
            working_dir,
            env: self.resolve_tool_env().await,
            timeout_secs,
            confirmed_paths,
            bash_readonly: false,
            #[cfg(feature = "lsp")]
            lsp_manager: Some(self.lsp_manager.clone()),
            ..ToolContext::default()
        }
        .with_plan_mode_flag(self.plan_mode_flag.clone())
    }

    /// Get the list of disabled tool names.
    pub async fn get_disabled_tools(&self) -> Vec<String> {
        self.config.read().await.tools.disabled.clone()
    }

    /// Set the list of disabled tools (replaces existing list).
    pub async fn set_disabled_tools(&self, tools: Vec<String>) -> Result<()> {
        {
            let mut config = self.config.write().await;
            config.tools.disabled = tools;
        }
        self.save_tools_config().await?;
        self.refresh_subagent_security().await;
        Ok(())
    }

    /// Enable a specific tool.
    pub async fn enable_tool(&self, name: &str) -> Result<()> {
        {
            let mut config = self.config.write().await;
            config.tools.disabled.retain(|t| t != name);
        }
        self.save_tools_config().await?;
        self.refresh_subagent_security().await;
        Ok(())
    }

    /// Disable a specific tool.
    pub async fn disable_tool(&self, name: &str) -> Result<()> {
        {
            let mut config = self.config.write().await;
            if !config.tools.disabled.contains(&name.to_string()) {
                config.tools.disabled.push(name.to_string());
            }
        }
        self.save_tools_config().await?;
        self.refresh_subagent_security().await;
        Ok(())
    }

    /// Get the proxy setting for a specific tool.
    pub async fn get_tool_proxy(&self, tool_name: &str) -> Option<String> {
        self.config.read().await.tools.tool_proxy.get(tool_name).cloned()
    }

    /// Set the proxy setting for a specific tool.
    pub async fn set_tool_proxy(&self, tool_name: &str, proxy_name: Option<String>) -> Result<()> {
        {
            let mut config = self.config.write().await;
            if let Some(name) = proxy_name {
                config.tools.tool_proxy.insert(tool_name.to_string(), name);
            } else {
                config.tools.tool_proxy.remove(tool_name);
            }
        }
        self.save_tools_config().await
    }

    /// Get all tool proxy settings.
    pub async fn get_all_tool_proxies(&self) -> HashMap<String, String> {
        self.config.read().await.tools.tool_proxy.clone()
    }

    /// Get the preferred web search provider.
    pub async fn get_search_provider(&self) -> Option<String> {
        self.config.read().await.tools.search_provider.clone()
    }

    /// Set the preferred web search provider.
    pub async fn set_search_provider(&self, provider: Option<String>) -> Result<()> {
        {
            let mut config = self.config.write().await;
            config.tools.search_provider =
                provider.map(|p| p.trim().to_string()).filter(|s| !s.is_empty());
        }
        self.save_tools_config().await
    }

    /// Save tools configuration to disk.
    async fn save_tools_config(&self) -> Result<()> {
        let config_path = forge_infra::config_dir().join("config.toml");
        self.save_tools_config_to_path(&config_path).await
    }

    pub(super) async fn save_tools_config_to_path(
        &self,
        config_path: &std::path::Path,
    ) -> Result<()> {
        let (disabled, tool_proxy, search_provider) = {
            let config = self.config.read().await;
            (
                config.tools.disabled.clone(),
                config.tools.tool_proxy.clone(),
                config.tools.search_provider.clone(),
            )
        };
        let _guard = self.config_persist_lock.lock().await;
        let _file_lock = Self::acquire_config_file_lock(config_path).await?;

        let mut doc = Self::read_toml_doc_or_empty(config_path)?;

        if !doc.contains_key("tools") {
            doc["tools"] = toml_edit::Item::Table(toml_edit::Table::new());
        }

        let disabled_array: toml_edit::Array = disabled.iter().map(|s| s.as_str()).collect();
        doc["tools"]["disabled"] = toml_edit::value(disabled_array);

        if tool_proxy.is_empty() {
            if let Some(tools_table) = doc["tools"].as_table_mut() {
                tools_table.remove("tool_proxy");
            }
        } else {
            let mut proxy_table = toml_edit::InlineTable::new();
            for (tool, proxy) in &tool_proxy {
                proxy_table.insert(tool, proxy.as_str().into());
            }
            doc["tools"]["tool_proxy"] = toml_edit::value(proxy_table);
        }

        if let Some(provider) = search_provider.as_deref() {
            doc["tools"]["search_provider"] = toml_edit::value(provider);
        } else if let Some(tools_table) = doc["tools"].as_table_mut() {
            tools_table.remove("search_provider");
        }

        Self::write_string_atomic(config_path, &doc.to_string(), "config")
    }

    /// Register the TaskTool for sub-agent support.
    ///
    /// # Errors
    ///
    /// Returns error if registration fails.
    pub async fn register_task_tool(&self) -> Result<()> {
        let config = self.config.read().await;
        let effective_model = config.llm.effective_model();
        let working_dir = config.working_dir.clone();
        let subagent_config = config.llm.subagent.clone();
        let permission_rules = config.tools.permission_rules.clone();
        drop(config);

        let full_registry = {
            let registry = self.tool_registry.read().await;
            Arc::new(registry.clone())
        };

        self.refresh_subagent_security().await;
        let security = self.subagent_security.clone();
        let provider_registry = Arc::new(self.provider_registry.clone());

        let executor = Arc::new(RealTaskExecutor::with_full_config(
            provider_registry,
            full_registry,
            working_dir.clone(),
            effective_model,
            subagent_config.clone(),
            self.plan_mode_flag.clone(),
            security,
            permission_rules,
        ));

        let task_state = Arc::new(tokio::sync::RwLock::new(TaskState::new()));
        let task_tool = Arc::new(TaskTool::with_full_config(
            task_state,
            executor,
            Some(self.background_manager.clone()),
            working_dir,
            subagent_config.max_concurrent,
        ));
        self.tool_registry.write().await.register(task_tool);
        Ok(())
    }

    /// Execute a sub-agent task directly.
    ///
    /// # Errors
    ///
    /// Returns error if execution fails.
    pub async fn execute_subagent(
        &self,
        subagent_type: &str,
        description: &str,
        prompt: &str,
    ) -> Result<String> {
        let task_tool = {
            let tool_registry = self.tool_registry.read().await;
            tool_registry.get("task").ok_or_else(|| {
                ForgeError::Tool(forge_tools::ToolError::NotFound(
                    "TaskTool not registered. Call register_task_tool() first.".into(),
                ))
            })?
        };

        let params = serde_json::json!({
            "subagent_type": subagent_type,
            "description": description,
            "prompt": prompt
        });

        let working_dir = { self.config.read().await.working_dir.clone() };
        let tool_context = ToolContext {
            working_dir,
            plan_mode_flag: self.plan_mode_flag.clone(),
            ..Default::default()
        };

        let result = task_tool.execute(params, &tool_context).await?;
        if result.is_error {
            return Err(ForgeError::Tool(forge_tools::ToolError::ExecutionFailed(result.content)));
        }
        Ok(result.content)
    }

    /// Register the BatchTool for parallel tool execution.
    ///
    /// # Errors
    ///
    /// Returns error if registration fails.
    pub async fn register_batch_tool(&self) -> Result<()> {
        use forge_tools::builtin::batch::BatchTool;

        let full_registry = {
            let registry = self.tool_registry.read().await;
            Arc::new(registry.clone())
        };

        let batch_tool = BatchTool::new();
        batch_tool.set_registry(full_registry);
        self.tool_registry.write().await.register(Arc::new(batch_tool));
        Ok(())
    }

    /// Register the GitTool for structured Git operations.
    pub async fn register_git_tool(&self) {
        use forge_tools::builtin::git::GitTool;
        self.tool_registry.write().await.register(Arc::new(GitTool::new()));
    }
}
