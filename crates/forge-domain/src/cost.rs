//! Cost tracking domain types.
//!
//! Defines pricing models, usage accumulators, and per-agent cost records
//! for real-time budget management across agent executions.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// ---------------------------------------------------------------------------
// Model pricing
// ---------------------------------------------------------------------------

/// Pricing entry for a specific model or model pattern.
///
/// `model_pattern` supports exact match or glob-style wildcards
/// (e.g. `"claude-sonnet-*"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    /// Model name or glob pattern.
    pub model_pattern: String,
    /// Cost per million input tokens (USD).
    pub input_cost_per_mtok: f64,
    /// Cost per million output tokens (USD).
    pub output_cost_per_mtok: f64,
    /// Cost per million cache-read input tokens (USD), if applicable.
    #[serde(default)]
    pub cache_read_per_mtok: Option<f64>,
    /// Cost per million cache-write input tokens (USD), if applicable.
    #[serde(default)]
    pub cache_write_per_mtok: Option<f64>,
}

impl ModelPricing {
    /// Check whether this pricing entry matches the given model name.
    ///
    /// Supports exact match and trailing-wildcard patterns (e.g. `"claude-sonnet-*"`).
    #[must_use]
    pub fn matches(&self, model: &str) -> bool {
        if let Some(prefix) = self.model_pattern.strip_suffix('*') {
            model.starts_with(prefix)
        } else {
            self.model_pattern == model
        }
    }

    /// Calculate cost in USD for the given token counts.
    #[must_use]
    pub fn calculate_cost(
        &self,
        input_tokens: usize,
        output_tokens: usize,
        cache_read_tokens: usize,
        cache_write_tokens: usize,
    ) -> f64 {
        let input = input_tokens as f64 * self.input_cost_per_mtok / 1_000_000.0;
        let output = output_tokens as f64 * self.output_cost_per_mtok / 1_000_000.0;
        let cache_read = cache_read_tokens as f64
            * self.cache_read_per_mtok.unwrap_or(self.input_cost_per_mtok) / 1_000_000.0;
        let cache_write = cache_write_tokens as f64
            * self.cache_write_per_mtok.unwrap_or(self.input_cost_per_mtok) / 1_000_000.0;
        input + output + cache_read + cache_write
    }
}

// ---------------------------------------------------------------------------
// Usage accumulator (atomic, lock-free)
// ---------------------------------------------------------------------------

/// Atomic token/cost accumulator for concurrent updates.
///
/// Cost is stored as micro-USD (1e-6 USD) in an `AtomicU64` to avoid
/// floating-point atomics while preserving sub-cent precision.
#[derive(Debug, Default)]
pub struct UsageAccumulator {
    /// Total input tokens consumed.
    pub input_tokens: AtomicUsize,
    /// Total output tokens generated.
    pub output_tokens: AtomicUsize,
    /// Total cache-read input tokens.
    pub cache_read_tokens: AtomicUsize,
    /// Total cache-write input tokens.
    pub cache_write_tokens: AtomicUsize,
    /// Estimated cost in micro-USD (1 USD = 1_000_000 micro-USD).
    pub cost_micro_usd: AtomicU64,
}

impl UsageAccumulator {
    /// Create a new zeroed accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add token usage and cost atomically.
    pub fn add(
        &self,
        input_tokens: usize,
        output_tokens: usize,
        cache_read_tokens: usize,
        cache_write_tokens: usize,
        cost_usd: f64,
    ) {
        self.input_tokens.fetch_add(input_tokens, Ordering::Relaxed);
        self.output_tokens.fetch_add(output_tokens, Ordering::Relaxed);
        self.cache_read_tokens.fetch_add(cache_read_tokens, Ordering::Relaxed);
        self.cache_write_tokens.fetch_add(cache_write_tokens, Ordering::Relaxed);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let micro = (cost_usd.max(0.0) * 1_000_000.0) as u64;
        self.cost_micro_usd.fetch_add(micro, Ordering::Relaxed);
    }

