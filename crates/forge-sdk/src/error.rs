//! Error types for Forge SDK

use thiserror::Error;

/// Forge SDK errors
#[derive(Debug, Error)]
pub enum ForgeError {
    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Session not found
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    /// Session already has an in-flight request
    #[error(
        "Session {session_id} already has an in-flight request ({inflight_request_id}); wait or cancel it first"
    )]
    SessionBusy {
        /// Active session identifier.
        session_id: String,
        /// In-flight request identifier.
        inflight_request_id: String,
    },

    /// No active session
    #[error("No active session")]
    NoActiveSession,

    /// Agent error (preserves structured `AgentError`)
    #[error(transparent)]
    Agent(#[from] forge_agent::AgentError),

    /// LLM provider error (preserves structured `LlmError` with `RateLimited`, etc.)
    #[error(transparent)]
    Llm(#[from] forge_llm::LlmError),

    /// Tool error (preserves structured `ToolError`)
    #[error(transparent)]
    Tool(#[from] forge_tools::ToolError),

    /// Session/storage error (preserves structured `SessionError`)
    #[error(transparent)]
    Session(#[from] forge_session::SessionError),

    /// Persona not found
    #[error("Persona not found: {0}")]
    PersonaNotFound(String),

    /// Generic storage error (for non-session storage failures)
    #[error("Storage error: {0}")]
    StorageError(String),

    /// IO error
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Already running
    #[error("SDK is already running")]
    AlreadyRunning,

    /// Aborted
    #[error("Operation was aborted")]
    Aborted,

    /// Invalid confirmation ID
    #[error("Invalid confirmation ID: {0}")]
    InvalidConfirmation(String),

    /// Tool confirmation rejected
    #[error("Tool confirmation was rejected: {0}")]
    ConfirmationRejected(String),

    /// Skill system error
    #[error("Skill error: {0}")]
    SkillError(String),
}

impl ForgeError {
    /// Stable machine-readable error code for frontend/IPC branching.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ConfigError(_) => "config_error",
            Self::SessionNotFound(_) => "session_not_found",
            Self::SessionBusy { .. } => "session_busy",
            Self::NoActiveSession => "no_active_session",
            Self::Agent(_) => "agent_error",
            Self::Llm(_) => "llm_error",
            Self::Tool(_) => "tool_error",
            Self::Session(_) => "session_error",
            Self::PersonaNotFound(_) => "persona_not_found",
            Self::StorageError(_) => "storage_error",
            Self::IoError(_) => "io_error",
            Self::AlreadyRunning => "already_running",
            Self::Aborted => "aborted",
            Self::InvalidConfirmation(_) => "invalid_confirmation",
            Self::ConfirmationRejected(_) => "confirmation_rejected",
            Self::SkillError(_) => "skill_error",
        }
    }

    /// Structured payload for API/IPC responses.
    #[must_use]
    pub fn machine_payload(&self) -> serde_json::Value {
        let mut payload = serde_json::json!({
            "code": self.code(),
            "message": self.to_string(),
            "retryable": self.is_retryable(),
            "rate_limited": self.is_rate_limited(),
        });

        if let serde_json::Value::Object(obj) = &mut payload {
            if let Some(delay) = self.retry_delay() {
                obj.insert(
                    "retry_after_seconds".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(delay.as_secs())),
                );
            }
            if let Self::SessionBusy { session_id, inflight_request_id } = self {
                obj.insert(
                    "session_id".to_string(),
                    serde_json::Value::String(session_id.clone()),
                );
                obj.insert(
                    "inflight_request_id".to_string(),
                    serde_json::Value::String(inflight_request_id.clone()),
                );
            }
        }

        payload
    }

    /// Check if this error is a rate-limit from the LLM provider.
    #[must_use]
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::Llm(forge_llm::LlmError::RateLimited { .. }))
    }

    /// Get the suggested retry delay if this is a rate-limit error.
    #[must_use]
    pub fn retry_delay(&self) -> Option<std::time::Duration> {
        match self {
            Self::Llm(e) => e.retry_delay(),
            _ => None,
        }
    }

    /// Check if this error is retryable (rate-limit, transient network, timeout).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::SessionBusy { .. }
                | Self::Llm(forge_llm::LlmError::RateLimited { .. })
                | Self::Agent(forge_agent::AgentError::Timeout(_))
        ) || matches!(self, Self::Llm(e) if e.is_retryable())
    }
}

impl From<forge_prompt::PromptError> for ForgeError {
    fn from(e: forge_prompt::PromptError) -> Self {
        Self::ConfigError(e.to_string())
    }
}

/// Result type for Forge SDK operations
pub type Result<T> = std::result::Result<T, ForgeError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes() {
        assert_eq!(ForgeError::ConfigError("x".into()).code(), "config_error");
        assert_eq!(ForgeError::SessionNotFound("s1".into()).code(), "session_not_found");
        assert_eq!(ForgeError::NoActiveSession.code(), "no_active_session");
        assert_eq!(ForgeError::AlreadyRunning.code(), "already_running");
        assert_eq!(ForgeError::Aborted.code(), "aborted");
        assert_eq!(ForgeError::SkillError("x".into()).code(), "skill_error");
        assert_eq!(ForgeError::PersonaNotFound("p".into()).code(), "persona_not_found");
        assert_eq!(ForgeError::StorageError("s".into()).code(), "storage_error");
        assert_eq!(
            ForgeError::SessionBusy {
                session_id: "s1".into(),
                inflight_request_id: "r1".into(),
            }
            .code(),
            "session_busy"
        );
    }

    #[test]
    fn test_error_display() {
        let err = ForgeError::ConfigError("bad value".into());
        assert_eq!(err.to_string(), "Configuration error: bad value");

        let err = ForgeError::SessionNotFound("abc".into());
        assert_eq!(err.to_string(), "Session not found: abc");

        let err = ForgeError::SessionBusy {
            session_id: "s1".into(),
            inflight_request_id: "r1".into(),
        };
        assert!(err.to_string().contains("s1"));
        assert!(err.to_string().contains("r1"));
    }

    #[test]
    fn test_machine_payload_structure() {
        let err = ForgeError::ConfigError("test".into());
        let payload = err.machine_payload();
        assert_eq!(payload["code"], "config_error");
        assert_eq!(payload["retryable"], false);
        assert_eq!(payload["rate_limited"], false);
    }

    #[test]
    fn test_session_busy_payload_includes_ids() {
        let err = ForgeError::SessionBusy {
            session_id: "sess-1".into(),
            inflight_request_id: "req-1".into(),
        };
        let payload = err.machine_payload();
        assert_eq!(payload["session_id"], "sess-1");
        assert_eq!(payload["inflight_request_id"], "req-1");
    }

    #[test]
    fn test_retryable_session_busy() {
        let err = ForgeError::SessionBusy {
            session_id: "s".into(),
            inflight_request_id: "r".into(),
        };
        assert!(err.is_retryable());
    }

    #[test]
    fn test_not_retryable_config_error() {
        let err = ForgeError::ConfigError("bad".into());
        assert!(!err.is_retryable());
        assert!(!err.is_rate_limited());
    }

    #[test]
    fn test_not_rate_limited_non_llm() {
        let err = ForgeError::Aborted;
        assert!(!err.is_rate_limited());
        assert!(err.retry_delay().is_none());
    }

    #[test]
    fn test_from_prompt_error() {
        let prompt_err = forge_prompt::PromptError::Load("test".into());
        let forge_err: ForgeError = prompt_err.into();
        assert_eq!(forge_err.code(), "config_error");
    }
}
