//! LLM Provider middleware layer.
//!
//! Wraps an inner `LlmProvider` with cross-cutting concerns:
//! - Automatic retry with exponential backoff for retryable errors
//! - Per-call timeout
//! - Structured logging
//! - Optional retry notification channel (for UI feedback)

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::auth::AuthRotator;
use crate::retry::RetryConfig;
use crate::{ChatMessage, LlmConfig, LlmError, LlmEventStream, LlmProvider, ModelInfo, ToolDef};

/// Maximum number of auth-rotation attempts before giving up.
///
/// Prevents infinite loops when all credentials are invalid.
const MAX_AUTH_ROTATIONS: u32 = 10;

// ---------------------------------------------------------------------------
// Retry notification (for forwarding to UI)
// ---------------------------------------------------------------------------

/// Notification emitted when the middleware retries an LLM call.
#[derive(Debug, Clone)]
pub struct RetryNotification {
    /// Current attempt (1-based).
    pub attempt: u32,
    /// Maximum attempts allowed.
    pub max_attempts: u32,
    /// Human-readable error that triggered the retry.
    pub error: String,
    /// Delay before the next attempt.
    pub delay: Duration,
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Lightweight atomic metrics for LLM calls.
#[derive(Debug, Default)]
pub struct LlmMetrics {
    /// Total calls attempted (including retries).
    pub total_calls: AtomicUsize,
    /// Calls that succeeded on the first attempt.
    pub first_attempt_successes: AtomicUsize,
    /// Calls that succeeded after retries.
    pub retry_successes: AtomicUsize,
    /// Calls that exhausted all retries and failed.
    pub exhausted_failures: AtomicUsize,
    /// Total retry attempts across all calls.
    pub total_retries: AtomicUsize,
    /// Cumulative latency in milliseconds (for averages).
    pub total_latency_ms: AtomicU64,
}

impl LlmMetrics {
    /// Create a new metrics instance.
    pub fn new() -> Self {
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// InstrumentedProvider
// ---------------------------------------------------------------------------

/// A provider wrapper that adds retry, timeout, logging, and metrics.
pub struct InstrumentedProvider {
    inner: Arc<dyn LlmProvider>,
    retry_config: RetryConfig,
    /// Per-call timeout (applied to the initial `chat_stream` request, not the
    /// stream itself).
    timeout: Duration,
    /// Optional channel for retry notifications (UI feedback).
    retry_tx: Option<mpsc::UnboundedSender<RetryNotification>>,
    /// Shared metrics.
    metrics: Arc<LlmMetrics>,
    /// Optional auth credential rotator for multi-key failover.
    auth_rotator: Option<Arc<AuthRotator>>,
    /// Per-credential provider instances (same length as rotator credentials).
    auth_providers: Vec<Arc<dyn LlmProvider>>,
}

impl InstrumentedProvider {
    /// Wrap an existing provider with default settings.
    pub fn new(inner: Arc<dyn LlmProvider>) -> Self {
        Self {
            inner,
            retry_config: RetryConfig::default(),
            timeout: Duration::from_secs(300),
            retry_tx: None,
            metrics: Arc::new(LlmMetrics::new()),
            auth_rotator: None,
            auth_providers: vec![],
        }
    }

    /// Set the retry configuration.
    #[must_use]
    pub const fn with_retry(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// Set the per-call timeout.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Attach a channel for retry notifications.
    #[must_use]
    pub fn with_retry_notifications(
        mut self,
        tx: mpsc::UnboundedSender<RetryNotification>,
    ) -> Self {
        self.retry_tx = Some(tx);
        self
    }

    /// Share pre-existing metrics (e.g. across multiple providers).
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<LlmMetrics>) -> Self {
        self.metrics = metrics;
        self
    }

    /// Get a reference to the collected metrics.
    pub const fn metrics(&self) -> &Arc<LlmMetrics> {
        &self.metrics
    }

    /// Attach an [`AuthRotator`] with per-credential provider instances.
    #[must_use]
    pub fn with_auth_rotator(
        mut self,
        rotator: Arc<AuthRotator>,
        providers: Vec<Arc<dyn LlmProvider>>,
    ) -> Self {
        debug_assert_eq!(
            rotator.len(),
            providers.len(),
            "auth_providers length must match rotator credential count"
        );
        self.auth_rotator = Some(rotator);
        self.auth_providers = providers;
        self
    }

    /// Get a reference to the auth rotator, if configured.
    pub const fn auth_rotator(&self) -> Option<&Arc<AuthRotator>> {
        self.auth_rotator.as_ref()
    }

    /// Select the active provider based on the auth rotator's current index.
    fn active_provider(&self) -> &Arc<dyn LlmProvider> {
        if let Some(ref rotator) = self.auth_rotator {
            let idx = rotator.current_index();
            if let Some(provider) = self.auth_providers.get(idx) {
                return provider;
            }
        }
        &self.inner
    }
}

#[async_trait]
impl LlmProvider for InstrumentedProvider {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        self.inner.supported_models()
    }

    fn context_limit(&self, model: &str) -> usize {
        self.inner.context_limit(model)
    }

    fn estimate_tokens(&self, text: &str) -> usize {
        self.inner.estimate_tokens(text)
    }

    #[allow(clippy::cast_possible_truncation)]
    async fn chat_stream(
        &self,
        messages: &[ChatMessage],
        tools: Vec<ToolDef>,
        config: &LlmConfig,
    ) -> crate::Result<LlmEventStream> {
        self.metrics.total_calls.fetch_add(1, Ordering::Relaxed);
        let start = Instant::now();

        let span = tracing::info_span!(
            "llm.chat_stream",
            otel.kind = "client",
            llm.provider = %self.inner.id(),
            llm.model = %config.model,
            llm.max_tokens = config.max_tokens,
            llm.tool_count = tools.len(),
            llm.message_count = messages.len(),
            llm.attempt = tracing::field::Empty,
            llm.latency_ms = tracing::field::Empty,
            llm.status = tracing::field::Empty,
        );

        self.chat_stream_inner(messages, tools, config, start, span).await
    }
}

impl InstrumentedProvider {
    /// Inner implementation that runs inside the tracing span.
    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    async fn chat_stream_inner(
        &self,
        messages: &[ChatMessage],
        tools: Vec<ToolDef>,
        config: &LlmConfig,
        start: Instant,
        span: tracing::Span,
    ) -> crate::Result<LlmEventStream> {
        use tracing::Instrument;

        async {
            let max_attempts = self.retry_config.max_retries + 1;
            let mut attempt = 0u32;
            let mut auth_rotations = 0u32;
            let mut delay = self.retry_config.initial_delay;

            loop {
                attempt += 1;

                // Capture credential index before the request to avoid TOCTOU races
                let cred_idx = self.auth_rotator.as_ref().map(|r| r.current_index());

                let result = tokio::time::timeout(
                    self.timeout,
                    self.active_provider().chat_stream(messages, tools.clone(), config),
                )
                .await;

                match result {
                    Ok(Ok(stream)) => {
                        let elapsed = start.elapsed();
                        self.metrics
                            .total_latency_ms
                            .fetch_add(elapsed.as_millis() as u64, Ordering::Relaxed);
                        if attempt == 1 {
                            self.metrics.first_attempt_successes.fetch_add(1, Ordering::Relaxed);
                        } else {
                            self.metrics.retry_successes.fetch_add(1, Ordering::Relaxed);
                        }
                        if let (Some(ref rotator), Some(idx)) = (&self.auth_rotator, cred_idx) {
                            rotator.record_success_at(idx);
                        }
                        tracing::Span::current().record("llm.attempt", attempt);
                        tracing::Span::current()
                            .record("llm.latency_ms", elapsed.as_millis() as u64);
                        tracing::Span::current().record("llm.status", "ok");
                        return Ok(stream);
                    }
                    Ok(Err(e)) => {
                        // Auth rotator: track credential-level errors
                        if let (Some(ref rotator), Some(idx)) = (&self.auth_rotator, cred_idx) {
                            if e.is_auth_error() {
                                auth_rotations += 1;
                                if auth_rotations >= MAX_AUTH_ROTATIONS {
                                    tracing::error!(
                                        auth_rotations,
                                        "All credentials exhausted after {} auth rotations",
                                        auth_rotations
                                    );
                                    let elapsed = start.elapsed();
                                    self.metrics
                                        .total_latency_ms
                                        .fetch_add(elapsed.as_millis() as u64, Ordering::Relaxed);
                                    self.metrics.exhausted_failures.fetch_add(1, Ordering::Relaxed);
                                    tracing::Span::current().record("llm.attempt", attempt);
                                    tracing::Span::current()
                                        .record("llm.latency_ms", elapsed.as_millis() as u64);
                                    tracing::Span::current().record("llm.status", "error");
                                    return Err(e);
                                }
                                rotator.record_auth_failure_at(idx);
                                tracing::warn!(
                                    attempt,
                                    auth_rotations,
                                    error = %e,
                                    "Auth error — rotated credential via AuthRotator"
                                );
                                attempt = attempt.saturating_sub(1);
                                tokio::time::sleep(Duration::from_millis(50)).await;
                                continue;
                            }
                            if e.is_rate_limited() {
                                rotator.record_rate_limited_at(idx);
                                tracing::warn!(
                                    attempt,
                                    error = %e,
                                    "Rate limited — rotated credential via AuthRotator"
                                );
                            }
                        }

                        // Non-retryable or exhausted → propagate
                        if !e.is_retryable() || attempt >= max_attempts {
                            let elapsed = start.elapsed();
                            self.metrics
                                .total_latency_ms
                                .fetch_add(elapsed.as_millis() as u64, Ordering::Relaxed);
                            if attempt > 1 {
                                self.metrics.exhausted_failures.fetch_add(1, Ordering::Relaxed);
                            }
                            tracing::Span::current().record("llm.attempt", attempt);
                            tracing::Span::current()
                                .record("llm.latency_ms", elapsed.as_millis() as u64);
                            tracing::Span::current().record("llm.status", "error");
                            return Err(e);
                        }

                        // Retryable — compute wait time
                        let wait =
                            e.retry_delay().unwrap_or(delay).min(self.retry_config.max_delay);
                        self.metrics.total_retries.fetch_add(1, Ordering::Relaxed);

                        tracing::warn!(
                            attempt,
                            max_attempts,
                            error = %e,
                            delay_ms = wait.as_millis(),
                            "LLM call failed, retrying"
                        );

                        if let Some(tx) = &self.retry_tx {
                            let _ = tx.send(RetryNotification {
                                attempt,
                                max_attempts,
                                error: e.to_string(),
                                delay: wait,
                            });
                        }

                        tokio::time::sleep(wait).await;
                        delay = Duration::from_secs_f32(
                            delay.as_secs_f32() * self.retry_config.backoff_factor,
                        );
                    }
                    Err(_elapsed) => {
                        // Timeout
                        let timeout_err = LlmError::Timeout(self.timeout.as_secs());

                        if attempt >= max_attempts {
                            let elapsed = start.elapsed();
                            self.metrics
                                .total_latency_ms
                                .fetch_add(elapsed.as_millis() as u64, Ordering::Relaxed);
                            self.metrics.exhausted_failures.fetch_add(1, Ordering::Relaxed);
                            tracing::Span::current().record("llm.attempt", attempt);
                            tracing::Span::current()
                                .record("llm.latency_ms", elapsed.as_millis() as u64);
                            tracing::Span::current().record("llm.status", "timeout");
                            return Err(timeout_err);
                        }

                        self.metrics.total_retries.fetch_add(1, Ordering::Relaxed);

                        let wait = delay.min(self.retry_config.max_delay);

                        tracing::warn!(
                            attempt,
                            max_attempts,
                            timeout_secs = self.timeout.as_secs(),
                            delay_ms = wait.as_millis(),
                            "LLM call timed out, retrying"
                        );

                        if let Some(tx) = &self.retry_tx {
                            let _ = tx.send(RetryNotification {
                                attempt,
                                max_attempts,
                                error: timeout_err.to_string(),
                                delay: wait,
                            });
                        }

                        tokio::time::sleep(wait).await;
                        delay = Duration::from_secs_f32(
                            delay.as_secs_f32() * self.retry_config.backoff_factor,
                        );
                    }
                }
            }
        }
        .instrument(span)
        .await
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LlmEvent, ModelInfo};
    use futures::stream;
    use std::sync::atomic::AtomicU32;

