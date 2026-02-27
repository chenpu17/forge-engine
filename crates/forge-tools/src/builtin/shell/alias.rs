//! Shell alias tool
//!
//! Provides a stable `shell` tool name that delegates to the platform-specific implementation.

use crate::{ConfirmationLevel, ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use serde_json::Value;
use std::sync::Arc;

/// Shell alias - stable cross-platform entry point
///
/// This tool delegates to the platform-specific shell tool (bash on Unix, powershell on Windows)
/// while providing a stable "shell" name for cross-platform scripts and prompts.
pub struct ShellAlias {
    inner: Arc<dyn Tool>,
    description: String,
}

impl ShellAlias {
    /// Create a new shell alias wrapping the given tool
    pub fn new(inner: Arc<dyn Tool>) -> Self {
        let platform_name = inner.name();
        let description = format!(
            "`shell` is an alias for `{}` on this platform.\n\n{}",
            platform_name,
            inner.description()
        );
        Self { inner, description }
    }
}

#[async_trait]
impl Tool for ShellAlias {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.inner.parameters_schema()
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        self.inner.execute(params, ctx).await
    }

    fn confirmation_level(&self, params: &Value) -> ConfirmationLevel {
        self.inner.confirmation_level(params)
    }
}
