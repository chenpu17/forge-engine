//! Circuit breaker for MCP server health management.
//!
//! Implements the circuit breaker pattern to prevent repeated calls to
//! failing MCP servers. When a server accumulates too many consecutive
//! failures, the breaker opens and subsequent calls fail fast without
//! attempting the actual request.
//!
//! State transitions:
//! ```text
//! Closed ──(failures >= threshold)──► Open
//!   ▲                                   │
//!   │                            (reset_timeout elapsed)
//!   │                                   ▼
//!   └──────(success)──────────── HalfOpen
//!                                   │
//!                            (failure)
//!                                   ▼
//!                                 Open
//! ```

use std::time::{Duration, Instant};

/// Circuit breaker states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests pass through.
    Closed,
    /// Too many failures — requests are rejected immediately.
    Open {
        /// When the breaker tripped open.
        since: Instant,
    },
    /// Tentatively allowing one request to probe recovery.
    HalfOpen,
}

/// Circuit breaker for a single MCP server.
#[derive(Debug)]
pub struct CircuitBreaker {
    state: CircuitState,
    /// Consecutive failure count (reset on success).
    consecutive_failures: usize,
    /// Number of failures before opening the circuit.
    failure_threshold: usize,
    /// How long to stay open before transitioning to half-open.
    reset_timeout: Duration,
    /// Server name (for logging).
    server_name: String,
    /// Whether a half-open probe request is currently in-flight.
    half_open_probe_in_flight: bool,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with default thresholds.
    #[must_use]
    pub fn new(server_name: impl Into<String>) -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            failure_threshold: 3,
            reset_timeout: Duration::from_secs(30),
            server_name: server_name.into(),
            half_open_probe_in_flight: false,
        }
    }

    /// Create a circuit breaker with custom thresholds.
    #[must_use]
    pub fn with_config(
        server_name: impl Into<String>,
        failure_threshold: usize,
        reset_timeout: Duration,
    ) -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            failure_threshold,
            reset_timeout,
            server_name: server_name.into(),
            half_open_probe_in_flight: false,
        }
    }

    /// Check whether a request should be allowed.
    ///
    /// Returns `Ok(())` if the request can proceed, or `Err(message)` if
    /// the circuit is open and the request should be rejected.
    ///
    /// # Errors
    /// Returns an error message when the circuit is open or half-open with a probe in flight.
    pub fn allow_request(&mut self) -> std::result::Result<(), String> {
        match &self.state {
            CircuitState::Closed => Ok(()),
            CircuitState::HalfOpen => {
                if self.half_open_probe_in_flight {
                    Err(format!(
                        "MCP server '{}' circuit breaker is half-open (probe in progress). Retry shortly.",
                        self.server_name
                    ))
                } else {
                    self.half_open_probe_in_flight = true;
                    Ok(())
                }
            }
            CircuitState::Open { since } => {
                let elapsed = since.elapsed();
                if elapsed >= self.reset_timeout {
                    // Transition to half-open: allow one probe request
                    tracing::info!(
                        server = %self.server_name,
                        "Circuit breaker transitioning to half-open"
                    );
                    self.state = CircuitState::HalfOpen;
                    self.half_open_probe_in_flight = true;
                    Ok(())
                } else {
                    let remaining = self.reset_timeout.saturating_sub(elapsed);
                    Err(format!(
                        "MCP server '{}' circuit breaker is open ({} consecutive failures). \
                         Retry in {:.0}s.",
                        self.server_name,
                        self.consecutive_failures,
                        remaining.as_secs_f64()
                    ))
                }
            }
        }
    }

    /// Record a successful request. Resets the failure counter and closes
    /// the circuit.
    pub fn record_success(&mut self) {
        if self.state != CircuitState::Closed {
            tracing::info!(
                server = %self.server_name,
                "Circuit breaker closing after successful request"
            );
        }
        self.consecutive_failures = 0;
        self.state = CircuitState::Closed;
        self.half_open_probe_in_flight = false;
    }

    /// Record a failed request. May trip the circuit open.
    pub fn record_failure(&mut self) {
        match &self.state {
            CircuitState::Closed => {
                self.consecutive_failures += 1;
                if self.consecutive_failures >= self.failure_threshold {
                    tracing::warn!(
                        server = %self.server_name,
                        failures = self.consecutive_failures,
                        threshold = self.failure_threshold,
                        "Circuit breaker opening"
                    );
                    self.state = CircuitState::Open { since: Instant::now() };
                    self.half_open_probe_in_flight = false;
                }
            }
            CircuitState::HalfOpen => {
                self.consecutive_failures += 1;
                // Probe failed — go back to open
                tracing::warn!(
                    server = %self.server_name,
                    "Circuit breaker re-opening after half-open probe failure"
                );
                self.state = CircuitState::Open { since: Instant::now() };
                self.half_open_probe_in_flight = false;
            }
            CircuitState::Open { .. } => {
                // Already open — don't increment; the counter reflects the
                // streak that *caused* the trip and is only meaningful when
                // compared against the threshold.
            }
        }
    }

    /// Current state of the circuit breaker.
    #[must_use]
    pub const fn state(&self) -> &CircuitState {
        &self.state
    }

    /// Number of consecutive failures.
    #[must_use]
    pub const fn consecutive_failures(&self) -> usize {
        self.consecutive_failures
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn new_breaker_is_closed() {
        let cb = CircuitBreaker::new("test-server");
        assert_eq!(*cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn allows_requests_when_closed() {
        let mut cb = CircuitBreaker::new("test-server");
        assert!(cb.allow_request().is_ok());
    }

    #[test]
    fn opens_after_threshold_failures() {
        let mut cb = CircuitBreaker::with_config("test-server", 3, Duration::from_secs(30));
        cb.record_failure();
        cb.record_failure();
        assert!(cb.allow_request().is_ok()); // still closed at 2 failures

        cb.record_failure(); // 3rd failure → opens
        assert!(matches!(cb.state(), CircuitState::Open { .. }));
        assert!(cb.allow_request().is_err());
    }

    #[test]
    fn rejects_requests_when_open() {
        let mut cb = CircuitBreaker::with_config("test-server", 1, Duration::from_secs(60));
        cb.record_failure();
        let err = cb.allow_request().expect_err("should be open");
        assert!(err.contains("circuit breaker is open"));
        assert!(err.contains("test-server"));
    }

    #[test]
    fn success_resets_failure_count() {
        let mut cb = CircuitBreaker::with_config("test-server", 3, Duration::from_secs(30));
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.consecutive_failures(), 2);

        cb.record_success();
        assert_eq!(cb.consecutive_failures(), 0);
        assert_eq!(*cb.state(), CircuitState::Closed);
    }

    #[test]
    fn transitions_to_half_open_after_timeout() {
        let mut cb = CircuitBreaker::with_config("test-server", 1, Duration::from_millis(1));
        cb.record_failure(); // opens
        assert!(matches!(cb.state(), CircuitState::Open { .. }));

        // Wait for reset timeout
        std::thread::sleep(Duration::from_millis(5));

        // Should transition to half-open and allow the request
        assert!(cb.allow_request().is_ok());
        assert_eq!(*cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn half_open_success_closes_circuit() {
        let mut cb = CircuitBreaker::with_config("test-server", 1, Duration::from_millis(1));
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(5));
        let _ = cb.allow_request(); // transitions to half-open

        cb.record_success();
        assert_eq!(*cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn half_open_failure_reopens_circuit() {
        let mut cb = CircuitBreaker::with_config("test-server", 1, Duration::from_millis(1));
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(5));
        let _ = cb.allow_request(); // transitions to half-open

        cb.record_failure();
        assert!(matches!(cb.state(), CircuitState::Open { .. }));
    }

    #[test]
    fn half_open_allows_only_single_probe_request() {
        let mut cb = CircuitBreaker::with_config("test-server", 1, Duration::from_millis(1));
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(5));

        // First request after timeout is the probe.
        assert!(cb.allow_request().is_ok());
        // Additional concurrent probes should be rejected.
        let err = cb.allow_request().expect_err("should reject second probe");
        assert!(err.contains("half-open"));
    }

    #[test]
    fn custom_config_respected() {
        let cb = CircuitBreaker::with_config("custom", 5, Duration::from_secs(120));
        assert_eq!(cb.failure_threshold, 5);
        assert_eq!(cb.reset_timeout, Duration::from_secs(120));
    }
}