    /// Get the estimated cost in USD.
    #[must_use]
    pub fn cost_usd(&self) -> f64 {
        self.cost_micro_usd.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }

    /// Take a point-in-time snapshot of accumulated usage.
    ///
    /// **Note:** Fields are loaded independently with relaxed ordering.
    /// Under concurrent writes, the snapshot may represent a state that never
    /// actually existed (e.g., tokens from one update with cost from another).
    /// For approximate monitoring and dashboards this is acceptable.
    #[must_use]
    pub fn snapshot(&self) -> UsageSnapshot {
        UsageSnapshot {
            input_tokens: self.input_tokens.load(Ordering::Relaxed),
            output_tokens: self.output_tokens.load(Ordering::Relaxed),
            cache_read_tokens: self.cache_read_tokens.load(Ordering::Relaxed),
            cache_write_tokens: self.cache_write_tokens.load(Ordering::Relaxed),
            cost_usd: self.cost_usd(),
        }
    }
}

// ---------------------------------------------------------------------------
// Usage snapshot (serializable)
// ---------------------------------------------------------------------------

/// Serializable snapshot of a [`UsageAccumulator`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSnapshot {
    /// Total input tokens.
    pub input_tokens: usize,
    /// Total output tokens.
    pub output_tokens: usize,
    /// Total cache-read tokens.
    pub cache_read_tokens: usize,
    /// Total cache-write tokens.
    pub cache_write_tokens: usize,
    /// Estimated cost in USD.
    pub cost_usd: f64,
}

// ---------------------------------------------------------------------------
// Per-agent cost record
// ---------------------------------------------------------------------------

/// Cost record for a single agent instance.
#[derive(Debug)]
pub struct AgentCostRecord {
    /// Unique agent identifier.
    pub agent_id: String,
    /// Agent type (e.g. "explore", "plan", "general-purpose").
    pub agent_type: String,
    /// Model used by this agent.
    pub model: String,
    /// Accumulated usage.
    pub usage: UsageAccumulator,
    /// Optional budget limit in USD.
    pub budget_limit_usd: Option<f64>,
}

impl AgentCostRecord {
    /// Create a new cost record.
    #[must_use]
    pub fn new(
        agent_id: String,
        agent_type: String,
        model: String,
        budget_limit_usd: Option<f64>,
    ) -> Self {
        Self {
            agent_id,
            agent_type,
            model,
            usage: UsageAccumulator::new(),
            budget_limit_usd,
        }
    }
}

// ---------------------------------------------------------------------------
// Cost check result
// ---------------------------------------------------------------------------

/// Result of a budget check after recording usage.
#[derive(Debug, Clone, PartialEq)]
pub enum CostCheckResult {
    /// Within budget.
    Ok,
    /// Approaching budget limit (above warning threshold, typically 80%).
    Warning {
        /// Current cost in USD.
        current_usd: f64,
        /// Budget limit in USD.
        limit_usd: f64,
        /// Percentage of budget consumed.
        percentage: f64,
    },
    /// Budget exceeded.
    BudgetExceeded {
        /// Current cost in USD.
        current_usd: f64,
        /// Budget limit in USD.
        limit_usd: f64,
    },
}

// ---------------------------------------------------------------------------
// Cost snapshot (for frontend/API)
// ---------------------------------------------------------------------------

/// Snapshot of cost tracking state for external consumption.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostSnapshot {
    /// Per-agent usage summaries.
    pub agents: Vec<AgentCostSummary>,
    /// Session-wide totals.
    pub session: UsageSnapshot,
    /// Session budget limit in USD, if configured.
    pub session_budget_usd: Option<f64>,
}

