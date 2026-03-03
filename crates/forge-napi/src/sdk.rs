//! Main SDK bindings for NAPI

use crate::config::ForgeConfig;
use crate::events::JsAgentEvent;
use crate::session::{
    JsCompressionResult, JsHistoryMessage, JsMcpConnectionTestResult, JsMcpServerManageConfig,
    JsMcpStatus, JsModelSwitchResult, JsSessionStatus, JsSessionSummary,
};
use crate::skills::{JsSkillFull, JsSkillInfo, JsSkillPath};
use crate::stream::process_stream_with_callback;
use crate::tools::{JsProxyConfig, JsProxyInfo, JsToolInfo};
use napi::threadsafe_function::ThreadsafeFunction;
use napi_derive::napi;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Processing options for `process_in_session`.
#[napi(object)]
pub struct JsProcessOptions {
    /// Dispatch mode: "Immediate" | "Batched".
    pub dispatch_mode: Option<String>,
    /// Flush when buffered bytes exceed this size (Batched).
    pub max_bytes: Option<u32>,
    /// Flush at least every N milliseconds (Batched).
    pub max_latency_ms: Option<u32>,
}

impl From<JsProcessOptions> for forge_sdk::ProcessOptions {
    fn from(opts: JsProcessOptions) -> Self {
        match opts.dispatch_mode.as_deref() {
            Some("Batched") | Some("batched") => Self {
                dispatch_mode: forge_sdk::EventDispatchMode::Batched {
                    max_bytes: usize::try_from(opts.max_bytes.unwrap_or(4096)).unwrap_or(4096),
                    max_latency_ms: u64::from(opts.max_latency_ms.unwrap_or(100)),
                },
            },
            _ => Self { dispatch_mode: forge_sdk::EventDispatchMode::Immediate },
        }
    }
}

