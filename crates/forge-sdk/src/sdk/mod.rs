//! Core `ForgeSDK` implementation.
//!
//! The main entry point for driving agent sessions from any frontend.

mod confirmation;
mod mcp;
mod memory;
mod persona;
mod process;
mod proxy;
mod session;
mod tools;

use crate::config::ForgeConfig;
use crate::error::{ForgeError, Result};
use crate::event::AgentEvent;
use crate::event::AgentEventExt;
use crate::extensions::skill::{
    parse_slash_command, SkillRegistry, SkillRegistryBuilder, SkillSource,
};
use crate::session::{SessionId, SessionSummary};
use crate::types::{
    EventDispatchMode, McpServerInfo, McpServerStatus, McpStatus, McpToolInfo, McpTransportType,
    MemoryScope, ModelSwitchResult, ProcessOptions, RequestId, SessionStatus, ToolInfo,
};
use async_trait::async_trait;
use chrono::Utc;
use forge_agent::{
    AgentConfig, CancellationToken, ConfirmationHandler, ConfirmationLevel, CoreAgent,
    HistoryMessage, RealTaskExecutor, SubAgentSecurity, ToolExecutor,
};
use forge_domain::event::SessionContext;
use forge_llm::{ChatMessage, ChatRole, LlmConfig, LlmEvent, MessageContent, ProviderRegistry};
use forge_mcp::security::McpSecurity;
use forge_mcp::{McpConfig, McpManager, McpTransportType as ToolsTransportType};
use forge_prompt::PromptManager;
use forge_prompt::SkillInfo;
use forge_session::{
    CompressionResult, CompressionStrategy, ContextConfig, ContextManager, Message, Session,
    SessionConfig, SessionManager, COMPRESSION_PROMPT,
};
use forge_tools::builtin::task::{TaskState, TaskTool};
use forge_tools::{BackgroundTaskManager, Tool, ToolContext, ToolRegistry};
use futures::future::BoxFuture;
use futures::Stream;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::Sleep;
use tokio_stream::StreamExt;
use uuid::Uuid;

/// Pending tool confirmation awaiting user response.
#[derive(Debug)]
pub(crate) struct PendingConfirmation {
    pub(crate) response_tx: tokio::sync::oneshot::Sender<bool>,
}

/// Key for tracking in-flight requests.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RequestKey {
    pub(crate) session_id: SessionId,
    pub(crate) request_id: RequestId,
}

/// Key for pending confirmations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[allow(clippy::struct_field_names)]
pub(crate) struct ConfirmationKey {
    pub(crate) session_id: SessionId,
    pub(crate) request_id: RequestId,
    pub(crate) confirmation_id: String,
}

/// Cached environment variables for tool execution.
#[derive(Debug, Default)]
pub(crate) struct EnvCache {
    pub(crate) policy: forge_config::EnvPolicy,
    pub(crate) env: HashMap<String, String>,
    pub(crate) initialized: bool,
}

/// Cached memory file metadata.
#[derive(Debug, Default, Clone)]
pub(crate) struct CachedMemoryFile {
    pub(crate) mtime: Option<std::time::SystemTime>,
    pub(crate) content: Option<String>,
}

/// Cache for memory prompt content (user + project scopes).
#[derive(Debug, Default)]
pub(crate) struct MemoryPromptCache {
    pub(crate) user: CachedMemoryFile,
    pub(crate) projects: HashMap<PathBuf, CachedMemoryFile>,
    pub(crate) user_structured: CachedMemoryFile,
    pub(crate) projects_structured: HashMap<PathBuf, CachedMemoryFile>,
}

/// RAII guard for config file lock.
pub(crate) struct ConfigFileLockGuard {
    path: PathBuf,
}

impl Drop for ConfigFileLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The main Forge SDK handle.
///
/// Manages configuration, providers, sessions, tools, and the agent loop.
/// Constructed via [`crate::ForgeSDKBuilder`].
pub struct ForgeSDK {
    /// SDK configuration (wrapped for interior mutability).
    pub(crate) config: Arc<RwLock<ForgeConfig>>,
    /// Prompt manager (persona, system prompt, etc.).
    pub(crate) prompt_manager: Arc<RwLock<PromptManager>>,
    /// Skill registry (builtin + user + project skills).
    pub(crate) skill_registry: Arc<SkillRegistry>,
    /// Session manager (persistence backend).
    pub(crate) session_manager: Arc<dyn SessionManager>,
    /// Currently active session.
    pub(crate) active_session: Arc<RwLock<Option<Session>>>,
    /// LLM provider registry (multi-provider routing).
    pub(crate) provider_registry: ProviderRegistry,
    /// Tool registry (built-in + custom + MCP tools).
    pub(crate) tool_registry: Arc<RwLock<ToolRegistry>>,
    /// Dirty flag (session has unsaved changes).
    pub(crate) is_dirty: Arc<RwLock<bool>>,
    /// Pending tool confirmations.
    pub(crate) pending_confirmations: Arc<RwLock<HashMap<ConfirmationKey, PendingConfirmation>>>,
    /// In-flight requests for cancellation.
    pub(crate) inflight_requests:
        Arc<RwLock<HashMap<RequestKey, Arc<parking_lot::Mutex<CancellationToken>>>>>,
    /// Most recently started request (for legacy abort routing).
    pub(crate) last_request: Arc<RwLock<Option<RequestKey>>>,
    /// MCP server status (for reporting).
    pub(crate) mcp_servers: Arc<RwLock<Vec<McpServerInfo>>>,
    /// Background task manager.
    pub(crate) background_manager: Arc<BackgroundTaskManager>,
    /// Plan mode active flag.
    pub(crate) plan_mode_flag: Arc<AtomicBool>,
    /// Current plan file path.
    pub(crate) plan_file_path: Arc<RwLock<Option<PathBuf>>>,
    /// Paths confirmed by user (allowed outside working directory).
    pub(crate) confirmed_paths: Arc<RwLock<HashSet<PathBuf>>>,
    /// Root directory for user-level memory storage.
    pub(crate) memory_dir: PathBuf,
    /// Cached environment variables for tool execution.
    pub(crate) env_cache: Arc<RwLock<EnvCache>>,
    /// Cached memory prompts.
    pub(crate) memory_prompt_cache: Arc<RwLock<MemoryPromptCache>>,
    /// `SubAgent` security settings.
    pub(crate) subagent_security: Arc<parking_lot::RwLock<SubAgentSecurity>>,
    /// Shared allowed-subagent list for the task tool schema.
    pub(crate) task_tool_allowed_subagents: Arc<parking_lot::RwLock<Option<Vec<String>>>>,
    /// Per-session persistence mutex.
    pub(crate) session_persist_locks: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// Process-local lock for config.toml read-modify-write.
    pub(crate) config_persist_lock: Arc<tokio::sync::Mutex<()>>,
    /// LSP manager for code intelligence tools.
    pub(crate) lsp_manager: Arc<forge_lsp::LspManager>,
    /// Trace writer for session recording.
    pub(crate) trace_writer: Option<Arc<forge_agent::trace_writer::TraceWriter>>,
    /// Session ID for tracing.
    pub(crate) session_id: String,
    /// Session start time.
    pub(crate) start_time: std::time::Instant,
}

