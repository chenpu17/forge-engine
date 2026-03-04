//! Request processing for ForgeSDK

use super::*;

impl ForgeSDK {
    fn try_expand_skill_invocation(&self, input: &str) -> Option<String> {
        let (name, args) = parse_slash_command(input)?;
        self.skill_registry.expand_user_invocable(&name, &args)
    }

    pub(super) fn expand_user_message_for_llm(&self, content: &str) -> String {
        self.try_expand_skill_invocation(content).unwrap_or_else(|| content.to_string())
    }

    pub(super) fn session_message_to_history(
        &self,
        message: &forge_session::Message,
    ) -> HistoryMessage {
        use forge_session::MessageRole;

        match message.role {
            MessageRole::User => {
                let content = match &message.content {
                    forge_session::MessageContent::Text(text) => {
                        self.expand_user_message_for_llm(text)
                    }
                    _ => Self::format_message_content(&message.content),
                };
                HistoryMessage::user(content)
            }
            MessageRole::Assistant => {
                HistoryMessage::assistant(Self::format_message_content(&message.content))
            }
            MessageRole::System => {
                HistoryMessage::system(Self::format_message_content(&message.content))
            }
        }
    }

    fn expand_history_for_llm(&self, history: &[HistoryMessage]) -> Vec<HistoryMessage> {
        history
            .iter()
            .map(|m| match m.role {
                forge_agent::HistoryRole::User => {
                    HistoryMessage::user(self.expand_user_message_for_llm(&m.content))
                }
                forge_agent::HistoryRole::Assistant => HistoryMessage::assistant(m.content.clone()),
                forge_agent::HistoryRole::System => HistoryMessage::system(m.content.clone()),
            })
            .collect()
    }

    pub(super) async fn reserve_session_request_slot(
        &self,
        request_key: &RequestKey,
        token: Arc<parking_lot::Mutex<CancellationToken>>,
    ) -> Result<()> {
        let mut inflight = self.inflight_requests.write().await;
        if let Some(conflict) =
            inflight.keys().find(|key| key.session_id == request_key.session_id).cloned()
        {
            return Err(ForgeError::SessionBusy {
                session_id: request_key.session_id.clone(),
                inflight_request_id: conflict.request_id,
            });
        }
        inflight.insert(request_key.clone(), token);
        Ok(())
    }

    pub(super) async fn release_session_request_slot(&self, request_key: &RequestKey) {
        self.inflight_requests.write().await.remove(request_key);
    }

    pub(super) async fn cancel_and_remove_session_requests(&self, session_id: &str) -> usize {
        let (request_keys, tokens) = {
            let mut inflight = self.inflight_requests.write().await;
            let keys = inflight
                .keys()
                .filter(|key| key.session_id == session_id)
                .cloned()
                .collect::<Vec<_>>();
            let mut tokens = Vec::with_capacity(keys.len());
            for key in &keys {
                if let Some(token) = inflight.remove(key) {
                    tokens.push(token);
                }
            }
            (keys, tokens)
        };

        for token in &tokens {
            token.lock().cancel();
        }

        if !request_keys.is_empty() {
            let mut pending = self.pending_confirmations.write().await;
            pending.retain(|key, _| key.session_id != session_id);

            let mut last_request = self.last_request.write().await;
            if last_request.as_ref().is_some_and(|r| r.session_id == session_id) {
                *last_request = None;
            }
        }

        tokens.len()
    }

