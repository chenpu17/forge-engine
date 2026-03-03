//! Retry logic and request deduplication for LLM operations

use crate::Result;
use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Retry configuration
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Initial delay between retries
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Exponential backoff factor
    pub backoff_factor: f32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 10,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            backoff_factor: 2.0,
        }
    }
}

/// Retry handler for executing operations with automatic retry
pub struct RetryHandler {
    config: RetryConfig,
}

impl RetryHandler {
    /// Create a new retry handler
    pub const fn new(config: RetryConfig) -> Self {
        Self { config }
    }

    /// Execute an operation with retry logic
    pub async fn execute<F, Fut, T>(&self, mut operation: F) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut attempt = 0;
        let mut delay = self.config.initial_delay;

        loop {
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) if e.is_retryable() && attempt < self.config.max_retries => {
                    attempt += 1;
                    let wait_time = e.retry_delay().unwrap_or(delay);
                    let wait_time = wait_time.min(self.config.max_delay);

                    tracing::warn!(
                        attempt = attempt,
                        max_retries = self.config.max_retries,
                        wait_ms = wait_time.as_millis(),
                        error = %e,
                        "Retrying after error"
                    );

                    tokio::time::sleep(wait_time).await;
                    delay =
                        Duration::from_secs_f32(delay.as_secs_f32() * self.config.backoff_factor);
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Default for RetryHandler {
    fn default() -> Self {
        Self::new(RetryConfig::default())
    }
}

// =============================================================================
// Request Deduplication
// =============================================================================

/// Request deduplication status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeduplicationStatus {
    /// This is a new request, proceed with execution
    New,
    /// This request is a duplicate of a pending request
    Duplicate,
    /// This request was recently completed
    RecentlyCompleted,
}

/// Configuration for request deduplication
#[derive(Debug, Clone)]
pub struct DeduplicationConfig {
    /// How long to track completed requests
    pub completed_ttl: Duration,
    /// How long before a pending request is considered stale
    pub pending_timeout: Duration,
    /// Maximum number of entries to track
    pub max_entries: usize,
}

impl Default for DeduplicationConfig {
    fn default() -> Self {
        Self {
            completed_ttl: Duration::from_secs(5),
            pending_timeout: Duration::from_secs(120),
            max_entries: 1000,
        }
    }
}

/// Entry in the deduplication cache
#[derive(Debug, Clone)]
struct DeduplicationEntry {
    created_at: Instant,
    is_pending: bool,
}

/// Request deduplicator to prevent duplicate LLM requests
#[derive(Debug)]
pub struct RequestDeduplicator {
    config: DeduplicationConfig,
    cache: Arc<RwLock<HashMap<u64, DeduplicationEntry>>>,
}

impl RequestDeduplicator {
    /// Create a new request deduplicator
    pub fn new(config: DeduplicationConfig) -> Self {
        Self { config, cache: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// Compute a hash for a request
    pub fn compute_hash<T: Hash>(request: &T) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        request.hash(&mut hasher);
        hasher.finish()
    }

    /// Check if a request should proceed or is a duplicate
    pub async fn check(&self, request_hash: u64) -> DeduplicationStatus {
        let now = Instant::now();

        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(&request_hash) {
                if entry.is_pending {
                    if now.duration_since(entry.created_at) < self.config.pending_timeout {
                        return DeduplicationStatus::Duplicate;
                    }
                } else if now.duration_since(entry.created_at) < self.config.completed_ttl {
                    return DeduplicationStatus::RecentlyCompleted;
                }
            }
        }

        let mut cache = self.cache.write().await;

        if let Some(entry) = cache.get(&request_hash) {
            if entry.is_pending
                && now.duration_since(entry.created_at) < self.config.pending_timeout
            {
                return DeduplicationStatus::Duplicate;
            }
            if !entry.is_pending && now.duration_since(entry.created_at) < self.config.completed_ttl
            {
                return DeduplicationStatus::RecentlyCompleted;
            }
        }

        if cache.len() >= self.config.max_entries {
            self.cleanup_cache(&mut cache, now);
        }

        cache.insert(request_hash, DeduplicationEntry { created_at: now, is_pending: true });
        DeduplicationStatus::New
    }

    /// Mark a request as completed
    pub async fn complete(&self, request_hash: u64) {
        let mut cache = self.cache.write().await;
        if let Some(entry) = cache.get_mut(&request_hash) {
            entry.is_pending = false;
            entry.created_at = Instant::now();
        }
    }