// ===================================================================
// Construction
// ===================================================================

impl ForgeSDK {
    /// Create an SDK instance from configuration.
    ///
    /// **Note:** Uses a temporary tokio runtime internally.
    /// If already in an async context, use [`new_async`](Self::new_async) instead.
    ///
    /// # Errors
    ///
    /// Returns error if configuration is invalid or initialization fails.
    pub fn new(config: ForgeConfig) -> Result<Self> {
        crate::ForgeSDKBuilder::new()
            .working_dir(&config.working_dir)
            .model(&config.llm.model)
            .provider_name(&config.llm.provider)
            .with_builtin_tools()
            .build()
    }

    /// Async variant of [`new`](Self::new).
    ///
    /// Use this when already inside a tokio runtime to avoid nested-runtime panics.
    ///
    /// # Errors
    ///
    /// Returns error if configuration is invalid or initialization fails.
    pub async fn new_async(config: ForgeConfig) -> Result<Self> {
        crate::ForgeSDKBuilder::new()
            .working_dir(&config.working_dir)
            .model(&config.llm.model)
            .provider_name(&config.llm.provider)
            .with_builtin_tools()
            .build_async()
            .await
    }

    /// Create an SDK instance with default configuration.
    ///
    /// **Note:** Uses a temporary tokio runtime internally.
    /// If already in an async context, use [`with_defaults_async`](Self::with_defaults_async) instead.
    ///
    /// # Errors
    ///
    /// Returns error if initialization fails.
    pub fn with_defaults() -> Result<Self> {
        crate::ForgeSDKBuilder::new().with_builtin_tools().build()
    }

    /// Async variant of [`with_defaults`](Self::with_defaults).
    ///
    /// # Errors
    ///
    /// Returns error if initialization fails.
    pub async fn with_defaults_async() -> Result<Self> {
        crate::ForgeSDKBuilder::new().with_builtin_tools().build_async().await
    }

    /// Build an SDK instance from builder parts.
    ///
    /// Called by [`crate::ForgeSDKBuilder::build`].
    ///
    /// # Errors
    ///
    /// Returns error if config validation, session manager creation,
    /// or prompt manager initialization fails.
    pub(crate) fn from_builder(
        config: ForgeConfig,
        provider: Option<Arc<dyn forge_llm::LlmProvider>>,
        session_manager: Option<Arc<dyn SessionManager>>,
        custom_tools: Vec<Arc<dyn Tool>>,
        use_builtin_tools: bool,
        memory_dir: PathBuf,
    ) -> Result<Self> {
        let sdk = Self::create_sdk_core(config, provider, session_manager, memory_dir)?;

        // Register tools synchronously via a temporary runtime.
        // NOTE: This will panic if called from within an existing tokio runtime.
        // Use `from_builder_async` instead when already in an async context.
        if use_builtin_tools || !custom_tools.is_empty() {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| ForgeError::StorageError(format!("Runtime creation failed: {e}")))?;

            rt.block_on(async {
                if use_builtin_tools {
                    sdk.register_builtin_tools().await;
                }
                for tool in custom_tools {
                    sdk.register_tool(tool).await;
                }
            });
        }