    pub(super) fn convert_agent_stream(
        stream: forge_agent::AgentEventStream,
        require_confirmation: bool,
    ) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
        let mut saw_terminal = false;
        Box::pin(stream.filter_map(move |result| match result {
            Ok(event) => {
                let event = AgentEvent::from(event);
                if !require_confirmation && matches!(event, AgentEvent::ConfirmationRequired { .. })
                {
                    None
                } else {
                    if event.is_terminal() {
                        saw_terminal = true;
                    }
                    Some(event)
                }
            }
            Err(forge_agent::AgentError::Aborted) => {
                if saw_terminal {
                    None
                } else {
                    saw_terminal = true;
                    Some(AgentEvent::Cancelled)
                }
            }
            Err(e) => {
                if saw_terminal {
                    None
                } else {
                    saw_terminal = true;
                    Some(AgentEvent::Error { message: e.to_string() })
                }
            }
        }))
    }

    /// Process user input and return a stream of events.
    ///
    /// Requires an active session (see [`Self::create_session`]).
    pub async fn process(
        &self,
        input: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>> {
        if self.active_session.read().await.is_none() {
            return Err(ForgeError::NoActiveSession);
        }

        let session_id = {
            let guard = self.active_session.read().await;
            let session = guard.as_ref().ok_or(ForgeError::NoActiveSession)?;
            session.id.to_string()
        };
        let request_id = Uuid::new_v4().to_string();
        let request_key =
            RequestKey { session_id: session_id.clone(), request_id: request_id.clone() };

        let config = self.config.read().await;
        let effective_model = config.llm.effective_model();
        let working_dir = config.working_dir.clone();
        let base_project_prompt = config.project_prompt.clone();
        let max_tokens = config.llm.max_tokens;
        let temperature = config.llm.effective_temperature();
        let thinking = config.llm.thinking.clone();
        let thinking_adaptor = config.llm.thinking_adaptor;
        let bash_timeout = config.tools.bash_timeout;
        let max_output_size = config.tools.max_output_size;
        let require_confirmation = config.tools.require_confirmation;
        let trust_level = config.tools.trust.level;
        let mut disabled_tools = config.tools.disabled.clone();
        let _memory_mode = config.tools.memory.effective_mode();
        let permission_rules = config.tools.permission_rules.clone();
        drop(config);

        let project_prompt =
            self.build_project_prompt_with_memory(base_project_prompt, &working_dir).await;
        let (memory_user_index, memory_project_index) =
            self.load_memory_indexes(&working_dir).await;

        let provider = self.provider_registry.get_for_model(&effective_model).ok_or_else(|| {
            ForgeError::Llm(forge_llm::LlmError::ProviderUnavailable(format!(
                "No provider found for model: {effective_model}"
            )))
        })?;

        // Extract persona options
        let pm = self.prompt_manager.read().await;
        let persona_config = pm.get_current_persona();
        let (
            persona_disabled_tools,
            max_iterations_override,
            bash_readonly,
            reflection_enabled,
            max_same_tool_calls_override,
            tool_call_limits_override,
        ) = if let Some(persona) = persona_config {
            (
                persona.disabled_tools.clone(),
                persona.options.max_iterations,
                persona.options.bash_readonly,
                persona.options.reflection_enabled,
                persona.options.max_same_tool_calls,
                if persona.options.tool_call_limits.is_empty() {
                    None
                } else {
                    Some(persona.options.tool_call_limits.clone())
                },
            )
        } else {
            (Vec::new(), None, false, true, None, None)
        };
        drop(pm);

        disabled_tools.extend(persona_disabled_tools);
        disabled_tools.sort();
        disabled_tools.dedup();

        let confirmed_paths = self.confirmed_paths.read().await.clone();
        let tool_context = ToolContext {
            working_dir: working_dir.clone(),
            env: self.resolve_tool_env().await,
            timeout_secs: bash_timeout,
            confirmed_paths,
            bash_readonly,
            lsp_manager: Some(self.lsp_manager.clone()),
            ..ToolContext::default()
        }
        .with_plan_mode_flag(self.plan_mode_flag.clone());

        let tool_registry = self.tool_registry.read().await;
        let filtered_registry = if disabled_tools.is_empty() {
            tool_registry.clone()
        } else {
            let mut filtered = tool_registry.clone();
            for tool_name in &disabled_tools {
                filtered.unregister(tool_name);
            }
            filtered
        };
        let executor = Arc::new(
            ToolExecutor::new(Arc::new(filtered_registry), tool_context)
                .with_timeout(Duration::from_secs(bash_timeout))
                .with_max_output_size(max_output_size),
        );
        drop(tool_registry);

        let model_invocable_skills = self.skill_registry.list_model_invocable();
        let mut agent_config = AgentConfig {
            model: effective_model,
            working_dir,
            project_prompt,
            generation: forge_agent::GenerationConfig { max_tokens, temperature },
            skills: model_invocable_skills,
            thinking,
            thinking_adaptor,
            trust_level,
            memory_user_index,
            memory_project_index,
            permission_rules,
            ..Default::default()
        };

        if let Some(max_iter) = max_iterations_override {
            agent_config.loop_protection.max_iterations = max_iter;
        }
        if let Some(max_same_tool_calls) = max_same_tool_calls_override {
            agent_config.loop_protection.max_same_tool_calls = max_same_tool_calls;
        }
        if let Some(tool_call_limits) = tool_call_limits_override {
            agent_config.loop_protection.tool_call_limits = tool_call_limits;
        }
        agent_config.reflection.enabled = reflection_enabled;

        let confirmation_handler: Arc<dyn ConfirmationHandler> = if require_confirmation {
            Arc::new(SdkConfirmationHandler {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                pending_confirmations: self.pending_confirmations.clone(),
                pre_registered: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            })
        } else {
            Arc::new(forge_agent::AutoApproveHandler)
        };

        let pm = self.prompt_manager.read().await;
        let mut agent = CoreAgent::with_prompt_manager(
            provider,
            executor,
            agent_config,
            Arc::new((*pm).clone()),
        )
        .with_confirmation_handler(confirmation_handler);

        if let Some(writer) = &self.trace_writer {
            agent = agent.with_trace_writer(Arc::clone(writer));
        }

        let token = agent.cancellation_token();
        self.reserve_session_request_slot(&request_key, token).await?;
        *self.last_request.write().await = Some(request_key.clone());
        *self.is_dirty.write().await = true;

        let history = {
            let session_guard = self.active_session.read().await;
            if let Some(session) = session_guard.as_ref() {
                session.messages.iter().map(|m| self.session_message_to_history(m)).collect()
            } else {
                Vec::new()
            }
        };

        if let Err(e) = self.add_user_message(input).await {
            self.release_session_request_slot(&request_key).await;
            return Err(e);
        }

        let input_for_llm = self.expand_user_message_for_llm(input);
        let stream = match agent.process_with_history(&input_for_llm, &history) {
            Ok(stream) => stream,
            Err(e) => {
                self.release_session_request_slot(&request_key).await;
                return Err(ForgeError::from(e));
            }
        };

        let converted = Self::convert_agent_stream(stream, require_confirmation);

        let wrapped = AutoSaveStream {
            inner: Box::pin(converted),
            persist_state: PersistState::default(),
            active_session: self.active_session.clone(),
            session_manager: self.session_manager.clone(),
            saved: false,
            is_dirty: self.is_dirty.clone(),
            plan_mode_flag: self.plan_mode_flag.clone(),
            plan_file_path: self.plan_file_path.clone(),
            pending_persist: None,
            queued_event: None,
            end_after_persist: false,
            request_key,
            inflight_requests: self.inflight_requests.clone(),
            pending_confirmations: self.pending_confirmations.clone(),
        };

        Ok(Box::pin(wrapped))
    }

    /// Cancel the most recently started request (legacy single-session API).
    pub async fn abort(&self) {
        if let Some(key) = self.last_request.read().await.clone() {
            let token = self.inflight_requests.read().await.get(&key).cloned();
            if let Some(token) = token {
                token.lock().cancel();
            }
        }
    }

    /// Cancel a specific in-flight request (multi-session safe).
    pub async fn cancel(&self, session_id: &str, request_id: &str) -> bool {
        let key =
            RequestKey { session_id: session_id.to_string(), request_id: request_id.to_string() };
        let token = self.inflight_requests.read().await.get(&key).cloned();
        if let Some(token) = token {
            token.lock().cancel();
            true
        } else {
            false
        }
    }

    /// Check if an abort has been requested.
    pub async fn is_abort_requested(&self) -> bool {
        if let Some(key) = self.last_request.read().await.clone() {
            let token = self.inflight_requests.read().await.get(&key).cloned();
            if let Some(token) = token {
                return token.lock().is_cancelled();
            }
        }
        false
    }

    /// Analyze the project and generate FORGE.md documentation.
    pub async fn init_project(&self) -> Result<forge_agent::ProjectAnalysis> {
        let config = self.config.read().await;
        let effective_model = config.llm.effective_model();
        let working_dir = config.working_dir.clone();
        drop(config);

        let provider = self.provider_registry.get_for_model(&effective_model).ok_or_else(|| {
            ForgeError::Llm(forge_llm::LlmError::ProviderUnavailable(format!(
                "No provider found for model: {effective_model}"
            )))
        })?;

        let tool_registry = self.tool_registry.read().await;
        let analyzer = forge_agent::ProjectAnalyzer::new(
            provider,
            Arc::new(tool_registry.clone()),
            working_dir,
            effective_model,
        );
        drop(tool_registry);

        let analysis = analyzer.analyze().await.map_err(|e| {
            ForgeError::Agent(forge_agent::AgentError::PlanningError(e.to_string()))
        })?;

        analyzer
            .generate_forge_md(&analysis)
            .map_err(|e| ForgeError::StorageError(e.to_string()))?;

        Ok(analysis)
    }

    /// Check if FORGE.md already exists in the working directory.
    pub async fn has_project_documentation(&self) -> bool {
        let config = self.config.read().await;
        config.working_dir.join("FORGE.md").exists()
    }

    /// Process user input with explicit conversation history.
    ///
    /// Unlike [`Self::process`], this does not persist messages to the active session.
    pub async fn process_with_history(
        &self,
        input: &str,
        history: &[HistoryMessage],
    ) -> Result<Pin<Box<dyn Stream<Item = AgentEvent> + Send>>> {
        if self.active_session.read().await.is_none() {
            return Err(ForgeError::NoActiveSession);
        }

        let session_id = {
            let guard = self.active_session.read().await;
            let session = guard.as_ref().ok_or(ForgeError::NoActiveSession)?;
            session.id.to_string()
        };
        let request_id = Uuid::new_v4().to_string();
        let request_key =
            RequestKey { session_id: session_id.clone(), request_id: request_id.clone() };

        let config = self.config.read().await;
        let effective_model = config.llm.effective_model();
        let working_dir = config.working_dir.clone();
        let base_project_prompt = config.project_prompt.clone();
        let max_tokens = config.llm.max_tokens;
        let temperature = config.llm.effective_temperature();
        let thinking = config.llm.thinking.clone();
        let thinking_adaptor = config.llm.thinking_adaptor;
        let bash_timeout = config.tools.bash_timeout;
        let max_output_size = config.tools.max_output_size;
        let require_confirmation = config.tools.require_confirmation;
        let trust_level = config.tools.trust.level;
        let mut disabled_tools = config.tools.disabled.clone();
        let _memory_mode = config.tools.memory.effective_mode();
        let permission_rules = config.tools.permission_rules.clone();
        drop(config);

        let project_prompt =
            self.build_project_prompt_with_memory(base_project_prompt, &working_dir).await;
        let (memory_user_index, memory_project_index) =
            self.load_memory_indexes(&working_dir).await;

        let provider = self.provider_registry.get_for_model(&effective_model).ok_or_else(|| {
            ForgeError::Llm(forge_llm::LlmError::ProviderUnavailable(format!(
                "No provider found for model: {effective_model}"
            )))
        })?;

        let pm = self.prompt_manager.read().await;
        let persona_disabled_tools =
            pm.get_current_persona().map(|p| p.disabled_tools.clone()).unwrap_or_default();
        drop(pm);
        disabled_tools.extend(persona_disabled_tools);
        disabled_tools.sort();
        disabled_tools.dedup();

        let confirmed_paths = self.confirmed_paths.read().await.clone();
        let tool_context = ToolContext {
            working_dir: working_dir.clone(),
            env: self.resolve_tool_env().await,
            timeout_secs: bash_timeout,
            confirmed_paths,
            bash_readonly: false,
            lsp_manager: Some(self.lsp_manager.clone()),
            ..ToolContext::default()
        }
        .with_plan_mode_flag(self.plan_mode_flag.clone());

        let tool_registry = self.tool_registry.read().await;
        let filtered_registry = if disabled_tools.is_empty() {
            tool_registry.clone()
        } else {
            let mut filtered = tool_registry.clone();
            for tool_name in &disabled_tools {
                filtered.unregister(tool_name);
            }
            filtered
        };
        let executor = Arc::new(
            ToolExecutor::new(Arc::new(filtered_registry), tool_context)
                .with_timeout(Duration::from_secs(bash_timeout))
                .with_max_output_size(max_output_size),
        );
        drop(tool_registry);

        let model_invocable_skills = self.skill_registry.list_model_invocable();
        let agent_config = AgentConfig {
            model: effective_model,
            working_dir,
            project_prompt,
            generation: forge_agent::GenerationConfig { max_tokens, temperature },
            skills: model_invocable_skills,
            thinking,
            thinking_adaptor,
            trust_level,
            memory_user_index,
            memory_project_index,
            permission_rules,
            ..Default::default()
        };

        let confirmation_handler: Arc<dyn ConfirmationHandler> = if require_confirmation {
            Arc::new(SdkConfirmationHandler {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                pending_confirmations: self.pending_confirmations.clone(),
                pre_registered: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            })
        } else {
            Arc::new(forge_agent::AutoApproveHandler)
        };

        let pm = self.prompt_manager.read().await;
        let mut agent = CoreAgent::with_prompt_manager(
            provider,
            executor,
            agent_config,
            Arc::new((*pm).clone()),
        )
        .with_confirmation_handler(confirmation_handler);

        if let Some(writer) = &self.trace_writer {
            agent = agent.with_trace_writer(Arc::clone(writer));
        }

        let token = agent.cancellation_token();
        self.reserve_session_request_slot(&request_key, token).await?;
        *self.last_request.write().await = Some(request_key.clone());

        let input_for_llm = self.expand_user_message_for_llm(input);
        let history_for_llm = self.expand_history_for_llm(history);

        let stream = match agent.process_with_history(&input_for_llm, &history_for_llm) {
            Ok(stream) => stream,
            Err(e) => {
                self.release_session_request_slot(&request_key).await;
                return Err(ForgeError::from(e));
            }
        };

        let converted = Self::convert_agent_stream(stream, require_confirmation);

        let wrapped = CleanupStream {
            inner: Box::pin(converted),
            request_key,
            inflight_requests: self.inflight_requests.clone(),
            pending_confirmations: self.pending_confirmations.clone(),
            cleaned: false,
        };

        Ok(Box::pin(wrapped))
    }

    /// Process input in a specific session (multi-session safe).
    ///
    /// Returns a [`ProcessHandle`] containing `request_id` for precise cancel/confirm routing.
    pub async fn process_in_session(
        &self,
        session_id: &str,
        input: &str,
        options: ProcessOptions,
    ) -> Result<ProcessHandle> {
        let session_update_lock = self.get_session_persist_lock(session_id).await;

        let request_id: RequestId = Uuid::new_v4().to_string();
        let request_key =
            RequestKey { session_id: session_id.to_string(), request_id: request_id.clone() };

        let session_uuid = forge_session::SessionId::parse(session_id)
            .map_err(|e| ForgeError::SessionNotFound(format!("Invalid session ID: {e}")))?;

        let session = self.session_manager.get(session_uuid).await?;
        let working_dir = session.config.working_dir.clone();

        let config = self.config.read().await;
        let default_model = config.llm.effective_model();
        let effective_model = if session.config.model.is_empty() {
            default_model
        } else {
            session.config.model.clone()
        };
        let base_project_prompt = config.project_prompt.clone();
        let max_tokens = config.llm.max_tokens;
        let temperature = config.llm.effective_temperature();
        let thinking = config.llm.thinking.clone();
        let thinking_adaptor = config.llm.thinking_adaptor;
        let bash_timeout = config.tools.bash_timeout;
        let max_output_size = config.tools.max_output_size;
        let require_confirmation = config.tools.require_confirmation;
        let trust_level = config.tools.trust.level;
        let mut disabled_tools = config.tools.disabled.clone();
        let _memory_mode = config.tools.memory.effective_mode();
        let permission_rules = config.tools.permission_rules.clone();
        drop(config);

        let project_prompt =
            self.build_project_prompt_with_memory(base_project_prompt, &working_dir).await;
        let (memory_user_index, memory_project_index) =
            self.load_memory_indexes(&working_dir).await;

        let provider = self.provider_registry.get_for_model(&effective_model).ok_or_else(|| {
            ForgeError::Llm(forge_llm::LlmError::ProviderUnavailable(format!(
                "No provider found for model: {effective_model}"
            )))
        })?;

        // Extract persona options
        let pm = self.prompt_manager.read().await;
        let persona_config = pm.get_current_persona();
        let (
            persona_disabled_tools,
            max_iterations_override,
            bash_readonly,
            reflection_enabled,
            max_same_tool_calls_override,
            tool_call_limits_override,
        ) = if let Some(persona) = persona_config {
            (
                persona.disabled_tools.clone(),
                persona.options.max_iterations,
                persona.options.bash_readonly,
                persona.options.reflection_enabled,
                persona.options.max_same_tool_calls,
                if persona.options.tool_call_limits.is_empty() {
                    None
                } else {
                    Some(persona.options.tool_call_limits.clone())
                },
            )
        } else {
            (Vec::new(), None, false, true, None, None)
        };
        drop(pm);

        disabled_tools.extend(persona_disabled_tools);
        disabled_tools.sort();
        disabled_tools.dedup();

        // Per-request plan mode flag (avoids cross-session contamination).
        let plan_mode_flag = Arc::new(AtomicBool::new(false));

        let confirmed_paths = self.confirmed_paths.read().await.clone();
        let tool_context = ToolContext {
            working_dir: working_dir.clone(),
            env: self.resolve_tool_env().await,
            timeout_secs: bash_timeout,
            confirmed_paths,
            bash_readonly,
            lsp_manager: Some(self.lsp_manager.clone()),
            ..ToolContext::default()
        }
        .with_plan_mode_flag(plan_mode_flag.clone());

        let tool_registry = self.tool_registry.read().await;
        let filtered_registry = if disabled_tools.is_empty() {
            tool_registry.clone()
        } else {
            let mut filtered = tool_registry.clone();
            for tool_name in &disabled_tools {
                filtered.unregister(tool_name);
            }
            filtered
        };
        let executor = Arc::new(
            ToolExecutor::new(Arc::new(filtered_registry), tool_context)
                .with_timeout(Duration::from_secs(bash_timeout))
                .with_max_output_size(max_output_size),
        );
        drop(tool_registry);

        let model_invocable_skills = self.skill_registry.list_model_invocable();
        let mut agent_config = AgentConfig {
            model: effective_model,
            working_dir: working_dir.clone(),
            project_prompt,
            generation: forge_agent::GenerationConfig { max_tokens, temperature },
            skills: model_invocable_skills,
            thinking,
            thinking_adaptor,
            trust_level,
            memory_user_index,
            memory_project_index,
            permission_rules,
            ..Default::default()
        };

        if let Some(max_iter) = max_iterations_override {
            agent_config.loop_protection.max_iterations = max_iter;
        }
        if let Some(max_same_tool_calls) = max_same_tool_calls_override {
            agent_config.loop_protection.max_same_tool_calls = max_same_tool_calls;
        }
        if let Some(tool_call_limits) = tool_call_limits_override {
            agent_config.loop_protection.tool_call_limits = tool_call_limits;
        }
        agent_config.reflection.enabled = reflection_enabled;

        let confirmation_handler: Arc<dyn ConfirmationHandler> = if require_confirmation {
            Arc::new(SdkConfirmationHandler {
                session_id: session_id.to_string(),
                request_id: request_id.clone(),
                pending_confirmations: self.pending_confirmations.clone(),
                pre_registered: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            })
        } else {
            Arc::new(forge_agent::AutoApproveHandler)
        };

        let pm = self.prompt_manager.read().await;
        let mut agent = CoreAgent::with_prompt_manager(
            provider,
            executor,
            agent_config,
            Arc::new((*pm).clone()),
        )
        .with_confirmation_handler(confirmation_handler);

        if let Some(writer) = &self.trace_writer {
            agent = agent.with_trace_writer(Arc::clone(writer));
        }

        let token = agent.cancellation_token();
        self.reserve_session_request_slot(&request_key, token).await?;
        *self.last_request.write().await = Some(request_key.clone());

        // Build history and persist raw user input under the session lock.
        let history_for_llm = {
            let persist_result: Result<Vec<HistoryMessage>> = async {
                let _guard = session_update_lock.lock().await;
                let mut latest_session = self.session_manager.get(session_uuid).await?;
                let history = latest_session
                    .messages
                    .iter()
                    .map(|m| self.session_message_to_history(m))
                    .collect::<Vec<_>>();
                latest_session.add_message(Message::user(input));
                self.session_manager.update(&latest_session).await?;
                Ok(history)
            }
            .await;

            match persist_result {
                Ok(history) => history,
                Err(e) => {
                    self.release_session_request_slot(&request_key).await;
                    return Err(e);
                }
            }
        };

        let input_for_llm = self.expand_user_message_for_llm(input);

        let stream = match agent.process_with_history(&input_for_llm, &history_for_llm) {
            Ok(stream) => stream,
            Err(e) => {
                self.release_session_request_slot(&request_key).await;
                return Err(ForgeError::from(e));
            }
        };

        let converted = Self::convert_agent_stream(stream, require_confirmation);

        let persisted = SessionPersistStream {
            inner: Box::pin(converted),
            persist_state: PersistState::default(),
            session_manager: self.session_manager.clone(),
            session_id: session_uuid,
            session_update_lock,
            saved: false,
            plan_mode_flag,
            pending_persist: None,
            queued_event: None,
            end_after_persist: false,
            request_key: request_key.clone(),
            inflight_requests: self.inflight_requests.clone(),
            pending_confirmations: self.pending_confirmations.clone(),
        };

        let stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>> = match options.dispatch_mode {
            EventDispatchMode::Immediate => Box::pin(persisted),
            EventDispatchMode::Batched { max_bytes, max_latency_ms } => {
                Box::pin(BatchedTextDeltaStream::new(
                    Box::pin(persisted),
                    max_bytes,
                    Duration::from_millis(max_latency_ms),
                ))
            }
        };

        Ok(ProcessHandle { session_id: session_id.to_string(), request_id, stream })
    }

    /// Convert active session messages to history format.
    pub async fn session_to_history(&self) -> Result<Vec<HistoryMessage>> {
        let session_guard = self.active_session.read().await;
        let session = session_guard.as_ref().ok_or(ForgeError::NoActiveSession)?;
        Ok(session.messages.iter().map(|m| self.session_message_to_history(m)).collect())
    }

    /// Get the current session messages (raw format).
    pub async fn get_session_messages(&self) -> Result<Vec<forge_session::Message>> {
        let session_guard = self.active_session.read().await;
        let session = session_guard.as_ref().ok_or(ForgeError::NoActiveSession)?;
        Ok(session.messages.clone())
    }
}
