//! Main SDK bindings for NAPI

use crate::config::ForgeConfig;
use napi_derive::napi;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Forge SDK - Main entry point for Node.js applications.
#[allow(missing_docs)]
#[napi]
pub struct ForgeSDK {
    inner: Arc<RwLock<Option<forge_sdk::ForgeSDK>>>,
    config: forge_sdk::ForgeConfig,
}

#[allow(missing_docs)]
#[napi]
impl ForgeSDK {
    /// Create a new SDK instance from configuration.
    #[napi(constructor)]
    pub fn new(config: &ForgeConfig) -> napi::Result<Self> {
        Ok(Self {
            inner: Arc::new(RwLock::new(None)),
            config: config.clone_inner(),
        })
    }

    /// Get a clone of the inner SDK handle.
    pub(crate) fn inner_handle(&self) -> Arc<RwLock<Option<forge_sdk::ForgeSDK>>> {
        self.inner.clone()
    }

    /// Initialize the SDK
    #[napi]
    pub async fn init(&self) -> napi::Result<()> {
        let config = self.config.clone();
        let sdk = forge_sdk::ForgeSDKBuilder::new()
            .working_dir(&config.working_dir)
            .provider_name(&config.llm.provider)
            .model(&config.llm.model)
            .max_tokens(config.llm.max_tokens)
            .with_builtin_tools()
            .build()
            .map_err(|e| napi::Error::from_reason(format!("Failed to initialize SDK: {e}")))?;

        *self.inner.write().await = Some(sdk);
        Ok(())
    }

    /// Get current configuration snapshot
    #[napi]
    pub async fn get_config(&self) -> napi::Result<String> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| {
            napi::Error::from_reason("SDK not initialized")
        })?;
        let config = sdk.config().await;
        serde_json::to_string(&config)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }
}