        Ok(sdk)
    }

    /// Async variant of [`from_builder`](Self::from_builder).
    ///
    /// Use this when already inside a tokio runtime to avoid nested-runtime panics.
    ///
    /// # Errors
    ///
    /// Returns error if config validation, session manager creation,
    /// or prompt manager initialization fails.
    pub(crate) async fn from_builder_async(
        config: ForgeConfig,
        provider: Option<Arc<dyn forge_llm::LlmProvider>>,
        session_manager: Option<Arc<dyn SessionManager>>,
        custom_tools: Vec<Arc<dyn Tool>>,
        use_builtin_tools: bool,
        memory_dir: PathBuf,
    ) -> Result<Self> {
        let mut sdk = Self::create_sdk_core(config, provider, session_manager, memory_dir)?;

        // Initialize trace writer if enabled
        if sdk.config.read().await.tracing.enabled {
            let config_guard = sdk.config.read().await;
            let tracing_config = &config_guard.tracing;

            // Cleanup old traces
            let _ = tracing_config.cleanup_old_traces().await;

            let output_path = tracing_config.generate_path(&sdk.session_id);

            match forge_agent::trace_writer::TraceWriter::new(
                output_path,
                tracing_config.buffer_size,
                50, // batch_size
            ).await {
                Ok(writer) => {
                    let writer = Arc::new(writer);

                    // Get git information
                    let git_branch = std::process::Command::new("git")
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .current_dir(&config_guard.working_dir)
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map(|s| s.trim().to_string());

                    let git_commit = std::process::Command::new("git")
                        .args(["rev-parse", "HEAD"])
                        .current_dir(&config_guard.working_dir)
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .map(|s| s.trim().to_string());

                    // Record SessionStart event
                    let _ = writer.record(forge_domain::AgentEvent::SessionStart {
                        session_id: sdk.session_id.clone(),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                        context: SessionContext {
                            engine_version: env!("CARGO_PKG_VERSION").to_string(),
                            working_dir: config_guard.working_dir.to_string_lossy().to_string(),
                            git_branch,
                            git_commit,
                            model: config_guard.llm.model.clone(),
                            config_summary: serde_json::json!({
                                "model": config_guard.llm.model,
                                "max_tokens": config_guard.llm.max_tokens,
                            }),
                        },
                    });

                    sdk.trace_writer = Some(writer);
                }
                Err(e) => {
                    eprintln!("Failed to initialize trace writer: {}", e);
                }
            }
        }

        if use_builtin_tools {
            sdk.register_builtin_tools().await;
        }
        for tool in custom_tools {
            sdk.register_tool(tool).await;
        }

        Ok(sdk)
    }

    /// Shared SDK construction logic (no tool registration).
    fn create_sdk_core(
        config: ForgeConfig,
        provider: Option<Arc<dyn forge_llm::LlmProvider>>,
        session_manager: Option<Arc<dyn SessionManager>>,
        memory_dir: PathBuf,
    ) -> Result<Self> {
        config.validate()?;

        // Provider registry
        let mut provider_registry = ProviderRegistry::new();
        if let Some(p) = provider {
            provider_registry.register(p);
        } else {
            Self::register_standard_providers(&mut provider_registry, &config);
        }

        // Session manager
        let session_mgr: Arc<dyn SessionManager> = if let Some(sm) = session_manager {
            sm
        } else {
            Arc::new(
                forge_session::FileSessionManager::new(forge_infra::data_dir().join("sessions"))
                    .map_err(|e| ForgeError::StorageError(e.to_string()))?
                    .with_persistence_format(config.session.persistence_format),
            )
        };

        // Prompt manager
        let prompt_manager = Self::init_prompt_manager(&config)?;

        // Sub-agent security
        let initial_security = Self::build_subagent_security(&config, &prompt_manager);
        let subagent_security = Arc::new(parking_lot::RwLock::new(initial_security.clone()));

        // Skill registry
        let skill_registry = Self::init_skill_registry(&config, &prompt_manager)?;

        // Tool registry
        let tool_registry = ToolRegistry::new();

        // LSP manager
        let lsp_manager = Arc::new(forge_lsp::LspManager::new(config.working_dir.clone()));

        let sdk = Self {
            config: Arc::new(RwLock::new(config)),
            prompt_manager: Arc::new(RwLock::new(prompt_manager)),
            skill_registry: Arc::new(skill_registry),
            session_manager: session_mgr,
            active_session: Arc::new(RwLock::new(None)),
            provider_registry,
            tool_registry: Arc::new(RwLock::new(tool_registry)),
            is_dirty: Arc::new(RwLock::new(false)),
            pending_confirmations: Arc::new(RwLock::new(HashMap::new())),
            inflight_requests: Arc::new(RwLock::new(HashMap::new())),
            last_request: Arc::new(RwLock::new(None)),
            mcp_servers: Arc::new(RwLock::new(Vec::new())),
            background_manager: Arc::new(BackgroundTaskManager::new()),
            plan_mode_flag: Arc::new(AtomicBool::new(false)),
            plan_file_path: Arc::new(RwLock::new(None)),
            confirmed_paths: Arc::new(RwLock::new(HashSet::new())),
            memory_dir,
            env_cache: Arc::new(RwLock::new(EnvCache::default())),
            memory_prompt_cache: Arc::new(RwLock::new(MemoryPromptCache::default())),
            subagent_security,
            task_tool_allowed_subagents: Arc::new(parking_lot::RwLock::new(
                initial_security.enabled_subagents.clone(),
            )),
            session_persist_locks: Arc::new(RwLock::new(HashMap::new())),
            config_persist_lock: Arc::new(tokio::sync::Mutex::new(())),
            lsp_manager,
            trace_writer: None,
            session_id: Uuid::new_v4().to_string(),
            start_time: std::time::Instant::now(),
        };

        Ok(sdk)
    }
}

// ===================================================================
// Private initialization helpers
// ===================================================================

impl ForgeSDK {
    /// Register standard LLM providers based on config.
    fn register_standard_providers(registry: &mut ProviderRegistry, config: &ForgeConfig) {
        let api_key = config
            .llm
            .api_key
            .clone()
            .or_else(|| std::env::var("FORGE_LLM_API_KEY").ok())
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok());

        let base_url =
            config.llm.base_url.clone().or_else(|| std::env::var("FORGE_LLM_BASE_URL").ok());

