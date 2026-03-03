//! Session, model, context, and status management for ForgeSDK

use super::*;

impl ForgeSDK {
    // ========================
    // Session Management API
    // ========================

    /// Create a new session.
    ///
    /// # Errors
    ///
    /// Returns error if session creation fails.
    pub async fn create_session(&self) -> Result<SessionId> {
        let config = self.config.read().await;
        let model = config.llm.effective_model();
        let max_context_tokens = self.resolve_context_limit_for_model(&model);
        let session_config = SessionConfig {
            model,
            max_context_tokens,
            system_prompt: None,
            working_dir: config.working_dir.clone(),
        };
        drop(config);

        let session = self.session_manager.create(session_config).await?;
        let id = session.id.to_string();
        *self.active_session.write().await = Some(session);
        *self.is_dirty.write().await = false;
        Ok(id)
    }

    /// Resume an existing session.
    ///
    /// # Errors
    ///
    /// Returns error if session ID is invalid or not found.
    pub async fn resume_session(&self, id: SessionId) -> Result<()> {
        let session_id = forge_session::SessionId::parse(&id)
            .map_err(|e| ForgeError::SessionNotFound(format!("Invalid session ID: {e}")))?;
        let session = self.session_manager.get(session_id).await?;
        *self.active_session.write().await = Some(session);
        *self.is_dirty.write().await = false;
        Ok(())
    }

    /// Get the most recent session.
    ///
    /// # Errors
    ///
    /// Returns error if session listing fails.
    pub async fn latest_session(&self) -> Result<Option<SessionId>> {
        let session = self.session_manager.latest().await?;
        if let Some(s) = session {
            let id = s.id.to_string();
            *self.active_session.write().await = Some(s);
            Ok(Some(id))
        } else {
            Ok(None)
        }
    }