    /// Minimal mock provider for testing the middleware.
    struct MockProvider {
        call_count: Arc<AtomicU32>,
        fail_count: u32,
        error_kind: MockErrorKind,
    }

    enum MockErrorKind {
        Retryable,
        NonRetryable,
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn id(&self) -> &str {
            "mock"
        }
        fn name(&self) -> &str {
            "Mock"
        }
        fn supported_models(&self) -> Vec<ModelInfo> {
            vec![ModelInfo::new("mock-model", "Mock Model", 100_000)]
        }

        async fn chat_stream(
            &self,
            _messages: &[ChatMessage],
            _tools: Vec<ToolDef>,
            _config: &LlmConfig,
        ) -> crate::Result<LlmEventStream> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_count {
                match self.error_kind {
                    MockErrorKind::Retryable => Err(LlmError::NetworkError("transient".into())),
                    MockErrorKind::NonRetryable => Err(LlmError::ConfigError("permanent".into())),
                }
            } else {
                let s = stream::once(async { Ok(LlmEvent::TextDelta("hello".into())) });
                Ok(Box::pin(s))
            }
        }
    }

    fn fast_retry_config(max_retries: u32) -> RetryConfig {
        RetryConfig {
            max_retries,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            backoff_factor: 1.0,
        }
    }