        if let Some(key) = api_key {
            let provider: Arc<dyn forge_llm::LlmProvider> = match config.llm.provider.as_str() {
                "openai" => {
                    let mut p = forge_llm::OpenAIProvider::new(&key);
                    if let Some(url) = &base_url {
                        p = p.with_base_url(url);
                    }
                    Arc::new(p)
                }
                "gemini" => Arc::new(forge_llm::GeminiProvider::new(&key)),
                "ollama" => {
                    let url = base_url.as_deref().unwrap_or("http://localhost:11434");
                    Arc::new(forge_llm::OllamaProvider::with_base_url(url))
                }
                // Default to Anthropic
                _ => {
                    let mut p = forge_llm::AnthropicProvider::new(&key);
                    if let Some(url) = &base_url {
                        p = p.with_base_url(url);
                    }
                    Arc::new(p)
                }
            };
            registry.register(provider);
        }
    }

    /// Initialize prompt manager from config.
    fn init_prompt_manager(config: &ForgeConfig) -> Result<PromptManager> {
        let mut pm = if let Some(ref prompts_dir) = config.prompts_dir {
            PromptManager::from_dir(prompts_dir)?
        } else {
            PromptManager::new()
        };

        if pm.list_personas().contains(&config.default_persona.as_str()) {
            let _ = pm.set_persona(&config.default_persona);
        }

        Ok(pm)
    }

    /// Build sub-agent security settings from config + current persona.
    fn build_subagent_security(
        config: &ForgeConfig,
        prompt_manager: &PromptManager,
    ) -> SubAgentSecurity {
        let persona = prompt_manager.get_current_persona();
        let mut disabled_tools = config.tools.disabled.clone();
        let bash_readonly = persona.is_some_and(|p| p.options.bash_readonly);
        let enabled_subagents = persona.and_then(|p| p.enabled_subagents.clone());
        if let Some(persona) = persona {
            disabled_tools.extend(persona.disabled_tools.clone());
        }
        disabled_tools.sort();
        disabled_tools.dedup();

        SubAgentSecurity { bash_readonly, disabled_tools, enabled_subagents }
    }

    /// Initialize skill registry from config + loaded prompts directory.
    fn init_skill_registry(
        config: &ForgeConfig,
        prompt_manager: &PromptManager,
    ) -> Result<SkillRegistry> {
        let builtin_skills_dir = prompt_manager.prompts_dir().map(|p| p.join("skills"));
        let user_skills_dir = forge_infra::data_dir().join("skills");
        let project_skills_dir =
            config.trust_project_skills.then(|| config.working_dir.join(".forge/skills"));

        let mut builder = SkillRegistryBuilder::new();
        if let Some(dir) = builtin_skills_dir {
            builder = builder.builtin_path(dir);
        }
        builder = builder.user_path(user_skills_dir);
        if let Some(dir) = project_skills_dir {
            builder = builder.project_path(dir);
        }

        Ok(builder.build()?)
    }

    pub(super) async fn refresh_subagent_security(&self) {
        let config = self.config.read().await;
        let pm = self.prompt_manager.read().await;
        let security = Self::build_subagent_security(&config, &pm);
        drop(pm);
        drop(config);
        // Update enforcement first, then schema (avoid window where schema
        // shows new restrictions but runtime still uses old ones)
        *self.subagent_security.write() = security.clone();
        *self.task_tool_allowed_subagents.write() = security.enabled_subagents;
    }

    pub(super) fn resolve_context_limit_for_model(&self, model: &str) -> usize {
        self.provider_registry
            .get_for_model(model)
            .map(|provider| provider.context_limit(model))
            .unwrap_or(200_000)
    }

    pub(super) fn build_context_manager_for_model(&self, model: &str) -> ContextManager {
        let context_config = ContextConfig {
            max_tokens: self.resolve_context_limit_for_model(model),
            ..Default::default()
        };
        ContextManager::new(context_config).with_strategy(CompressionStrategy::Summarize)
    }

    pub(super) fn read_toml_doc_or_empty(path: &std::path::Path) -> Result<toml_edit::DocumentMut> {
        if !path.exists() {
            return Ok(toml_edit::DocumentMut::new());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| ForgeError::ConfigError(format!("Failed to read config: {e}")))?;
        content
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| ForgeError::ConfigError(format!("Failed to parse config: {e}")))
    }

    pub(super) fn write_string_atomic(
        path: &std::path::Path,
        content: &str,
        target: &str,
    ) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ForgeError::StorageError(format!("Failed to create {target} directory: {e}"))
            })?;
        }
        let tmp_suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp_path = path.with_extension(format!("tmp.{tmp_suffix}"));
        std::fs::write(&tmp_path, content).map_err(|e| {
            ForgeError::StorageError(format!(
                "Failed to write temporary {target} file {}: {e}",
                tmp_path.display()
            ))
        })?;
        std::fs::rename(&tmp_path, path).map_err(|e| {
            ForgeError::StorageError(format!(
                "Failed to atomically replace {target} file {}: {e}",
                path.display()
            ))
        })
    }

    pub(super) async fn acquire_config_file_lock(
        config_path: &std::path::Path,
    ) -> Result<ConfigFileLockGuard> {
        const MAX_ATTEMPTS: usize = 500;
        const WAIT_MS: u64 = 20;
        const STALE_LOCK_SECS: u64 = 300;

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ForgeError::StorageError(format!("Failed to create config directory for lock: {e}"))
            })?;
        }

        let lock_path = config_path.with_extension("lock");
        for _ in 0..MAX_ATTEMPTS {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lock_path) {
                Ok(mut lock_file) => {
                    let _ = std::io::Write::write_all(
                        &mut lock_file,
                        format!("pid={}\n", std::process::id()).as_bytes(),
                    );
                    return Ok(ConfigFileLockGuard { path: lock_path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Ok(metadata) = std::fs::metadata(&lock_path) {
                        if let Ok(modified) = metadata.modified() {
                            if modified.elapsed().unwrap_or_else(|_| Duration::from_secs(0))
                                >= Duration::from_secs(STALE_LOCK_SECS)
                            {
                                let _ = std::fs::remove_file(&lock_path);
                                continue;
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(WAIT_MS)).await;
                    continue;
                }
                Err(e) => {
                    return Err(ForgeError::StorageError(format!(
                        "Failed to acquire config file lock '{}': {e}",
                        lock_path.display()
                    )));
                }
            }
        }

        Err(ForgeError::StorageError(format!(
            "Timed out acquiring config file lock '{}'",
            lock_path.display()
        )))
    }

    /// Register all built-in tools.
    async fn register_builtin_tools(&self) {
        use forge_tools::builtin::{
            ask_user::AskUserQuestionTool, edit::EditTool, glob::GlobTool, grep::GrepTool,
            read::ReadTool, shell::get_shell_tools, todo::TodoWriteTool, web_fetch::WebFetchTool,
            web_search::WebSearchTool, write::WriteTool, EnterPlanModeTool, ExitPlanModeTool,
            KillShellTool, MemoryManageTool, MemoryReadTool, MemoryWriteTool, SkillTool,
            TaskOutputTool,
        };
        use forge_tools_coding::symbols::SymbolsTool;

        let config = self.config.read().await;
        let working_dir = config.working_dir.clone();
        let trust_project = config.trust_project_skills;
        let proxy_config = config.tools.proxy.clone();
        let search_provider = config.tools.search_provider.clone();
        drop(config);

        let user_memory_dir = self.memory_dir.join("memory");

        // File system tools
        self.register_tool(Arc::new(ReadTool)).await;
        self.register_tool(Arc::new(WriteTool)).await;
        self.register_tool(Arc::new(EditTool)).await;
        self.register_tool(Arc::new(GlobTool)).await;
        self.register_tool(Arc::new(GrepTool)).await;
        self.register_tool(Arc::new(SymbolsTool)).await;

        // Shell tools (platform-specific)
        for tool in get_shell_tools() {
            self.register_tool(tool).await;
        }

        // Web tools (with proxy config from tools.proxy)
        self.register_tool(Arc::new(WebFetchTool::with_proxy(&proxy_config))).await;
        self.register_tool(Arc::new(WebSearchTool::from_settings(
            search_provider.as_deref(),
            Some(&proxy_config),
        )))
        .await;

        // Background task tools
        self.register_tool(Arc::new(TaskOutputTool::new())).await;
        self.register_tool(Arc::new(KillShellTool::new(self.background_manager.clone()))).await;

        // Plan mode tools
        self.register_tool(Arc::new(EnterPlanModeTool)).await;
        self.register_tool(Arc::new(ExitPlanModeTool)).await;

        // Skills tool
        self.register_tool(Arc::new(SkillTool::new())).await;

        // Interaction tools
        self.register_tool(Arc::new(AskUserQuestionTool::new())).await;
        self.register_tool(Arc::new(TodoWriteTool::with_new_state())).await;

        // Memory tools
        self.register_tool(Arc::new(MemoryReadTool::new(user_memory_dir.clone()))).await;
        self.register_tool(Arc::new(MemoryWriteTool::new(user_memory_dir.clone()))).await;
        self.register_tool(Arc::new(MemoryManageTool::new(user_memory_dir))).await;

        // Script plugins
        if trust_project {
            for tool in forge_tools::plugin::load_all_plugins(&working_dir) {
                self.register_tool(tool).await;
            }
        } else {
            let user_dir = forge_infra::data_dir().join("tools");
            for tool in forge_tools::plugin::load_plugins(&user_dir) {
                self.register_tool(tool).await;
            }
        }
    }
}

// ===================================================================
// Process-internal types (stream wrappers, confirmation handler, etc.)
// ===================================================================

#[derive(Debug, Default)]
struct PendingToolCall {
    name: String,
    input: serde_json::Value,
}

/// Handle for an in-flight request in a specific session.
pub struct ProcessHandle {
    /// Session ID
    pub session_id: SessionId,
    /// Request ID for cancellation/confirmation routing
    pub request_id: RequestId,
    /// Event stream
    pub stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
}

#[derive(Debug, Default)]
struct PersistState {
    pending_messages: Vec<Message>,
    current_text: String,
    tool_call_order: Vec<String>,
    tool_calls: HashMap<String, PendingToolCall>,
    tool_results: Vec<forge_session::ContentBlock>,
}

impl PersistState {
    fn record_event(&mut self, event: &AgentEvent) {
        if self.should_flush_before(event) {
            self.flush_tool_batch();
        }
        match event {
            AgentEvent::TextDelta { delta } => self.current_text.push_str(delta),
            AgentEvent::ToolCallStart { id, name, input } => {
                self.record_tool_call(id, name, input.clone());
            }
            AgentEvent::ToolExecuting { id, name, input } => {
                self.record_tool_call(id, name, input.clone());
            }
            AgentEvent::ConfirmationRequired { id, tool, params, .. } => {
                self.record_tool_call(id, tool, params.clone());
            }
            AgentEvent::ToolResult { id, output, is_error } => {
                self.record_tool_result(id, output, *is_error);
            }
            _ => {}
        }
    }

    fn should_flush_before(&self, event: &AgentEvent) -> bool {
        if self.tool_results.is_empty() {
            return false;
        }
        matches!(
            event,
            AgentEvent::TextDelta { .. }
                | AgentEvent::ThinkingStart
                | AgentEvent::Thinking { .. }
                | AgentEvent::ToolCallStart { .. }
                | AgentEvent::Retrying { .. }
        )
    }

    fn record_tool_call(&mut self, id: &str, name: &str, input: serde_json::Value) {
        let entry = self.tool_calls.entry(id.to_string()).or_insert_with(|| PendingToolCall {
            name: name.to_string(),
            input: serde_json::Value::Null,
        });
        if entry.name.is_empty() {
            entry.name = name.to_string();
        }
        if !input.is_null() {
            entry.input = input;
        }
        if !self.tool_call_order.iter().any(|existing| existing == id) {
            self.tool_call_order.push(id.to_string());
        }
    }

    fn record_tool_result(&mut self, id: &str, output: &str, is_error: bool) {
        if !self.tool_calls.contains_key(id) {
            self.tool_calls.insert(
                id.to_string(),
                PendingToolCall { name: String::new(), input: serde_json::Value::Null },
            );
            self.tool_call_order.push(id.to_string());
        }
        self.tool_results.push(forge_session::ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: output.to_string(),
            is_error,
        });
    }

    fn flush_tool_batch(&mut self) {
        if !self.tool_calls.is_empty() || !self.current_text.is_empty() {
            let content = if self.tool_calls.is_empty() {
                forge_session::MessageContent::Text(self.current_text.clone())
            } else {
                let mut blocks = Vec::new();
                if !self.current_text.is_empty() {
                    blocks.push(forge_session::ContentBlock::Text {
                        text: self.current_text.clone(),
                    });
                }
                for id in &self.tool_call_order {
                    if let Some(call) = self.tool_calls.get(id) {
                        let name = if call.name.is_empty() {
                            "unknown".to_string()
                        } else {
                            call.name.clone()
                        };
                        blocks.push(forge_session::ContentBlock::ToolUse {
                            id: id.clone(),
                            name,
                            input: call.input.clone(),
                        });
                    }
                }
                forge_session::MessageContent::Blocks(blocks)
            };
            self.pending_messages.push(Message {
                role: forge_session::MessageRole::Assistant,
                content,
                timestamp: Utc::now(),
            });
        }
        if !self.tool_results.is_empty() {
            self.pending_messages.push(Message {
                role: forge_session::MessageRole::User,
                content: forge_session::MessageContent::Blocks(self.tool_results.clone()),
                timestamp: Utc::now(),
            });
        }
        self.current_text.clear();
        self.tool_calls.clear();
        self.tool_call_order.clear();
        self.tool_results.clear();
    }

    fn finalize_for_persist(&mut self) -> Vec<Message> {
        if !self.tool_calls.is_empty() || !self.tool_results.is_empty() {
            self.flush_tool_batch();
        }
        if !self.current_text.is_empty() {
            self.pending_messages.push(Message::assistant(self.current_text.clone()));
            self.current_text.clear();
        }
        std::mem::take(&mut self.pending_messages)
    }
}