    /// List all sessions.
    ///
    /// # Errors
    ///
    /// Returns error if session listing fails.
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let summaries = self.session_manager.list_summaries().await?;
        Ok(summaries
            .into_iter()
            .map(|s| SessionSummary {
                id: s.id.to_string(),
                title: s.title,
                created_at: s.created_at,
                updated_at: s.updated_at,
                message_count: s.message_count,
                total_tokens: s.total_tokens,
                tags: s.tags,
                working_dir: s.working_dir,
            })
            .collect())
    }

    /// Save the current session.
    ///
    /// # Errors
    ///
    /// Returns error if persistence fails.
    pub async fn save_session(&self) -> Result<()> {
        self.persist_active_session_snapshot().await?;
        *self.is_dirty.write().await = false;
        Ok(())
    }

    /// Close the current session.
    ///
    /// # Errors
    ///
    /// Returns error if save fails.
    pub async fn close_session(&self) -> Result<()> {
        self.save_session().await?;
        let closed_session_id = self.active_session.read().await.as_ref().map(|s| s.id.to_string());
        *self.active_session.write().await = None;

        if let Some(session_id) = closed_session_id {
            self.cancel_and_remove_session_requests(&session_id).await;
            self.session_persist_locks.write().await.remove(&session_id);
        }
        Ok(())
    }

    /// Delete a session.
    ///
    /// # Errors
    ///
    /// Returns error if session ID is invalid or deletion fails.
    pub async fn delete_session(&self, id: SessionId) -> Result<()> {
        let session_id = forge_session::SessionId::parse(&id)
            .map_err(|e| ForgeError::SessionNotFound(format!("Invalid session ID: {e}")))?;
        self.cancel_and_remove_session_requests(&id).await;
        self.session_manager.delete(session_id).await?;

        if self.active_session.read().await.as_ref().is_some_and(|s| s.id.to_string() == id) {
            *self.active_session.write().await = None;
        }
        self.session_persist_locks.write().await.remove(&id);
        Ok(())
    }

    /// Get the active session ID.
    pub async fn active_session_id(&self) -> Option<SessionId> {
        self.active_session.read().await.as_ref().map(|s| s.id.to_string())
    }

    pub(super) async fn get_session_persist_lock(
        &self,
        session_id: &str,
    ) -> Arc<tokio::sync::Mutex<()>> {
        if let Some(existing) = self.session_persist_locks.read().await.get(session_id).cloned() {
            return existing;
        }
        let mut locks = self.session_persist_locks.write().await;
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    // ========================
    // Model API
    // ========================

    /// Switch to a different model at runtime.
    ///
    /// # Errors
    ///
    /// Returns error if no provider supports the model.
    pub async fn switch_model(&self, model: &str) -> Result<ModelSwitchResult> {
        let mut config = self.config.write().await;
        let previous_model = config.llm.model.clone();
        config.llm.model = model.to_string();
        let new_model = config.llm.effective_model();
        drop(config);

        if self.provider_registry.get_for_model(&new_model).is_none() {
            let mut config = self.config.write().await;
            config.llm.model = previous_model.clone();
            return Err(ForgeError::Llm(forge_llm::LlmError::ProviderUnavailable(format!(
                "No provider found for model: {new_model}"
            ))));
        }

        let session_snapshot = {
            let mut session_guard = self.active_session.write().await;
            if let Some(session) = session_guard.as_mut() {
                session.config.model = new_model.clone();
                session.config.max_context_tokens =
                    self.resolve_context_limit_for_model(&new_model);
                session.metadata.updated_at = Utc::now();
                session.metadata.total_tokens =
                    ContextManager::estimate_total_tokens(&session.messages);
                Some(session.clone())
            } else {
                None
            }
        };

        if let Some(session) = session_snapshot {
            *self.is_dirty.write().await = true;
            self.session_manager.update(&session).await?;
            *self.is_dirty.write().await = false;
        }

        Ok(ModelSwitchResult { previous_model, new_model })
    }

    /// Get the current model name.
    pub async fn current_model(&self) -> String {
        self.config.read().await.llm.effective_model()
    }

    // ========================
    // Context API
    // ========================

    /// Compress the current session context.
    ///
    /// # Errors
    ///
    /// Returns error if no active session or compression fails.
    pub async fn compact_context(&self, instructions: Option<&str>) -> Result<CompressionResult> {
        let mut session_guard = self.active_session.write().await;
        let session = session_guard.as_mut().ok_or(ForgeError::NoActiveSession)?;

        let config = self.config.read().await;
        let effective_model = config.llm.effective_model();
        drop(config);

        let messages_before = session.messages.len();
        let tokens_before = ContextManager::estimate_total_tokens(&session.messages);

        const KEEP_RECENT: usize = 5;
        if session.messages.len() <= KEEP_RECENT {
            return Ok(CompressionResult {
                messages_before,
                messages_after: messages_before,
                tokens_before,
                tokens_after: tokens_before,
                summary: None,
            });
        }

        let split_point = session.messages.len().saturating_sub(KEEP_RECENT);
        let to_summarize = &session.messages[..split_point];
        let to_keep = session.messages[split_point..].to_vec();

        let context_manager = self.build_context_manager_for_model(&effective_model);
        let summary_result = self.generate_llm_summary(to_summarize, instructions).await;

        let (compressed, actual_summary) = match summary_result {
            Ok(summary_text) => {
                let mut result = vec![Message::system(format!(
                    "[Previous Conversation Summary]\n\n{summary_text}\n\n[End of Summary]"
                ))];
                result.extend(to_keep);
                (result, Some(summary_text))
            }
            Err(e) => {
                tracing::warn!("LLM compression failed: {e}, using fallback");
                let (compressed, fallback_result) = context_manager.compress(&session.messages);
                (compressed, fallback_result.summary.map(|s| format!("[Fallback] {s}")))
            }
        };

        let mut compressed = compressed;
        let available = context_manager.available_tokens();
        let mut tokens_after = ContextManager::estimate_total_tokens(&compressed);
        if tokens_after > available {
            let trimmed = context_manager.trim_to_fit(&compressed);
            if trimmed.len() < compressed.len() {
                compressed = trimmed;
                tokens_after = ContextManager::estimate_total_tokens(&compressed);
            }
        }

        let messages_after = compressed.len();
        session.messages = compressed;
        session.metadata.total_tokens = tokens_after;
        session.metadata.updated_at = Utc::now();

        *self.is_dirty.write().await = true;
        self.session_manager.update(session).await?;
        *self.is_dirty.write().await = false;

        Ok(CompressionResult {
            messages_before,
            messages_after,
            tokens_before,
            tokens_after,
            summary: actual_summary,
        })
    }

    /// Generate summary using LLM.
    async fn generate_llm_summary(
        &self,
        messages: &[Message],
        custom_instructions: Option<&str>,
    ) -> Result<String> {
        let config = self.config.read().await;
        let effective_model = config.llm.effective_model();
        let max_tokens = config.llm.max_tokens;
        let temperature = config.llm.effective_temperature();
        let thinking = config.llm.thinking.clone();
        let thinking_adaptor = config.llm.thinking_adaptor;
        drop(config);

        let provider = self.provider_registry.get_for_model(&effective_model).ok_or_else(|| {
            ForgeError::Llm(forge_llm::LlmError::ProviderUnavailable(
                "No provider for compression".to_string(),
            ))
        })?;

        let conversation_text = messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    forge_session::MessageRole::User => "User",
                    forge_session::MessageRole::Assistant => "Assistant",
                    forge_session::MessageRole::System => "System",
                };
                let content = Self::format_message_content(&m.content);
                format!("{role}: {content}")
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        let prompt = custom_instructions.unwrap_or(COMPRESSION_PROMPT);
        let compression_messages = vec![ChatMessage {
            role: ChatRole::User,
            content: MessageContent::Text(format!(
                "{prompt}\n\nSummarize this conversation:\n\n{conversation_text}"
            )),
        }];

        let llm_config = LlmConfig {
            model: effective_model,
            max_tokens,
            temperature,
            system_prompt: None,
            system_blocks: None,
            enable_cache: true,
            thinking,
            thinking_adaptor,
            stream_timeout_secs: LlmConfig::default_stream_timeout_secs(),
            response_schema: None,
        };

        let stream_result = tokio::time::timeout(
            Duration::from_secs(30),
            provider.chat_stream(&compression_messages, vec![], &llm_config),
        )
        .await;

        let mut stream = match stream_result {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => return Err(ForgeError::Llm(e)),
            Err(_) => return Err(ForgeError::Llm(forge_llm::LlmError::Timeout(30))),
        };

        let mut summary = String::new();
        while let Some(event) = stream.next().await {
            if let Ok(LlmEvent::TextDelta(text)) = event {
                summary.push_str(&text);
            }
        }

        if summary.is_empty() {
            return Err(ForgeError::Llm(forge_llm::LlmError::ParseError(
                "Empty summary".to_string(),
            )));
        }

        Ok(summary)
    }

    /// Truncate string safely at UTF-8 character boundary.
    fn truncate_utf8_safe(s: &str, max_chars: usize) -> String {
        if s.chars().count() <= max_chars {
            s.to_string()
        } else {
            let truncated: String = s.chars().take(max_chars).collect();
            format!("{truncated}...(truncated)")
        }
    }

    /// Format message content including tool info.
    pub(super) fn format_message_content(content: &forge_session::MessageContent) -> String {
        const MAX_TOOL_CONTENT_LEN: usize = 2000;

        match content {
            forge_session::MessageContent::Text(s) => s.clone(),
            forge_session::MessageContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    forge_session::ContentBlock::Text { text } => text.clone(),
                    forge_session::ContentBlock::ToolUse { name, input, .. } => {
                        let input_str = input.to_string();
                        let truncated = Self::truncate_utf8_safe(&input_str, MAX_TOOL_CONTENT_LEN);
                        format!("[Tool: {name}] {truncated}")
                    }
                    forge_session::ContentBlock::ToolResult { content, is_error, .. } => {
                        let truncated = Self::truncate_utf8_safe(content, MAX_TOOL_CONTENT_LEN);
                        if *is_error {
                            format!("[Tool Error] {truncated}")
                        } else {
                            format!("[Tool Result] {truncated}")
                        }
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// Check if context compression is needed.
    ///
    /// # Errors
    ///
    /// Returns error if no active session.
    pub async fn needs_compression(&self) -> Result<bool> {
        let session_guard = self.active_session.read().await;
        let session = session_guard.as_ref().ok_or(ForgeError::NoActiveSession)?;
        let config = self.config.read().await;
        let effective_model = config.llm.effective_model();
        drop(config);
        let context_manager = self.build_context_manager_for_model(&effective_model);
        Ok(context_manager.needs_compression(&session.messages))
    }

    /// Get the current context token count (estimated).
    ///
    /// # Errors
    ///
    /// Returns error if no active session.
    pub async fn context_token_count(&self) -> Result<usize> {
        let session_guard = self.active_session.read().await;
        let session = session_guard.as_ref().ok_or(ForgeError::NoActiveSession)?;
        Ok(ContextManager::estimate_total_tokens(&session.messages))
    }

    // ========================
    // Status API
    // ========================

    /// Get the current session status.
    ///
    /// # Errors
    ///
    /// Returns error if no active session.
    pub async fn get_status(&self) -> Result<SessionStatus> {
        let session_guard = self.active_session.read().await;
        let session = session_guard.as_ref().ok_or(ForgeError::NoActiveSession)?;
        let config = self.config.read().await;
        let effective_model = config.llm.effective_model();
        let persona = self.prompt_manager.read().await.current_persona().to_string();
        let is_dirty = *self.is_dirty.read().await;
        let token_count = ContextManager::estimate_total_tokens(&session.messages);

        Ok(SessionStatus {
            id: session.id.to_string(),
            message_count: session.messages.len(),
            model: effective_model.clone(),
            working_dir: config.working_dir.clone(),
            token_usage: crate::event::TokenUsage {
                input_tokens: token_count,
                output_tokens: 0,
                cache_read_tokens: None,
                cache_creation_tokens: None,
            },
            context_limit: self.resolve_context_limit_for_model(&effective_model),
            persona,
            title: session.metadata.title.clone(),
            is_dirty,
        })
    }

    /// Check if session has unsaved changes.
    pub async fn is_dirty(&self) -> bool {
        *self.is_dirty.read().await
    }

    // ========================
    // Message API
    // ========================

    /// Add a user message to the current session.
    ///
    /// # Errors
    ///
    /// Returns error if no active session or persistence fails.
    pub async fn add_user_message(&self, content: &str) -> Result<()> {
        *self.is_dirty.write().await = true;
        let snapshot = {
            let mut session_guard = self.active_session.write().await;
            let session = session_guard.as_mut().ok_or(ForgeError::NoActiveSession)?;
            session.add_message(Message::user(content));
            session.clone()
        };
        self.session_manager.update(&snapshot).await?;
        *self.is_dirty.write().await = false;
        Ok(())
    }

    /// Add an assistant message to the current session.
    ///
    /// # Errors
    ///
    /// Returns error if no active session or persistence fails.
    pub async fn add_assistant_message(&self, content: &str) -> Result<()> {
        *self.is_dirty.write().await = true;
        let snapshot = {
            let mut session_guard = self.active_session.write().await;
            let session = session_guard.as_mut().ok_or(ForgeError::NoActiveSession)?;
            session.add_message(Message::assistant(content));
            session.clone()
        };
        self.session_manager.update(&snapshot).await?;
        *self.is_dirty.write().await = false;
        Ok(())
    }
}