/// Forge SDK — Main entry point for Node.js applications.
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
        Ok(Self { inner: Arc::new(RwLock::new(None)), config: config.clone_inner() })
    }

    /// Get a clone of the inner SDK handle.
    pub(crate) fn inner_handle(&self) -> Arc<RwLock<Option<forge_sdk::ForgeSDK>>> {
        self.inner.clone()
    }

    // ========================
    // Initialization
    // ========================

    /// Initialize the SDK.
    #[napi]
    pub async fn init(&self) -> napi::Result<()> {
        let config = self.config.clone();
        // Use `from_forge_config` so that ALL settings (api_key, base_url,
        // temperature, thinking, etc.) are preserved.  Previously only 4 fields
        // were forwarded to the builder, causing api_key/base_url to be lost,
        // which made LLM requests go to the wrong endpoint and hang forever.
        //
        // Use `build_async` because `init()` runs inside the NAPI tokio runtime.
        // `build()` creates its own runtime internally and panics with
        // "Cannot start a runtime from within a runtime" when called
        // from an async context.
        let sdk = forge_sdk::ForgeSDKBuilder::from_forge_config(config)
            .with_builtin_tools()
            .build_async()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to initialize SDK: {e}")))?;

        *self.inner.write().await = Some(sdk);
        Ok(())
    }

    /// Get current configuration snapshot as JSON.
    #[napi]
    pub async fn get_config(&self) -> napi::Result<String> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let config = sdk.config().await;
        serde_json::to_string(&config)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    // ========================
    // Session Management
    // ========================

    /// Create a new session and return its ID.
    #[napi]
    pub async fn create_session(&self) -> napi::Result<String> {
        let guard = self.inner.read().await;
        let sdk = guard
            .as_ref()
            .ok_or_else(|| napi::Error::from_reason("SDK not initialized. Call init() first."))?;
        sdk.create_session()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to create session: {e}")))
    }

    /// Resume an existing session by ID.
    #[napi]
    pub async fn resume_session(&self, id: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.resume_session(id)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to resume session: {e}")))
    }

    /// Get the most recent session ID (resumes it if found).
    #[napi]
    pub async fn latest_session(&self) -> napi::Result<Option<String>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.latest_session()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to get latest session: {e}")))
    }

    /// List all sessions.
    #[napi]
    pub async fn list_sessions(&self) -> napi::Result<Vec<JsSessionSummary>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let sessions = sdk
            .list_sessions()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to list sessions: {e}")))?;
        Ok(sessions.into_iter().map(Into::into).collect())
    }

    /// Save the current session.
    #[napi]
    pub async fn save_session(&self) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.save_session()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to save session: {e}")))
    }

    /// Close the current session (saves first).
    #[napi]
    pub async fn close_session(&self) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.close_session()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to close session: {e}")))
    }

    /// Delete a session by ID.
    #[napi]
    pub async fn delete_session(&self, id: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.delete_session(id)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to delete session: {e}")))
    }

    /// Get the active session ID.
    #[napi]
    pub async fn active_session_id(&self) -> napi::Result<Option<String>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.active_session_id().await)
    }

    // ========================
    // Message Processing
    // ========================

    /// Process user input and stream events to a callback.
    #[napi]
    pub async fn process(
        &self,
        input: String,
        callback: ThreadsafeFunction<JsAgentEvent>,
    ) -> napi::Result<()> {
        // Release the read lock before consuming the stream so that concurrent
        // write-lock operations (e.g. init()) are not blocked for the entire
        // duration of an LLM response.
        let stream = {
            let guard = self.inner.read().await;
            let sdk =
                guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
            sdk.process(&input)
                .await
                .map_err(|e| napi::Error::from_reason(format!("Failed to process input: {e}")))?
        };
        process_stream_with_callback(stream, callback).await
    }

    /// Process input with conversation history.
    #[napi]
    pub async fn process_with_history(
        &self,
        input: String,
        history: Vec<JsHistoryMessage>,
        callback: ThreadsafeFunction<JsAgentEvent>,
    ) -> napi::Result<()> {
        let rust_history: Vec<forge_sdk::HistoryMessage> =
            history.into_iter().map(Into::into).collect();
        // Release the read lock before consuming the stream (same rationale as
        // `process()`).
        let stream = {
            let guard = self.inner.read().await;
            let sdk = guard
                .as_ref()
                .ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
            sdk.process_with_history(&input, &rust_history)
                .await
                .map_err(|e| napi::Error::from_reason(format!("Failed to process input: {e}")))?
        };
        process_stream_with_callback(stream, callback).await
    }

    /// Process user input in a specific session (multi-session safe).
    ///
    /// Returns request_id that can be used to cancel the request.
    #[napi]
    pub async fn process_in_session(
        &self,
        session_id: String,
        input: String,
        callback: ThreadsafeFunction<JsAgentEvent>,
        options: Option<JsProcessOptions>,
    ) -> napi::Result<String> {
        let process_opts = options.map(Into::into).unwrap_or(forge_sdk::ProcessOptions {
            dispatch_mode: forge_sdk::EventDispatchMode::Immediate,
        });

        // Release the read lock before spawning the stream task.
        let (request_id, stream) = {
            let guard = self.inner.read().await;
            let sdk = guard
                .as_ref()
                .ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
            let handle = sdk
                .process_in_session(&session_id, &input, process_opts)
                .await
                .map_err(|e| {
                    napi::Error::from_reason(format!("Failed to process in session: {e}"))
                })?;
            (handle.request_id, handle.stream)
        };

        tokio::spawn(async move {
            if let Err(e) = process_stream_with_callback(stream, callback).await {
                eprintln!("[forge-napi] process_in_session: stream processing error: {e}");
            }
        });
        Ok(request_id)
    }

    // ========================
    // Sub-agent Execution
    // ========================

    /// Execute a sub-agent and return its output.
    #[napi]
    pub async fn execute_subagent(
        &self,
        subagent_type: String,
        description: String,
        prompt: String,
    ) -> napi::Result<String> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.execute_subagent(&subagent_type, &description, &prompt)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to execute subagent: {e}")))
    }

    // ========================
    // Abort / Cancel
    // ========================

    /// Abort the current operation (deprecated: prefer cancel).
    #[napi]
    pub async fn abort(&self) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.abort().await;
        Ok(())
    }

    /// Cancel a specific in-flight request. Returns true if cancelled.
    #[napi]
    pub async fn cancel(&self, session_id: String, request_id: String) -> napi::Result<bool> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.cancel(&session_id, &request_id).await)
    }

    /// Check if an abort has been requested.
    #[napi]
    pub async fn is_abort_requested(&self) -> napi::Result<bool> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.is_abort_requested().await)
    }

    // ========================
    // Confirmation
    // ========================

    /// Respond to a tool confirmation request.
    #[napi]
    pub async fn respond_to_confirmation(
        &self,
        id: String,
        allowed: bool,
    ) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.respond_to_confirmation(&id, allowed)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to respond to confirmation: {e}")))
    }

    /// Respond to a confirmation in a specific session.
    #[napi]
    pub async fn respond_to_confirmation_in_session(
        &self,
        session_id: String,
        confirmation_id: String,
        allowed: bool,
        always_allow: bool,
    ) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.respond_to_confirmation_in_session(&session_id, &confirmation_id, allowed, always_allow)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to respond to confirmation: {e}")))
    }

    /// Respond to a confirmation in a specific request.
    #[napi]
    pub async fn respond_to_confirmation_in_request(
        &self,
        session_id: String,
        request_id: String,
        confirmation_id: String,
        allowed: bool,
    ) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.respond_to_confirmation_in_request(&session_id, &request_id, &confirmation_id, allowed)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to respond to confirmation: {e}")))
    }

    /// Check if there are pending confirmations.
    #[napi]
    pub async fn has_pending_confirmations(&self) -> napi::Result<bool> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.has_pending_confirmations().await)
    }

    /// Check if a specific session has pending confirmations.
    #[napi]
    pub async fn has_pending_confirmations_in_session(
        &self,
        session_id: String,
    ) -> napi::Result<bool> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.has_pending_confirmations_in_session(&session_id).await)
    }

    /// Cancel all pending confirmations.
    #[napi]
    pub async fn cancel_all_confirmations(&self) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.cancel_all_confirmations().await;
        Ok(())
    }

    /// Cancel all pending confirmations in a specific session.
    #[napi]
    pub async fn cancel_all_confirmations_in_session(
        &self,
        session_id: String,
    ) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.cancel_all_confirmations_in_session(&session_id).await;
        Ok(())
    }

    /// Add a confirmed path (skip confirmation for this path).
    #[napi]
    pub async fn add_confirmed_path(&self, path: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.add_confirmed_path(std::path::PathBuf::from(path)).await;
        Ok(())
    }

    /// Set the list of confirmed paths.
    #[napi]
    pub async fn set_confirmed_paths(&self, paths: Vec<String>) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_confirmed_paths(paths.into_iter().map(std::path::PathBuf::from).collect()).await;
        Ok(())
    }

    /// Clear all confirmed paths.
    #[napi]
    pub async fn clear_confirmed_paths(&self) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.clear_confirmed_paths().await;
        Ok(())
    }

    // ========================
    // Thinking Mode
    // ========================

    /// Enable or disable extended thinking.
    #[napi]
    pub async fn set_thinking_enabled(
        &self,
        enabled: bool,
        budget_tokens: Option<u32>,
    ) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_thinking_enabled(
            enabled,
            budget_tokens.map(|t| usize::try_from(t).unwrap_or(usize::MAX)),
        )
        .await;
        Ok(())
    }

    // ========================
    // Model / Context
    // ========================

    /// Switch the LLM model.
    #[napi]
    pub async fn switch_model(&self, model: String) -> napi::Result<JsModelSwitchResult> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let result = sdk
            .switch_model(&model)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to switch model: {e}")))?;
        Ok(result.into())
    }

    /// Get the current model name.
    #[napi]
    pub async fn current_model(&self) -> napi::Result<String> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.current_model().await)
    }

    /// Compact the context (summarize old messages).
    #[napi]
    pub async fn compact_context(
        &self,
        instructions: Option<String>,
    ) -> napi::Result<JsCompressionResult> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let result = sdk
            .compact_context(instructions.as_deref())
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to compact context: {e}")))?;
        Ok(result.into())
    }

    /// Check if context compression is needed.
    #[napi]
    pub async fn needs_compression(&self) -> napi::Result<bool> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.needs_compression()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to check compression: {e}")))
    }

    /// Get the current context token count.
    #[napi]
    pub async fn context_token_count(&self) -> napi::Result<u32> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let count = sdk
            .context_token_count()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to get token count: {e}")))?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    // ========================
    // Status
    // ========================

    /// Get the current session status.
    #[napi]
    pub async fn get_status(&self) -> napi::Result<JsSessionStatus> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let status = sdk
            .get_status()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to get status: {e}")))?;
        Ok(status.into())
    }

    /// Check if the session has unsaved changes.
    #[napi]
    pub async fn is_dirty(&self) -> napi::Result<bool> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.is_dirty().await)
    }

    // ========================
    // Persona
    // ========================

    /// Set the active persona.
    #[napi]
    pub async fn set_persona(&self, name: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_persona(&name)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to set persona: {e}")))
    }

    /// Get the current persona name.
    #[napi]
    pub async fn current_persona(&self) -> napi::Result<String> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.current_persona().await)
    }

    /// List all available persona names.
    #[napi]
    pub async fn list_personas(&self) -> napi::Result<Vec<String>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.list_personas().await)
    }

    // ========================
    // Tool Management
    // ========================

    /// List all registered tool names.
    #[napi]
    pub async fn list_tools(&self) -> napi::Result<Vec<String>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.list_tools().await)
    }

    /// List built-in tools with metadata.
    #[napi]
    pub async fn list_builtin_tools(&self) -> napi::Result<Vec<JsToolInfo>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.list_builtin_tools().await.into_iter().map(Into::into).collect())
    }

    /// Get the list of disabled tool names.
    #[napi]
    pub async fn get_disabled_tools(&self) -> napi::Result<Vec<String>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.get_disabled_tools().await)
    }

    /// Set the list of disabled tool names.
    #[napi]
    pub async fn set_disabled_tools(&self, tools: Vec<String>) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_disabled_tools(tools).await;
        Ok(())
    }

    /// Enable a specific tool.
    #[napi]
    pub async fn enable_tool(&self, name: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.enable_tool(&name).await;
        Ok(())
    }

    /// Disable a specific tool.
    #[napi]
    pub async fn disable_tool(&self, name: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.disable_tool(&name).await;
        Ok(())
    }

    /// Get the proxy assigned to a tool.
    #[napi]
    pub async fn get_tool_proxy(&self, tool_name: String) -> napi::Result<Option<String>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.get_tool_proxy(&tool_name).await)
    }

    /// Set the proxy for a tool.
    #[napi]
    pub async fn set_tool_proxy(
        &self,
        tool_name: String,
        proxy_name: Option<String>,
    ) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_tool_proxy(&tool_name, proxy_name)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to set tool proxy: {e}")))
    }

    /// Get all tool proxy assignments.
    #[napi]
    pub async fn get_all_tool_proxies(
        &self,
    ) -> napi::Result<std::collections::HashMap<String, String>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.get_all_tool_proxies().await)
    }

    /// Get the current search provider name.
    #[napi]
    pub async fn get_search_provider(&self) -> napi::Result<Option<String>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.get_search_provider().await)
    }

    /// Set the search provider name.
    #[napi]
    pub async fn set_search_provider(&self, provider: Option<String>) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_search_provider(provider)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to set search provider: {e}")))
    }

    /// Get tool registry snapshot as JSON.
    #[napi]
    pub async fn tool_registry_snapshot(&self) -> napi::Result<String> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let registry = sdk.tool_registry_snapshot().await;
        let defs = registry.all_defs();
        serde_json::to_string(&defs)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    // ========================
    // Skills
    // ========================

    /// List all available skills.
    #[napi]
    pub async fn list_skills(&self) -> napi::Result<Vec<JsSkillInfo>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.list_skills().await.into_iter().map(Into::into).collect())
    }

    /// Reload skills from disk.
    #[napi]
    pub async fn reload_skills(&self) -> napi::Result<u32> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let count = sdk
            .reload_skills()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to reload skills: {e}")))?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// Get full skill definition by name.
    #[napi]
    pub async fn get_skill_full(&self, name: String) -> napi::Result<JsSkillFull> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let skill = sdk
            .get_skill_full(&name)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to get skill: {e}")))?;
        Ok(skill.into())
    }

    /// Get configured skill search paths.
    #[napi]
    pub async fn get_skill_paths(&self) -> napi::Result<Vec<JsSkillPath>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk
            .get_skill_paths()
            .await
            .into_iter()
            .map(|(path, source)| {
                let source_str = match source {
                    forge_sdk::extensions::skill::SkillSource::Builtin => "builtin",
                    forge_sdk::extensions::skill::SkillSource::User => "user",
                    forge_sdk::extensions::skill::SkillSource::Project => "project",
                };
                JsSkillPath { path: path.to_string_lossy().to_string(), source: source_str.to_string() }
            })
            .collect())
    }

    /// Check if project-level skills are trusted.
    #[napi]
    pub async fn is_project_skills_trusted(&self) -> napi::Result<bool> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.is_project_skills_trusted().await)
    }

    // ========================
    // Message History
    // ========================

    /// Add a user message to the current session.
    #[napi]
    pub async fn add_user_message(&self, content: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.add_user_message(&content)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to add message: {e}")))
    }

    /// Add an assistant message to the current session.
    #[napi]
    pub async fn add_assistant_message(&self, content: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.add_assistant_message(&content)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to add message: {e}")))
    }

    /// Convert active session messages to history format.
    #[napi]
    pub async fn session_to_history(&self) -> napi::Result<Vec<JsHistoryMessage>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let history = sdk
            .session_to_history()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to get history: {e}")))?;
        Ok(history.into_iter().map(Into::into).collect())
    }

    /// Get raw session messages as JSON.
    #[napi]
    pub async fn get_session_messages(&self) -> napi::Result<String> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let messages = sdk
            .get_session_messages()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to get messages: {e}")))?;
        serde_json::to_string(&messages)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Reload prompts from disk.
    #[napi]
    pub async fn reload_prompts(&self) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.reload_prompts()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to reload prompts: {e}")))
    }

    // ========================
    // Project
    // ========================

    /// Analyze the project and return a ProjectAnalysis JSON.
    #[napi]
    pub async fn init_project(&self) -> napi::Result<String> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let analysis = sdk
            .init_project()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to init project: {e}")))?;
        serde_json::to_string(&analysis)
            .map_err(|e| napi::Error::from_reason(format!("Serialization error: {e}")))
    }

    /// Check if the project has documentation files.
    #[napi]
    pub async fn has_project_documentation(&self) -> napi::Result<bool> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.has_project_documentation().await)
    }

    // ========================
    // Memory
    // ========================

    /// Append a memory entry.
    #[napi]
    pub async fn add_memory(&self, scope: String, content: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let memory_scope = match scope.to_lowercase().as_str() {
            "project" => forge_sdk::MemoryScope::Project,
            _ => forge_sdk::MemoryScope::User,
        };
        sdk.add_memory(memory_scope, &content)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to add memory: {e}")))
    }

    /// Get memory content for the given scope.
    #[napi]
    pub async fn get_memory(&self, scope: String) -> napi::Result<Option<String>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let memory_scope = match scope.to_lowercase().as_str() {
            "project" => forge_sdk::MemoryScope::Project,
            _ => forge_sdk::MemoryScope::User,
        };
        sdk.get_memory(memory_scope)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to get memory: {e}")))
    }

    // ========================
    // MCP
    // ========================

    /// Load MCP tools from configured servers.
    #[napi]
    pub async fn load_mcp_tools(&self) -> napi::Result<u32> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        let count = sdk
            .load_mcp_tools()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to load MCP tools: {e}")))?;
        Ok(u32::try_from(count).unwrap_or(u32::MAX))
    }

    /// List MCP server status.
    #[napi]
    pub async fn list_mcp_servers(&self) -> napi::Result<JsMcpStatus> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.list_mcp_servers().await.into())
    }

    /// Get a specific MCP server configuration.
    #[napi]
    pub async fn get_mcp_server(
        &self,
        name: String,
    ) -> napi::Result<Option<JsMcpServerManageConfig>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.get_mcp_server(&name).await.map(Into::into))
    }

    /// Add a new MCP server configuration.
    #[napi]
    pub async fn add_mcp_server(&self, config: JsMcpServerManageConfig) -> napi::Result<()> {
        // Validate env_json before conversion to catch malformed JSON early.
        if let Some(ref json) = config.env_json {
            serde_json::from_str::<std::collections::HashMap<String, String>>(json)
                .map_err(|e| napi::Error::from_reason(format!("Invalid env_json: {e}")))?;
        }
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.add_mcp_server(config.into())
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to add MCP server: {e}")))
    }

    /// Update an existing MCP server configuration.
    #[napi]
    pub async fn update_mcp_server(
        &self,
        name: String,
        config: JsMcpServerManageConfig,
    ) -> napi::Result<()> {
        // Validate env_json before conversion to catch malformed JSON early.
        if let Some(ref json) = config.env_json {
            serde_json::from_str::<std::collections::HashMap<String, String>>(json)
                .map_err(|e| napi::Error::from_reason(format!("Invalid env_json: {e}")))?;
        }
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.update_mcp_server(&name, config.into())
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to update MCP server: {e}")))
    }

    /// Remove an MCP server.
    #[napi]
    pub async fn remove_mcp_server(&self, name: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.remove_mcp_server(&name)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to remove MCP server: {e}")))
    }

    /// Set an API key for an MCP server.
    #[napi]
    pub async fn set_mcp_api_key(
        &self,
        server_name: String,
        api_key: String,
    ) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_mcp_api_key(&server_name, &api_key)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to set MCP API key: {e}")))
    }

    /// Set a proxy password for an MCP server (stored in keychain).
    #[napi]
    pub async fn set_mcp_proxy_password(
        &self,
        server_name: String,
        password: String,
    ) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_mcp_proxy_password(&server_name, &password)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to set proxy password: {e}")))
    }

    /// Set the global proxy password (stored in keychain).
    #[napi]
    pub async fn set_global_proxy_password(&self, password: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_global_proxy_password(&password)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to set proxy password: {e}")))
    }

    /// Test an MCP server connection.
    #[napi]
    pub async fn test_mcp_connection(
        &self,
        name: String,
    ) -> napi::Result<JsMcpConnectionTestResult> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.test_mcp_connection(&name).await.into())
    }

    // ========================
    // Proxy Management
    // ========================

    /// List all named proxies.
    #[napi]
    pub async fn list_proxies(&self) -> napi::Result<Vec<JsProxyInfo>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.list_proxies().await.into_iter().map(Into::into).collect())
    }

    /// Get a named proxy configuration.
    #[napi]
    pub async fn get_proxy(&self, name: String) -> napi::Result<Option<JsProxyConfig>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.get_proxy(&name).await.map(Into::into))
    }

    /// Create or update a named proxy.
    #[napi]
    pub async fn set_proxy(&self, name: String, config: JsProxyConfig) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_proxy(&name, config.into())
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to set proxy: {e}")))
    }

    /// Delete a named proxy.
    #[napi]
    pub async fn delete_proxy(&self, name: String) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.delete_proxy(&name)
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to delete proxy: {e}")))
    }

    /// Get the global proxy configuration.
    #[napi]
    pub async fn get_global_proxy_config(&self) -> napi::Result<JsProxyConfig> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.get_global_proxy_config().await.into())
    }

    /// Set the global proxy configuration.
    #[napi]
    pub async fn set_global_proxy_config(&self, config: JsProxyConfig) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.set_global_proxy_config(config.into())
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to set proxy config: {e}")))
    }

    // ========================
    // Plan Mode
    // ========================

    /// Enter plan mode.
    #[napi]
    pub async fn enter_plan_mode(&self, plan_file: Option<String>) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.enter_plan_mode(plan_file.map(std::path::PathBuf::from)).await;
        Ok(())
    }

    /// Exit plan mode.
    #[napi]
    pub async fn exit_plan_mode(&self) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.exit_plan_mode().await;
        Ok(())
    }

    /// Check if plan mode is active.
    #[napi]
    pub async fn is_plan_mode_active(&self) -> napi::Result<bool> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.is_plan_mode_active())
    }

    /// Get the plan file path (if in plan mode).
    #[napi]
    pub async fn plan_file_path(&self) -> napi::Result<Option<String>> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        Ok(sdk.plan_file_path().await.map(|p| p.to_string_lossy().to_string()))
    }

    // ========================
    // Tool Registration
    // ========================

    /// Register the task management tool.
    #[napi]
    pub async fn register_task_tool(&self) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.register_task_tool()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to register task tool: {e}")))
    }

    /// Register the batch processing tool.
    #[napi]
    pub async fn register_batch_tool(&self) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.register_batch_tool()
            .await
            .map_err(|e| napi::Error::from_reason(format!("Failed to register batch tool: {e}")))
    }

    /// Register the git tool.
    #[napi]
    pub async fn register_git_tool(&self) -> napi::Result<()> {
        let guard = self.inner.read().await;
        let sdk = guard.as_ref().ok_or_else(|| napi::Error::from_reason("SDK not initialized"))?;
        sdk.register_git_tool().await;
        Ok(())
    }
}
