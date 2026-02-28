//! Configuration types for Python bindings

use pyo3::prelude::*;

/// Forge SDK configuration
#[pyclass(name = "ForgeConfig")]
#[derive(Clone, Default)]
pub struct PyForgeConfig {
    #[pyo3(get, set)]
    pub model: Option<String>,
    #[pyo3(get, set)]
    pub provider: Option<String>,
    #[pyo3(get, set)]
    pub working_dir: Option<String>,
}

#[pymethods]
impl PyForgeConfig {
    #[new]
    #[pyo3(signature = (model=None, provider=None, working_dir=None))]
    fn new(model: Option<String>, provider: Option<String>, working_dir: Option<String>) -> Self {
        Self { model, provider, working_dir }
    }

    fn __repr__(&self) -> String {
        format!(
            "ForgeConfig(model={:?}, provider={:?}, working_dir={:?})",
            self.model, self.provider, self.working_dir
        )
    }
}
