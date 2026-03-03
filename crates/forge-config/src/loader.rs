//! Multi-source configuration loader.

use std::path::{Path, PathBuf};

use crate::{ConfigError, ForgeConfig};

/// Configuration loader with multi-source support.
pub struct ConfigLoader {
    /// Configuration file search paths.
    search_paths: Vec<PathBuf>,
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigLoader {
    /// Create a new configuration loader.
    #[must_use]
    pub fn new() -> Self {
        let mut search_paths = Vec::new();

        // Global config (~/.forge/config.toml)
        if let Some(home) = dirs::home_dir() {
            search_paths.push(home.join(".forge/config.toml"));
        }

        // Project config (added during load based on cwd)
        search_paths.push(PathBuf::from(".forge/config.toml"));

        Self { search_paths }
    }

    /// Load configuration from all sources.
    ///
    /// Priority (low to high):
    /// 1. Default values
    /// 2. Global config (`~/.forge/config.toml`)
    /// 3. Project config (`.forge/config.toml`)
    /// 4. Environment variables (`FORGE__*`)
    ///
    /// # Errors
    /// Returns error if config files are malformed.
    pub fn load(&self) -> Result<ForgeConfig, ConfigError> {
        let mut builder = config::Config::builder();

        // 1. Default values — serialize ForgeConfig::default() as base
        let default_toml = toml::to_string(&ForgeConfig::default())?;
        builder =
            builder.add_source(config::File::from_str(&default_toml, config::FileFormat::Toml));

        // 2. Configuration files (by priority low to high)
        for path in &self.search_paths {
            if path.exists() {
                builder = builder.add_source(config::File::from(path.clone()).required(false));
            }
        }

        // 3. Environment variables (highest priority)
        // Use "__" separator for nested structure: FORGE__LLM__BASE_URL -> llm.base_url
        builder = builder.add_source(
            config::Environment::with_prefix("FORGE").separator("__").try_parsing(true),
        );

        let settings = builder.build().map_err(|e| ConfigError::Load(e.to_string()))?;

        settings.try_deserialize().map_err(|e| ConfigError::Load(e.to_string()))
    }

    /// Load configuration from a specific file.
    ///
    /// # Errors
    /// Returns error if the file is unreadable or malformed.
    pub fn load_file(&self, path: &Path) -> Result<ForgeConfig, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }

    /// Save configuration to a file.
    ///
    /// # Errors
    /// Returns error if the file cannot be written.
    pub fn save(&self, config: &ForgeConfig, path: &Path) -> Result<(), ConfigError> {
        let content = toml::to_string_pretty(config)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }
}
