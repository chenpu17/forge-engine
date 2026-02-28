//! Session types for Python bindings

use pyo3::prelude::*;

/// Session summary
#[pyclass(name = "SessionSummary")]
#[derive(Clone)]
pub struct PySessionSummary {
    #[pyo3(get)]
    pub id: String,
    #[pyo3(get)]
    pub title: Option<String>,
    #[pyo3(get)]
    pub created_at: String,
    #[pyo3(get)]
    pub updated_at: String,
}

#[pymethods]
impl PySessionSummary {
    fn __repr__(&self) -> String {
        format!("SessionSummary(id='{}', title={:?})", self.id, self.title)
    }
}

impl From<forge_sdk::SessionSummary> for PySessionSummary {
    fn from(s: forge_sdk::SessionSummary) -> Self {
        Self {
            id: s.id,
            title: s.title,
            created_at: s.created_at.to_rfc3339(),
            updated_at: s.updated_at.to_rfc3339(),
        }
    }
}

/// Session status
#[pyclass(name = "SessionStatus")]
#[derive(Clone)]
pub struct PySessionStatus {
    #[pyo3(get)]
    pub id: String,
    #[pyo3(get)]
    pub message_count: usize,
    #[pyo3(get)]
    pub model: String,
    #[pyo3(get)]
    pub working_dir: String,
    #[pyo3(get)]
    pub input_tokens: usize,
    #[pyo3(get)]
    pub output_tokens: usize,
    #[pyo3(get)]
    pub context_limit: usize,
    #[pyo3(get)]
    pub persona: String,
    #[pyo3(get)]
    pub title: Option<String>,
    #[pyo3(get)]
    pub is_dirty: bool,
}

#[pymethods]
impl PySessionStatus {
    fn __repr__(&self) -> String {
        format!("SessionStatus(id='{}', model='{}')", self.id, self.model)
    }
}

impl From<forge_sdk::SessionStatus> for PySessionStatus {
    fn from(s: forge_sdk::SessionStatus) -> Self {
        Self {
            id: s.id,
            message_count: s.message_count,
            model: s.model,
            working_dir: s.working_dir.to_string_lossy().to_string(),
            input_tokens: s.token_usage.input_tokens,
            output_tokens: s.token_usage.output_tokens,
            context_limit: s.context_limit,
            persona: s.persona,
            title: s.title,
            is_dirty: s.is_dirty,
        }
    }
}

/// Tool info
#[pyclass(name = "ToolInfo")]
#[derive(Clone)]
pub struct PyToolInfo {
    #[pyo3(get)]
    pub name: String,
    #[pyo3(get)]
    pub description: String,
    #[pyo3(get)]
    pub builtin: bool,
    #[pyo3(get)]
    pub disabled: bool,
    #[pyo3(get)]
    pub category: String,
}

#[pymethods]
impl PyToolInfo {
    fn __repr__(&self) -> String {
        format!("ToolInfo(name='{}', category='{}')", self.name, self.category)
    }
}

impl From<forge_sdk::ToolInfo> for PyToolInfo {
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
        }
    }
}