fn cleanup_request_state(
    request_key: &RequestKey,
    inflight_requests: &Arc<
        RwLock<HashMap<RequestKey, Arc<parking_lot::Mutex<CancellationToken>>>>,
    >,
    pending_confirmations: &Arc<RwLock<HashMap<ConfirmationKey, PendingConfirmation>>>,
) {
    let inflight_cleaned = if let Ok(mut guard) = inflight_requests.try_write() {
        guard.remove(request_key);
        true
    } else {
        false
    };
    let pending_cleaned = if let Ok(mut guard) = pending_confirmations.try_write() {
        guard.retain(|k, _| {
            !(k.session_id == request_key.session_id && k.request_id == request_key.request_id)
        });
        true
    } else {
        false
    };

    if inflight_cleaned && pending_cleaned {
        return;
    }

    let key = request_key.clone();
    let inflight = inflight_requests.clone();
    let pending = pending_confirmations.clone();

    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(async move {
            inflight.write().await.remove(&key);
            pending
                .write()
                .await
                .retain(|k, _| !(k.session_id == key.session_id && k.request_id == key.request_id));
        });
    } else {
        if !inflight_cleaned {
            inflight_requests.blocking_write().remove(request_key);
        }
        if !pending_cleaned {
            pending_confirmations.blocking_write().retain(|k, _| {
                !(k.session_id == request_key.session_id && k.request_id == request_key.request_id)
            });
        }
    }
}