/// Summary of a single agent's cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCostSummary {
    /// Agent identifier.
    pub agent_id: String,
    /// Agent type.
    pub agent_type: String,
    /// Model used.
    pub model: String,
    /// Usage snapshot.
    pub usage: UsageSnapshot,
    /// Budget limit in USD, if configured.
    pub budget_limit_usd: Option<f64>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_pricing_exact_match() {
        let pricing = ModelPricing {
            model_pattern: "claude-sonnet-4-5-20250929".to_string(),
            input_cost_per_mtok: 3.0,
            output_cost_per_mtok: 15.0,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
        };
        assert!(pricing.matches("claude-sonnet-4-5-20250929"));
        assert!(!pricing.matches("claude-opus-4"));
    }

    #[test]
    fn test_model_pricing_wildcard_match() {
        let pricing = ModelPricing {
            model_pattern: "claude-sonnet-*".to_string(),
            input_cost_per_mtok: 3.0,
            output_cost_per_mtok: 15.0,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
        };
        assert!(pricing.matches("claude-sonnet-4-5-20250929"));
        assert!(pricing.matches("claude-sonnet-3-5"));
        assert!(!pricing.matches("claude-opus-4"));
    }

    #[test]
    fn test_model_pricing_calculate_cost() {
        let pricing = ModelPricing {
            model_pattern: "test".to_string(),
            input_cost_per_mtok: 3.0,
            output_cost_per_mtok: 15.0,
            cache_read_per_mtok: Some(0.3),
            cache_write_per_mtok: Some(3.75),
        };
        // 1M input = $3, 1M output = $15
        let cost = pricing.calculate_cost(1_000_000, 1_000_000, 0, 0);
        assert!((cost - 18.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_usage_accumulator_add_and_snapshot() {
        let acc = UsageAccumulator::new();
        acc.add(100, 50, 10, 5, 0.001);
        acc.add(200, 100, 20, 10, 0.002);

        let snap = acc.snapshot();
        assert_eq!(snap.input_tokens, 300);
        assert_eq!(snap.output_tokens, 150);
        assert_eq!(snap.cache_read_tokens, 30);
        assert_eq!(snap.cache_write_tokens, 15);
        assert!((snap.cost_usd - 0.003).abs() < 0.000_01);
    }

    #[test]
    fn test_cost_check_result_variants() {
        let ok = CostCheckResult::Ok;
        assert_eq!(ok, CostCheckResult::Ok);

        let warning = CostCheckResult::Warning {
            current_usd: 4.0,
            limit_usd: 5.0,
            percentage: 80.0,
        };
        assert!(matches!(warning, CostCheckResult::Warning { .. }));

        let exceeded = CostCheckResult::BudgetExceeded {
            current_usd: 5.5,
            limit_usd: 5.0,
        };
        assert!(matches!(exceeded, CostCheckResult::BudgetExceeded { .. }));
    }

    #[test]
    fn test_agent_cost_record_new() {
        let record = AgentCostRecord::new(
            "agent-1".to_string(),
            "explore".to_string(),
            "claude-sonnet-4".to_string(),
            Some(1.0),
        );
        assert_eq!(record.agent_id, "agent-1");
        assert_eq!(record.budget_limit_usd, Some(1.0));
        assert_eq!(record.usage.cost_usd(), 0.0);
    }

    #[test]
    fn test_cost_snapshot_serde() {
        let snap = CostSnapshot {
            agents: vec![AgentCostSummary {
                agent_id: "a1".to_string(),
                agent_type: "explore".to_string(),
                model: "claude-sonnet-4".to_string(),
                usage: UsageSnapshot {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    cost_usd: 0.001,
                },
                budget_limit_usd: Some(1.0),
            }],
            session: UsageSnapshot::default(),
            session_budget_usd: Some(5.0),
        };
        let json = serde_json::to_string(&snap).expect("serialize");
        let parsed: CostSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.agents.len(), 1);
        assert_eq!(parsed.session_budget_usd, Some(5.0));
    }
}