    #[tokio::test]
    async fn success_on_first_attempt() {
        let calls = Arc::new(AtomicU32::new(0));
        let inner = Arc::new(MockProvider {
            call_count: calls.clone(),
            fail_count: 0,
            error_kind: MockErrorKind::Retryable,
        });
        let provider = InstrumentedProvider::new(inner).with_retry(fast_retry_config(3));

        let result = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(result.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.metrics().first_attempt_successes.load(Ordering::Relaxed), 1);
        assert_eq!(provider.metrics().total_retries.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn retries_on_retryable_error() {
        let calls = Arc::new(AtomicU32::new(0));
        let inner = Arc::new(MockProvider {
            call_count: calls.clone(),
            fail_count: 2,
            error_kind: MockErrorKind::Retryable,
        });
        let provider = InstrumentedProvider::new(inner).with_retry(fast_retry_config(5));

        let result = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(result.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(provider.metrics().retry_successes.load(Ordering::Relaxed), 1);
        assert_eq!(provider.metrics().total_retries.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn no_retry_on_non_retryable_error() {
        let calls = Arc::new(AtomicU32::new(0));
        let inner = Arc::new(MockProvider {
            call_count: calls.clone(),
            fail_count: 5,
            error_kind: MockErrorKind::NonRetryable,
        });
        let provider = InstrumentedProvider::new(inner).with_retry(fast_retry_config(5));

        let result = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn exhausts_retries() {
        let calls = Arc::new(AtomicU32::new(0));
        let inner = Arc::new(MockProvider {
            call_count: calls.clone(),
            fail_count: 100,
            error_kind: MockErrorKind::Retryable,
        });
        let provider = InstrumentedProvider::new(inner).with_retry(fast_retry_config(2));

        let result = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(provider.metrics().exhausted_failures.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn retry_notifications_sent() {
        let calls = Arc::new(AtomicU32::new(0));
        let inner = Arc::new(MockProvider {
            call_count: calls.clone(),
            fail_count: 2,
            error_kind: MockErrorKind::Retryable,
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let provider = InstrumentedProvider::new(inner)
            .with_retry(fast_retry_config(5))
            .with_retry_notifications(tx);

        let _ = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;

        let n1 = rx.try_recv().expect("expected first notification");
        assert_eq!(n1.attempt, 1);
        let n2 = rx.try_recv().expect("expected second notification");
        assert_eq!(n2.attempt, 2);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn timeout_triggers_retry() {
        struct SlowProvider;

        #[async_trait]
        impl LlmProvider for SlowProvider {
            fn id(&self) -> &str {
                "slow"
            }
            fn name(&self) -> &str {
                "Slow"
            }
            fn supported_models(&self) -> Vec<ModelInfo> {
                vec![]
            }
            async fn chat_stream(
                &self,
                _messages: &[ChatMessage],
                _tools: Vec<ToolDef>,
                _config: &LlmConfig,
            ) -> crate::Result<LlmEventStream> {
                tokio::time::sleep(Duration::from_secs(60)).await;
                unreachable!()
            }
        }

        let provider = InstrumentedProvider::new(Arc::new(SlowProvider))
            .with_retry(fast_retry_config(1))
            .with_timeout(Duration::from_millis(10));

        let result = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(matches!(result, Err(LlmError::Timeout(_))));
    }

    #[tokio::test]
    async fn auth_rotator_records_success() {
        let calls = Arc::new(AtomicU32::new(0));
        let inner = Arc::new(MockProvider {
            call_count: calls.clone(),
            fail_count: 0,
            error_kind: MockErrorKind::Retryable,
        });

        let rotator =
            Arc::new(crate::auth::AuthRotator::new(vec![forge_config::AuthConfig::Bearer {
                token: "key1".to_string(),
            }]));

        let provider = InstrumentedProvider::new(inner.clone())
            .with_retry(fast_retry_config(3))
            .with_auth_rotator(rotator.clone(), vec![inner]);

        let result = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(result.is_ok());

        let stats = rotator.stats();
        assert_eq!(stats[0].0, 1);
        assert_eq!(stats[0].1, 0);
    }

    #[tokio::test]
    async fn auth_rotator_tracks_failures_on_exhaustion() {
        let calls = Arc::new(AtomicU32::new(0));
        let inner = Arc::new(MockProvider {
            call_count: calls.clone(),
            fail_count: 100,
            error_kind: MockErrorKind::Retryable,
        });

        let rotator =
            Arc::new(crate::auth::AuthRotator::new(vec![forge_config::AuthConfig::Bearer {
                token: "key1".to_string(),
            }]));

        let provider = InstrumentedProvider::new(inner.clone())
            .with_retry(fast_retry_config(2))
            .with_auth_rotator(rotator.clone(), vec![inner]);

        let result = provider.chat_stream(&[], vec![], &LlmConfig::default()).await;
        assert!(result.is_err());

        let stats = rotator.stats();
        assert_eq!(stats[0].0, 0);
    }

    #[tokio::test]
    async fn with_auth_rotator_builder() {
        let inner: Arc<dyn LlmProvider> = Arc::new(MockProvider {
            call_count: Arc::new(AtomicU32::new(0)),
            fail_count: 0,
            error_kind: MockErrorKind::Retryable,
        });

        let rotator =
            Arc::new(crate::auth::AuthRotator::new(vec![forge_config::AuthConfig::Bearer {
                token: "k".to_string(),
            }]));

        let provider = InstrumentedProvider::new(inner.clone())
            .with_auth_rotator(rotator.clone(), vec![inner]);

        assert!(provider.auth_rotator().is_some());
        assert!(Arc::ptr_eq(provider.auth_rotator().unwrap(), &rotator));
    }

    #[test]
    fn llm_config_default_has_no_response_schema() {
        let config = LlmConfig::default();
        assert!(config.response_schema.is_none());
    }

    #[test]
    fn llm_config_response_schema_serde_roundtrip() {
        let config = LlmConfig {
            response_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "answer": {"type": "string"}
                },
                "required": ["answer"]
            })),
            ..Default::default()
        };

        let json = serde_json::to_string(&config).expect("serialize");
        let deserialized: LlmConfig = serde_json::from_str(&json).expect("deserialize");
        assert!(deserialized.response_schema.is_some());
        let schema = deserialized.response_schema.unwrap();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["required"][0], "answer");
    }
}
