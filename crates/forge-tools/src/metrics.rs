//! Tool execution metrics.
//!
//! Provides lightweight, lock-free counters for tracking tool call statistics.
//! Integrated into `ToolExecutor` to automatically record every call.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use dashmap::DashMap;

/// Per-tool call statistics (lock-free atomics).
#[derive(Debug, Default)]
pub struct ToolCallStats {
    /// Total number of calls.
    pub total_calls: AtomicUsize,
    /// Number of calls that returned an error.
    pub error_count: AtomicUsize,
    /// Cumulative execution duration in milliseconds.
    pub total_duration_ms: AtomicU64,
}

impl ToolCallStats {
    /// Record a completed call.
    pub fn record(&self, duration_ms: u64, is_error: bool) {
        self.total_calls.fetch_add(1, Ordering::Relaxed);
        self.total_duration_ms.fetch_add(duration_ms, Ordering::Relaxed);
        if is_error {
            self.error_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Average duration in milliseconds (0 if no calls).
    pub fn avg_duration_ms(&self) -> u64 {
        let total = self.total_calls.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        self.total_duration_ms.load(Ordering::Relaxed) / total as u64
    }

    /// Error rate as a fraction (0.0–1.0).
    #[allow(clippy::cast_precision_loss)]
    pub fn error_rate(&self) -> f64 {
        let total = self.total_calls.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        self.error_count.load(Ordering::Relaxed) as f64 / total as f64
    }
}

/// Aggregated metrics across all tools.
#[derive(Debug, Default)]
pub struct ToolMetrics {
    /// Per-tool statistics keyed by tool name.
    calls: DashMap<String, Arc<ToolCallStats>>,
}

impl ToolMetrics {
    /// Create a new metrics collector.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a tool call.
    pub fn record(&self, tool_name: &str, duration_ms: u64, is_error: bool) {
        let stats = self
            .calls
            .entry(tool_name.to_string())
            .or_insert_with(|| Arc::new(ToolCallStats::default()));
        stats.record(duration_ms, is_error);
    }

    /// Get stats for a specific tool.
    #[must_use]
    pub fn get(&self, tool_name: &str) -> Option<Arc<ToolCallStats>> {
        self.calls.get(tool_name).map(|r| r.value().clone())
    }

    /// Snapshot of all tool stats as a plain `HashMap`.
    #[must_use]
    pub fn snapshot(&self) -> HashMap<String, ToolStatsSnapshot> {
        self.calls
            .iter()
            .map(|entry| {
                let name = entry.key().clone();
                let stats = entry.value();
                let snap = ToolStatsSnapshot {
                    total_calls: stats.total_calls.load(Ordering::Relaxed),
                    error_count: stats.error_count.load(Ordering::Relaxed),
                    total_duration_ms: stats.total_duration_ms.load(Ordering::Relaxed),
                };
                (name, snap)
            })
            .collect()
    }

    /// Total calls across all tools.
    #[must_use]
    pub fn total_calls(&self) -> usize {
        self.calls.iter().map(|e| e.value().total_calls.load(Ordering::Relaxed)).sum()
    }

    /// Total errors across all tools.
    #[must_use]
    pub fn total_errors(&self) -> usize {
        self.calls.iter().map(|e| e.value().error_count.load(Ordering::Relaxed)).sum()
    }
}

/// Non-atomic snapshot of a tool's stats (for serialization / display).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolStatsSnapshot {
    /// Total number of calls.
    pub total_calls: usize,
    /// Number of calls that returned an error.
    pub error_count: usize,
    /// Cumulative execution duration in milliseconds.
    pub total_duration_ms: u64,
}

impl ToolStatsSnapshot {
    /// Average duration in milliseconds.
    #[must_use]
    pub const fn avg_duration_ms(&self) -> u64 {
        if self.total_calls == 0 {
            return 0;
        }
        self.total_duration_ms / self.total_calls as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_retrieve() {
        let metrics = ToolMetrics::new();
        metrics.record("bash", 100, false);
        metrics.record("bash", 200, true);
        metrics.record("read", 50, false);

        let bash = metrics.get("bash").expect("bash stats");
        assert_eq!(bash.total_calls.load(Ordering::Relaxed), 2);
        assert_eq!(bash.error_count.load(Ordering::Relaxed), 1);
        assert_eq!(bash.total_duration_ms.load(Ordering::Relaxed), 300);
        assert_eq!(bash.avg_duration_ms(), 150);

        let read = metrics.get("read").expect("read stats");
        assert_eq!(read.total_calls.load(Ordering::Relaxed), 1);
        assert_eq!(read.error_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn snapshot_captures_all() {
        let metrics = ToolMetrics::new();
        metrics.record("a", 10, false);
        metrics.record("b", 20, true);

        let snap = metrics.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap["a"].total_calls, 1);
        assert_eq!(snap["b"].error_count, 1);
    }

    #[test]
    fn totals() {
        let metrics = ToolMetrics::new();
        metrics.record("a", 10, false);
        metrics.record("a", 20, true);
        metrics.record("b", 30, false);

        assert_eq!(metrics.total_calls(), 3);
        assert_eq!(metrics.total_errors(), 1);
    }

    #[test]
    fn error_rate() {
        let stats = ToolCallStats::default();
        assert_eq!(stats.error_rate(), 0.0);

        stats.record(10, false);
        stats.record(10, true);
        assert!((stats.error_rate() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn missing_tool_returns_none() {
        let metrics = ToolMetrics::new();
        assert!(metrics.get("nonexistent").is_none());
    }

    #[test]
    fn snapshot_avg_duration() {
        let snap = ToolStatsSnapshot { total_calls: 4, error_count: 1, total_duration_ms: 400 };
        assert_eq!(snap.avg_duration_ms(), 100);

        let empty = ToolStatsSnapshot { total_calls: 0, error_count: 0, total_duration_ms: 0 };
        assert_eq!(empty.avg_duration_ms(), 0);
    }
}