    /// Cancel a pending request
    pub async fn cancel(&self, request_hash: u64) {
        let mut cache = self.cache.write().await;
        cache.remove(&request_hash);
    }

    fn cleanup_cache(&self, cache: &mut HashMap<u64, DeduplicationEntry>, now: Instant) {
        let pending_timeout = self.config.pending_timeout;
        let completed_ttl = self.config.completed_ttl;

        cache.retain(|_, entry| {
            let age = now.duration_since(entry.created_at);
            if entry.is_pending {
                age < pending_timeout
            } else {
                age < completed_ttl
            }
        });
    }

    /// Get the current cache size
    pub async fn cache_size(&self) -> usize {
        self.cache.read().await.len()
    }

    /// Clear all entries
    pub async fn clear(&self) {
        self.cache.write().await.clear();
    }
}

impl Default for RequestDeduplicator {
    fn default() -> Self {
        Self::new(DeduplicationConfig::default())
    }
}

impl Clone for RequestDeduplicator {
    fn clone(&self) -> Self {
        Self { config: self.config.clone(), cache: Arc::clone(&self.cache) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LlmError;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_retry_success_on_first_try() {
        let handler = RetryHandler::default();
        let result = handler.execute(|| async { Ok::<_, LlmError>(42) }).await;
        assert_eq!(result.expect("ok"), 42);
    }

    #[tokio::test]
    async fn test_retry_success_after_failures() {
        let handler = RetryHandler::new(RetryConfig {
            max_retries: 3,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            backoff_factor: 2.0,
        });

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = handler
            .execute(|| {
                let attempts = attempts_clone.clone();
                async move {
                    let count = attempts.fetch_add(1, Ordering::SeqCst);
                    if count < 2 {
                        Err(LlmError::NetworkError("temporary".into()))
                    } else {
                        Ok(42)
                    }
                }
            })
            .await;

        assert_eq!(result.expect("ok"), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let handler = RetryHandler::new(RetryConfig {
            max_retries: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            backoff_factor: 2.0,
        });

        let result: Result<i32> =
            handler.execute(|| async { Err(LlmError::NetworkError("permanent".into())) }).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_no_retry_for_non_retryable() {
        let handler = RetryHandler::default();
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result: Result<i32> = handler
            .execute(|| {
                let attempts = attempts_clone.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err(LlmError::ConfigError("not retryable".into()))
                }
            })
            .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_deduplicator_new_request() {
        let dedup = RequestDeduplicator::default();
        let status = dedup.check(12345u64).await;
        assert_eq!(status, DeduplicationStatus::New);
        assert_eq!(dedup.cache_size().await, 1);
    }

    #[tokio::test]
    async fn test_deduplicator_duplicate_request() {
        let dedup = RequestDeduplicator::default();
        let hash = 12345u64;
        assert_eq!(dedup.check(hash).await, DeduplicationStatus::New);
        assert_eq!(dedup.check(hash).await, DeduplicationStatus::Duplicate);
    }

    #[tokio::test]
    async fn test_deduplicator_completed_request() {
        let dedup = RequestDeduplicator::new(DeduplicationConfig {
            completed_ttl: Duration::from_secs(60),
            pending_timeout: Duration::from_secs(120),
            max_entries: 1000,
        });
        let hash = 12345u64;
        assert_eq!(dedup.check(hash).await, DeduplicationStatus::New);
        dedup.complete(hash).await;
        assert_eq!(dedup.check(hash).await, DeduplicationStatus::RecentlyCompleted);
    }

    #[tokio::test]
    async fn test_deduplicator_cancelled_request() {
        let dedup = RequestDeduplicator::default();
        let hash = 12345u64;
        assert_eq!(dedup.check(hash).await, DeduplicationStatus::New);
        dedup.cancel(hash).await;
        assert_eq!(dedup.check(hash).await, DeduplicationStatus::New);
    }

    #[tokio::test]
    async fn test_deduplicator_clone_shares_cache() {
        let dedup1 = RequestDeduplicator::default();
        let dedup2 = dedup1.clone();
        let hash = 12345u64;
        assert_eq!(dedup1.check(hash).await, DeduplicationStatus::New);
        assert_eq!(dedup2.check(hash).await, DeduplicationStatus::Duplicate);
    }

    #[test]
    fn test_compute_hash() {
        let hash1 = RequestDeduplicator::compute_hash(&"hello world");
        let hash2 = RequestDeduplicator::compute_hash(&"hello world");
        assert_eq!(hash1, hash2);
        let hash3 = RequestDeduplicator::compute_hash(&"different");
        assert_ne!(hash1, hash3);
    }
}
