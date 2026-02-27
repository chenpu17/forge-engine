//! Credential rotation and failover for LLM providers.
//!
//! [`AuthRotator`] manages multiple credentials for the same provider,
//! rotating through them on auth failures (401/403) and applying cooldowns
//! on rate-limit errors (429).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use forge_config::AuthConfig;
use parking_lot::RwLock;

/// Default cooldown duration when a credential hits a rate limit.
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);

/// Per-credential health tracking.
#[derive(Debug)]
struct CredentialState {
    /// The credential configuration.
    auth: AuthConfig,
    /// Cumulative successful calls.
    successes: AtomicU64,
    /// Cumulative failed calls.
    failures: AtomicU64,
    /// Instant when cooldown expires (`None` = available).
    cooldown_until: RwLock<Option<Instant>>,
}

impl CredentialState {
    const fn new(auth: AuthConfig) -> Self {
        Self {
            auth,
            successes: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            cooldown_until: RwLock::new(None),
        }
    }

    /// Whether this credential is currently available (not in cooldown).
    fn is_available(&self) -> bool {
        let guard = self.cooldown_until.read();
        guard.map_or(true, |until| Instant::now() >= until)
    }

    fn record_success(&self) {
        self.successes.fetch_add(1, Ordering::Relaxed);
        *self.cooldown_until.write() = None;
    }

    fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    fn apply_cooldown(&self, duration: Duration) {
        *self.cooldown_until.write() = Some(Instant::now() + duration);
    }
}

/// Manages multiple credentials for a single provider with round-robin
/// rotation and per-credential cooldown.
pub struct AuthRotator {
    credentials: Vec<Arc<CredentialState>>,
    /// Current index (round-robin).
    index: AtomicUsize,
    /// Cooldown duration for rate-limited credentials.
    cooldown: Duration,
}

impl AuthRotator {
    /// Create a rotator from a list of credentials.
    ///
    /// Panics if `credentials` is empty.
    pub fn new(credentials: Vec<AuthConfig>) -> Self {
        assert!(!credentials.is_empty(), "AuthRotator requires at least one credential");
        Self {
            credentials: credentials
                .into_iter()
                .map(|a| Arc::new(CredentialState::new(a)))
                .collect(),
            index: AtomicUsize::new(0),
            cooldown: DEFAULT_COOLDOWN,
        }
    }

    /// Create a rotator from an [`AuthConfig`], automatically flattening
    /// `Multi` variants.
    pub fn from_auth_config(config: &AuthConfig) -> Option<Self> {
        let flat: Vec<AuthConfig> = config.flatten().into_iter().cloned().collect();
        if flat.is_empty() {
            return None;
        }
        Some(Self::new(flat))
    }

    /// Override the default cooldown duration.
    #[must_use]
    pub const fn with_cooldown(mut self, duration: Duration) -> Self {
        self.cooldown = duration;
        self
    }

    /// Number of credentials managed.
    pub fn len(&self) -> usize {
        self.credentials.len()
    }

    /// Whether the rotator has no credentials.
    pub fn is_empty(&self) -> bool {
        self.credentials.is_empty()
    }

    /// Get the current credential without advancing.
    pub fn current(&self) -> &AuthConfig {
        let idx = self.current_index();
        &self.credentials[idx].auth
    }

    /// Get the current credential index (0-based).
    pub fn current_index(&self) -> usize {
        self.index.load(Ordering::Relaxed) % self.credentials.len()
    }

    /// Advance to the next available credential and return it.
    pub fn next(&self) -> &AuthConfig {
        let len = self.credentials.len();
        if len == 1 {
            return &self.credentials[0].auth;
        }

        loop {
            let current = self.index.load(Ordering::Relaxed);
            let start = current % len;

            let mut target = (start + 1) % len;
            let mut found_available = false;
            for _ in 0..len {
                if self.credentials[target].is_available() {
                    found_available = true;
                    break;
                }
                target = (target + 1) % len;
            }

            if !found_available {
                target = (start + 1) % len;
            }

            if self
                .index
                .compare_exchange(current, target, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return &self.credentials[target].auth;
            }
        }
    }

    /// Record a successful call on the current credential.
    pub fn record_success(&self) {
        let idx = self.index.load(Ordering::Relaxed) % self.credentials.len();
        self.credentials[idx].record_success();
    }

    /// Record a successful call on a specific credential index.
    pub fn record_success_at(&self, idx: usize) {
        let idx = idx % self.credentials.len();
        self.credentials[idx].record_success();
    }

    /// Record an auth failure (401/403) on the current credential and advance.
    pub fn record_auth_failure(&self) -> &AuthConfig {
        let idx = self.index.load(Ordering::Relaxed) % self.credentials.len();
        self.credentials[idx].record_failure();
        self.next()
    }