/// Stream wrapper that auto-saves assistant response to session.
struct AutoSaveStream {
    inner: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
    persist_state: PersistState,
    active_session: Arc<RwLock<Option<Session>>>,
    session_manager: Arc<dyn SessionManager>,
    saved: bool,
    is_dirty: Arc<RwLock<bool>>,
    plan_mode_flag: Arc<AtomicBool>,
    plan_file_path: Arc<RwLock<Option<PathBuf>>>,
    pending_persist: Option<BoxFuture<'static, Option<String>>>,
    queued_event: Option<AgentEvent>,
    end_after_persist: bool,
    request_key: RequestKey,
    inflight_requests: Arc<RwLock<HashMap<RequestKey, Arc<parking_lot::Mutex<CancellationToken>>>>>,
    pending_confirmations: Arc<RwLock<HashMap<ConfirmationKey, PendingConfirmation>>>,
}

impl Stream for AutoSaveStream {
    type Item = AgentEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(mut fut) = self.pending_persist.take() {
            match fut.as_mut().poll(cx) {
                Poll::Ready(persist_error) => {
                    if let Some(message) = persist_error {
                        self.queued_event = None;
                        self.end_after_persist = false;
                        cleanup_request_state(
                            &self.request_key,
                            &self.inflight_requests,
                            &self.pending_confirmations,
                        );
                        return Poll::Ready(Some(AgentEvent::Error { message }));
                    }
                    if let Some(event) = self.queued_event.take() {
                        if event.is_terminal() {
                            cleanup_request_state(
                                &self.request_key,
                                &self.inflight_requests,
                                &self.pending_confirmations,
                            );
                        }
                        return Poll::Ready(Some(event));
                    }
                    if self.end_after_persist {
                        self.end_after_persist = false;
                        cleanup_request_state(
                            &self.request_key,
                            &self.inflight_requests,
                            &self.pending_confirmations,
                        );
                        return Poll::Ready(Some(AgentEvent::Done { summary: None }));
                    }
                }
                Poll::Pending => {
                    self.pending_persist = Some(fut);
                    return Poll::Pending;
                }
            }
        }

        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(event)) => {
                self.persist_state.record_event(&event);
                match &event {
                    AgentEvent::PlanModeEntered { plan_file } => {
                        self.plan_mode_flag.store(true, Ordering::Release);
                        let pfp = self.plan_file_path.clone();
                        let pf = plan_file.clone();
                        tokio::spawn(async move {
                            *pfp.write().await = pf.map(PathBuf::from);
                        });
                    }
                    AgentEvent::PlanModeExited { .. } => {
                        self.plan_mode_flag.store(false, Ordering::Release);
                        let pfp = self.plan_file_path.clone();
                        tokio::spawn(async move {
                            *pfp.write().await = None;
                        });
                    }
                    _ => {}
                }
                if event.is_terminal() && !self.saved {
                    self.saved = true;
                    self.queued_event = Some(event);
                    self.end_after_persist = false;
                    self.pending_persist = Some(self.build_persist_future());
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                Poll::Ready(Some(event))
            }
            Poll::Ready(None) => {
                if !self.saved {
                    self.saved = true;
                    self.queued_event = None;
                    self.end_after_persist = true;
                    self.pending_persist = Some(self.build_persist_future());
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                cleanup_request_state(
                    &self.request_key,
                    &self.inflight_requests,
                    &self.pending_confirmations,
                );
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for AutoSaveStream {
    fn drop(&mut self) {
        cleanup_request_state(
            &self.request_key,
            &self.inflight_requests,
            &self.pending_confirmations,
        );
    }
}

impl AutoSaveStream {
    fn build_persist_future(&mut self) -> BoxFuture<'static, Option<String>> {
        use futures::FutureExt;
        let messages = self.persist_state.finalize_for_persist();
        let active_session = self.active_session.clone();
        let session_manager = self.session_manager.clone();
        let is_dirty = self.is_dirty.clone();
        let request_session_id = self.request_key.session_id.clone();

        async move {
            if messages.is_empty() {
                let clear = {
                    let g = active_session.read().await;
                    g.as_ref().is_some_and(|s| s.id.to_string() == request_session_id)
                };
                if clear {
                    *is_dirty.write().await = false;
                }
                return None;
            }

            let snapshot = {
                let mut guard = active_session.write().await;
                if let Some(session) = guard.as_mut() {
                    if session.id.to_string() == request_session_id {
                        for msg in &messages {
                            session.add_message(msg.clone());
                        }
                        Some(session.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            };

            if let Some(session) = snapshot {
                if let Err(e) = session_manager.update(&session).await {
                    return Some(format!("Failed to persist assistant response: {e}"));
                }
                let clear = {
                    let g = active_session.read().await;
                    g.as_ref().is_some_and(|s| s.id.to_string() == request_session_id)
                };
                if clear {
                    *is_dirty.write().await = false;
                }
                return None;
            }

            let session_id = match forge_session::SessionId::parse(&request_session_id) {
                Ok(id) => id,
                Err(e) => return Some(format!("Invalid session id {request_session_id}: {e}")),
            };
            let mut session = match session_manager.get(session_id).await {
                Ok(s) => s,
                Err(e) => return Some(format!("Cannot load session {request_session_id}: {e}")),
            };
            for msg in messages {
                session.add_message(msg);
            }
            if let Err(e) = session_manager.update(&session).await {
                return Some(format!("Cannot update session {request_session_id}: {e}"));
            }
            None
        }
        .boxed()
    }
}

/// Stream wrapper that persists assistant response to a specific session on completion.
struct SessionPersistStream {
    inner: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
    persist_state: PersistState,
    session_manager: Arc<dyn SessionManager>,
    session_id: forge_session::SessionId,
    session_update_lock: Arc<tokio::sync::Mutex<()>>,
    saved: bool,
    plan_mode_flag: Arc<AtomicBool>,
    pending_persist: Option<BoxFuture<'static, Option<String>>>,
    queued_event: Option<AgentEvent>,
    end_after_persist: bool,
    request_key: RequestKey,
    inflight_requests: Arc<RwLock<HashMap<RequestKey, Arc<parking_lot::Mutex<CancellationToken>>>>>,
    pending_confirmations: Arc<RwLock<HashMap<ConfirmationKey, PendingConfirmation>>>,
}

impl SessionPersistStream {
    fn build_persist_future(&mut self) -> BoxFuture<'static, Option<String>> {
        use futures::FutureExt;
        let messages = self.persist_state.finalize_for_persist();
        let session_manager = self.session_manager.clone();
        let session_id = self.session_id;
        let lock = self.session_update_lock.clone();

        async move {
            if messages.is_empty() {
                return None;
            }
            let _guard = lock.lock().await;
            match session_manager.get(session_id).await {
                Ok(mut session) => {
                    for msg in messages {
                        session.add_message(msg);
                    }
                    if let Err(e) = session_manager.update(&session).await {
                        return Some(format!("Failed to persist messages for {session_id}: {e}"));
                    }
                }
                Err(e) => {
                    return Some(format!("Failed to load session {session_id}: {e}"));
                }
            }
            None
        }
        .boxed()
    }
}

impl Stream for SessionPersistStream {
    type Item = AgentEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(mut fut) = self.pending_persist.take() {
            match fut.as_mut().poll(cx) {
                Poll::Ready(persist_error) => {
                    if let Some(message) = persist_error {
                        self.queued_event = None;
                        self.end_after_persist = false;
                        cleanup_request_state(
                            &self.request_key,
                            &self.inflight_requests,
                            &self.pending_confirmations,
                        );
                        return Poll::Ready(Some(AgentEvent::Error { message }));
                    }
                    if let Some(event) = self.queued_event.take() {
                        if event.is_terminal() {
                            cleanup_request_state(
                                &self.request_key,
                                &self.inflight_requests,
                                &self.pending_confirmations,
                            );
                        }
                        return Poll::Ready(Some(event));
                    }
                    if self.end_after_persist {
                        self.end_after_persist = false;
                        cleanup_request_state(
                            &self.request_key,
                            &self.inflight_requests,
                            &self.pending_confirmations,
                        );
                        return Poll::Ready(Some(AgentEvent::Done { summary: None }));
                    }
                }
                Poll::Pending => {
                    self.pending_persist = Some(fut);
                    return Poll::Pending;
                }
            }
        }

        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(event)) => {
                self.persist_state.record_event(&event);
                match &event {
                    AgentEvent::PlanModeEntered { .. } => {
                        self.plan_mode_flag.store(true, Ordering::Release);
                    }
                    AgentEvent::PlanModeExited { .. } => {
                        self.plan_mode_flag.store(false, Ordering::Release);
                    }
                    _ => {}
                }
                if event.is_terminal() && !self.saved {
                    self.saved = true;
                    self.queued_event = Some(event);
                    self.pending_persist = Some(self.build_persist_future());
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                Poll::Ready(Some(event))
            }
            Poll::Ready(None) => {
                if !self.saved {
                    self.saved = true;
                    self.end_after_persist = true;
                    self.pending_persist = Some(self.build_persist_future());
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                cleanup_request_state(
                    &self.request_key,
                    &self.inflight_requests,
                    &self.pending_confirmations,
                );
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for SessionPersistStream {
    fn drop(&mut self) {
        cleanup_request_state(
            &self.request_key,
            &self.inflight_requests,
            &self.pending_confirmations,
        );
    }
}

/// Stream wrapper that only performs request-scoped cleanup.
struct CleanupStream {
    inner: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
    request_key: RequestKey,
    inflight_requests: Arc<RwLock<HashMap<RequestKey, Arc<parking_lot::Mutex<CancellationToken>>>>>,
    pending_confirmations: Arc<RwLock<HashMap<ConfirmationKey, PendingConfirmation>>>,
    cleaned: bool,
}

impl CleanupStream {
    fn cleanup(&mut self) {
        if self.cleaned {
            return;
        }
        self.cleaned = true;
        cleanup_request_state(
            &self.request_key,
            &self.inflight_requests,
            &self.pending_confirmations,
        );
    }
}

impl Stream for CleanupStream {
    type Item = AgentEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(event)) => {
                if event.is_terminal() {
                    self.cleanup();
                }
                Poll::Ready(Some(event))
            }
            Poll::Ready(None) => {
                self.cleanup();
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for CleanupStream {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Stream wrapper that batches TextDelta events.
struct BatchedTextDeltaStream {
    inner: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
    buffer: String,
    max_bytes: usize,
    max_latency: Duration,
    flush_timer: Option<Pin<Box<Sleep>>>,
    queued_event: Option<AgentEvent>,
    end_after_flush: bool,
}

impl BatchedTextDeltaStream {
    fn new(
        inner: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
        max_bytes: usize,
        max_latency: Duration,
    ) -> Self {
        Self {
            inner,
            buffer: String::new(),
            max_bytes: max_bytes.max(1),
            max_latency,
            flush_timer: None,
            queued_event: None,
            end_after_flush: false,
        }
    }

    fn flush_now(&mut self) -> Option<AgentEvent> {
        if self.buffer.is_empty() {
            return None;
        }
        let delta = std::mem::take(&mut self.buffer);
        self.flush_timer = None;
        Some(AgentEvent::TextDelta { delta })
    }
}

impl Stream for BatchedTextDeltaStream {
    type Item = AgentEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(event) = self.queued_event.take() {
            return Poll::Ready(Some(event));
        }
        if let Some(timer) = self.flush_timer.as_mut() {
            if timer.as_mut().poll(cx).is_ready() {
                if let Some(flush) = self.flush_now() {
                    return Poll::Ready(Some(flush));
                }
            }
        }
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(event)) => match event {
                AgentEvent::TextDelta { delta } => {
                    if self.buffer.is_empty() {
                        self.flush_timer = Some(Box::pin(tokio::time::sleep(self.max_latency)));
                    }
                    self.buffer.push_str(&delta);
                    if self.buffer.len() >= self.max_bytes {
                        if let Some(flush) = self.flush_now() {
                            return Poll::Ready(Some(flush));
                        }
                    }
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
                other => {
                    if let Some(flush) = self.flush_now() {
                        self.queued_event = Some(other);
                        return Poll::Ready(Some(flush));
                    }
                    Poll::Ready(Some(other))
                }
            },
            Poll::Ready(None) => {
                if let Some(flush) = self.flush_now() {
                    self.end_after_flush = true;
                    return Poll::Ready(Some(flush));
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

// ===================================================================
// Confirmation handler
// ===================================================================

/// Confirmation handler that bridges agent confirmation requests to SDK.
pub(crate) struct SdkConfirmationHandler {
    pub(crate) session_id: String,
    pub(crate) request_id: String,
    pub(crate) pending_confirmations: Arc<RwLock<HashMap<ConfirmationKey, PendingConfirmation>>>,
    pub(crate) pre_registered:
        Arc<tokio::sync::Mutex<HashMap<String, tokio::sync::oneshot::Receiver<bool>>>>,
}

#[async_trait]
impl ConfirmationHandler for SdkConfirmationHandler {
    async fn request_confirmation(
        &self,
        id: &str,
        _tool: &str,
        _params: &serde_json::Value,
        _level: ConfirmationLevel,
    ) -> forge_agent::Result<bool> {
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut pending = self.pending_confirmations.write().await;
            pending.insert(
                ConfirmationKey {
                    session_id: self.session_id.clone(),
                    request_id: self.request_id.clone(),
                    confirmation_id: id.to_string(),
                },
                PendingConfirmation { response_tx: tx },
            );
        }

        Self::await_confirmation_response(
            id,
            rx,
            &self.session_id,
            &self.request_id,
            &self.pending_confirmations,
        )
        .await
    }

    async fn pre_register(&self, id: &str) {
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            let mut pending = self.pending_confirmations.write().await;
            pending.insert(
                ConfirmationKey {
                    session_id: self.session_id.clone(),
                    request_id: self.request_id.clone(),
                    confirmation_id: id.to_string(),
                },
                PendingConfirmation { response_tx: tx },
            );
        }

        self.pre_registered.lock().await.insert(id.to_string(), rx);
    }

    async fn wait_for_confirmation(
        &self,
        id: &str,
        tool: &str,
        params: &serde_json::Value,
        level: ConfirmationLevel,
    ) -> forge_agent::Result<bool> {
        let rx = self.pre_registered.lock().await.remove(id);
        if let Some(rx) = rx {
            return Self::await_confirmation_response(
                id,
                rx,
                &self.session_id,
                &self.request_id,
                &self.pending_confirmations,
            )
            .await;
        }

        tracing::warn!(
            id = %id,
            "wait_for_confirmation called without pre_register; falling back to request_confirmation"
        );
        self.request_confirmation(id, tool, params, level).await
    }
}

impl SdkConfirmationHandler {
    async fn await_confirmation_response(
        id: &str,
        rx: tokio::sync::oneshot::Receiver<bool>,
        session_id: &str,
        request_id: &str,
        pending_confirmations: &Arc<RwLock<HashMap<ConfirmationKey, PendingConfirmation>>>,
    ) -> forge_agent::Result<bool> {
        match tokio::time::timeout(Duration::from_secs(300), rx).await {
            Ok(Ok(allowed)) => Ok(allowed),
            Ok(Err(_)) => {
                tracing::warn!(id = %id, "Confirmation channel dropped, treating as rejection");
                Ok(false)
            }
            Err(_) => {
                tracing::warn!(id = %id, "Confirmation timed out after 5 minutes, treating as rejection");
                let mut pending = pending_confirmations.write().await;
                pending.remove(&ConfirmationKey {
                    session_id: session_id.to_string(),
                    request_id: request_id.to_string(),
                    confirmation_id: id.to_string(),
                });
                Ok(false)
            }
        }
    }
}

impl ForgeSDK {
    /// Gracefully shutdown SDK and flush trace writer.
    ///
    /// This method ensures all buffered trace events are written to disk
    /// before returning. Always call this method before dropping the SDK
    /// to guarantee no trace data is lost.
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(writer) = self.trace_writer.take() {
            let session_id = self.session_id.clone();
            let duration_ms = self.start_time.elapsed().as_millis() as u64;

            // Record session end event
            let _ = writer.record(forge_domain::AgentEvent::SessionEnd {
                session_id,
                timestamp: chrono::Utc::now().timestamp_millis(),
                duration_ms,
            });

            // Always flush to ensure data is written, even if Arc::try_unwrap fails
            let _ = writer.flush().await;

            // Try to take ownership and shutdown the writer
            if let Ok(writer_owned) = Arc::try_unwrap(writer) {
                writer_owned.shutdown().await.ok();
            }
            // If Arc::try_unwrap fails, the writer will be dropped and
            // TraceWriter::drop will handle graceful shutdown
        }
        Ok(())
    }
}

impl Drop for ForgeSDK {
    fn drop(&mut self) {
        // Best-effort cleanup: record SessionEnd and attempt flush
        // TraceWriter::drop now handles graceful shutdown with timeout
        if let Some(writer) = &self.trace_writer {
            let session_id = self.session_id.clone();
            let duration_ms = self.start_time.elapsed().as_millis() as u64;

            // Try non-blocking record of session end
            let _ = writer.record(forge_domain::AgentEvent::SessionEnd {
                session_id,
                timestamp: chrono::Utc::now().timestamp_millis(),
                duration_ms,
            });

            // Note: We can't call async flush() in Drop, but TraceWriter::drop
            // will now attempt graceful shutdown with a timeout
        }
    }
}
