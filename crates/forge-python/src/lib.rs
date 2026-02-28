//! Python bindings for Forge SDK

mod config;
mod event;
mod session;

use pyo3::prelude::*;

pub use config::PyForgeConfig;
pub use event::PyAgentEvent;
pub use session::*;

/// Python module definition
#[pymodule]
fn forge_python(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyForgeConfig>()?;
    m.add_class::<PyAgentEvent>()?;
    m.add_class::<PySessionSummary>()?;
    m.add_class::<PySessionStatus>()?;
    m.add_class::<PyToolInfo>()?;
    m.add_class::<ForgeSDK>()?;
    Ok(())
}

/// Global tokio runtime shared across all SDK instances
fn global_runtime() -> PyResult<&'static tokio::runtime::Runtime> {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Runtime::new()
                .map_err(|e| format!("Failed to create tokio runtime: {e}"))
        })
        .as_ref()
        .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.clone()))
}

/// Forge SDK - Main entry point for Python applications
#[pyclass(name = "ForgeSDK")]
pub struct ForgeSDK {
    inner: std::sync::Arc<tokio::sync::RwLock<Option<forge_sdk::ForgeSDK>>>,
    config: PyForgeConfig,
}

#[pymethods]
impl ForgeSDK {
    #[new]
    fn new(config: PyForgeConfig) -> Self {
        Self {
            inner: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
            config,
        }
    }

    /// Initialize the SDK
    fn init(&self) -> PyResult<()> {
        let rt = global_runtime()?;
        let config = self.config.clone();
        let inner = self.inner.clone();

        rt.block_on(async move {
            let mut builder = forge_sdk::ForgeSDKBuilder::new();

            if let Some(ref dir) = config.working_dir {
                builder = builder.working_dir(dir);
            }
            if let Some(ref provider) = config.provider {
                builder = builder.provider_name(provider);
            }
            if let Some(ref model) = config.model {
                builder = builder.model(model);
            }

            builder = builder.with_builtin_tools();

            let sdk = builder.build().map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("Failed to init SDK: {e}"))
            })?;

            *inner.write().await = Some(sdk);
            Ok(())
        })
    }

    /// Get current configuration as JSON
    fn get_config(&self) -> PyResult<String> {
        let rt = global_runtime()?;
        let inner = self.inner.clone();

        rt.block_on(async move {
            let guard: tokio::sync::RwLockReadGuard<'_, Option<forge_sdk::ForgeSDK>> = inner.read().await;
            let sdk = guard.as_ref().ok_or_else(|| {
                pyo3::exceptions::PyRuntimeError::new_err("SDK not initialized")
            })?;
            let config = sdk.config().await;
            serde_json::to_string(&config)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("{e}")))
        })
    }

    fn __repr__(&self) -> String {
        format!(
            "ForgeSDK(model={:?}, provider={:?})",
            self.config.model, self.config.provider
        )
    }
}