    /// Record an auth failure on a specific credential index and advance.
    pub fn record_auth_failure_at(&self, idx: usize) -> &AuthConfig {
        let idx = idx % self.credentials.len();
        self.credentials[idx].record_failure();
        self.next()
    }

    /// Record a rate-limit (429) on the current credential, apply cooldown, and advance.
    pub fn record_rate_limited(&self) -> &AuthConfig {
        let idx = self.index.load(Ordering::Relaxed) % self.credentials.len();
        self.credentials[idx].record_failure();
        self.credentials[idx].apply_cooldown(self.cooldown);
        self.next()
    }

    /// Record a rate-limit on a specific credential index, apply cooldown, and advance.
    pub fn record_rate_limited_at(&self, idx: usize) -> &AuthConfig {
        let idx = idx % self.credentials.len();
        self.credentials[idx].record_failure();
        self.credentials[idx].apply_cooldown(self.cooldown);
        self.next()
    }

    /// Get per-credential stats: `(successes, failures, in_cooldown)`.
    pub fn stats(&self) -> Vec<(u64, u64, bool)> {
        self.credentials
            .iter()
            .map(|c| {
                (
                    c.successes.load(Ordering::Relaxed),
                    c.failures.load(Ordering::Relaxed),
                    !c.is_available(),
                )
            })
            .collect()
    }

    /// Check if any credential is available (not in cooldown).
    pub fn has_available(&self) -> bool {
        self.credentials.iter().any(|c| c.is_available())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bearer(token: &str) -> AuthConfig {
        AuthConfig::Bearer { token: token.to_string() }
    }

    fn token_of(auth: &AuthConfig) -> &str {
        match auth {
            AuthConfig::Bearer { token } => token,
            _ => panic!("expected Bearer"),
        }
    }

    #[test]
    fn single_credential_always_returns_same() {
        let rotator = AuthRotator::new(vec![bearer("key-1")]);
        assert_eq!(token_of(rotator.current()), "key-1");
        assert_eq!(token_of(rotator.next()), "key-1");
    }

    #[test]
    fn round_robin_rotation() {
        let rotator = AuthRotator::new(vec![bearer("a"), bearer("b"), bearer("c")]);
        assert_eq!(token_of(rotator.current()), "a");
        assert_eq!(token_of(rotator.next()), "b");
        assert_eq!(token_of(rotator.next()), "c");
        assert_eq!(token_of(rotator.next()), "a");
    }

    #[test]
    fn auth_failure_advances() {
        let rotator = AuthRotator::new(vec![bearer("a"), bearer("b")]);
        assert_eq!(token_of(rotator.current()), "a");
        let next = rotator.record_auth_failure();
        assert_eq!(token_of(next), "b");
    }

    #[test]
    fn rate_limit_applies_cooldown() {
        let rotator =
            AuthRotator::new(vec![bearer("a"), bearer("b")]).with_cooldown(Duration::from_secs(60));
        assert_eq!(token_of(rotator.current()), "a");
        let next = rotator.record_rate_limited();
        assert_eq!(token_of(next), "b");

        let stats = rotator.stats();
        assert!(stats[0].2, "credential 'a' should be in cooldown");
        assert!(!stats[1].2, "credential 'b' should be available");
    }

    #[test]
    fn success_clears_cooldown() {
        let rotator =
            AuthRotator::new(vec![bearer("a"), bearer("b")]).with_cooldown(Duration::from_secs(60));
        rotator.record_rate_limited();
        assert!(rotator.stats()[0].2);

        rotator.index.store(0, Ordering::Relaxed);
        rotator.record_success();
        assert!(!rotator.stats()[0].2, "cooldown should be cleared after success");
    }

    #[test]
    fn from_auth_config_multi() {
        let multi = AuthConfig::Multi { credentials: vec![bearer("x"), bearer("y")] };
        let rotator = AuthRotator::from_auth_config(&multi).expect("should create");
        assert_eq!(rotator.len(), 2);
    }

    #[test]
    fn from_auth_config_single() {
        let single = bearer("z");
        let rotator = AuthRotator::from_auth_config(&single).expect("should create");
        assert_eq!(rotator.len(), 1);
    }

    #[test]
    fn from_auth_config_none_returns_none() {
        assert!(AuthRotator::from_auth_config(&AuthConfig::None).is_none());
    }

    #[test]
    fn stats_tracks_successes_and_failures() {
        let rotator = AuthRotator::new(vec![bearer("a"), bearer("b")]);
        rotator.record_success();
        rotator.record_success();
        rotator.record_auth_failure();

        let stats = rotator.stats();
        assert_eq!(stats[0].0, 2); // successes for "a"
        assert_eq!(stats[0].1, 1); // failures for "a"
    }
}
