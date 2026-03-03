//! Proxy management for ForgeSDK

use super::*;

impl ForgeSDK {
    /// Set the global proxy password in keychain.
    ///
    /// # Errors
    ///
    /// Returns error if keychain storage fails.
    pub async fn set_global_proxy_password(&self, password: &str) -> Result<()> {
        crate::extensions::keychain::set_global_proxy_password(password).map_err(|e| {
            ForgeError::ConfigError(format!("Failed to store global proxy password: {e}"))
        })
    }

    /// Get a named proxy configuration.
    pub async fn get_proxy(&self, name: &str) -> Option<forge_config::ProxyConfig> {
        if name == "global" {
            Some(self.config.read().await.tools.proxy.clone())
        } else {
            None
        }
    }

    /// Create or update a named proxy configuration.
    ///
    /// # Errors
    ///
    /// Returns error if name is not "global" or persistence fails.
    pub async fn set_proxy(&self, name: &str, config: forge_config::ProxyConfig) -> Result<()> {
        if name != "global" {
            return Err(ForgeError::ConfigError(format!(
                "Only 'global' proxy is supported. Got: '{name}'"
            )));
        }
        {
            let mut cfg = self.config.write().await;
            cfg.tools.proxy = config.clone();
        }
        let config_path = forge_infra::config_dir().join("config.toml");
        self.save_global_config_proxy(&config_path, &config).await
    }

    /// Delete a named proxy configuration.
    ///
    /// # Errors
    ///
    /// Returns error if name is not "global".
    pub async fn delete_proxy(&self, name: &str) -> Result<()> {
        if name != "global" {
            return Err(ForgeError::ConfigError(format!(
                "Only 'global' proxy is supported. Got: '{name}'"
            )));
        }
        self.set_proxy(name, forge_config::ProxyConfig::none()).await
    }

    /// List all named proxies.
    pub async fn list_proxies(&self) -> Vec<crate::types::ProxyInfo> {
        let config = self.config.read().await.tools.proxy.clone();
        vec![crate::types::ProxyInfo { name: "global".to_string(), config }]
    }

    /// Get the global proxy configuration.
    pub async fn get_global_proxy_config(&self) -> forge_config::ProxyConfig {
        self.get_proxy("global").await.unwrap_or_default()
    }

    /// Set the global proxy configuration.
    ///
    /// # Errors
    ///
    /// Returns error if persistence fails.
    pub async fn set_global_proxy_config(&self, proxy: forge_config::ProxyConfig) -> Result<()> {
        self.set_proxy("global", proxy).await
    }

    async fn save_global_config_proxy(
        &self,
        path: &std::path::Path,
        proxy: &forge_config::ProxyConfig,
    ) -> Result<()> {
        let _guard = self.config_persist_lock.lock().await;
        let _file_lock = Self::acquire_config_file_lock(path).await?;
        self.save_global_config_proxy_unlocked(path, proxy)
    }

    fn save_global_config_proxy_unlocked(
        &self,
        path: &std::path::Path,
        proxy: &forge_config::ProxyConfig,
    ) -> Result<()> {
        let existing_content = if path.exists() {
            std::fs::read_to_string(path)
                .map_err(|e| ForgeError::StorageError(format!("Failed to read config: {e}")))?
        } else {
            String::new()
        };

        let mut doc: toml::Value = if existing_content.is_empty() {
            toml::Value::Table(toml::map::Map::new())
        } else {
            existing_content
                .parse()
                .map_err(|e| ForgeError::ConfigError(format!("Failed to parse config: {e}")))?
        };

        let proxy_value = toml::Value::try_from(proxy.clone())
            .map_err(|e| ForgeError::ConfigError(format!("Failed to serialize proxy: {e}")))?;

        if let toml::Value::Table(ref mut root) = doc {
            let tools = root
                .entry("tools".to_string())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            if let toml::Value::Table(ref mut tools_table) = tools {
                tools_table.insert("proxy".to_string(), proxy_value);
            }
        }

        let content = toml::to_string_pretty(&doc)
            .map_err(|e| ForgeError::ConfigError(format!("Failed to serialize config: {e}")))?;
        Self::write_string_atomic(path, &content, "config")
    }
}
