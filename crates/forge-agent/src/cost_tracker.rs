//! Per-agent cost tracking with budget enforcement.
//!
//! Provides real-time cost attribution for each agent (main + sub-agents),
//! with configurable budget limits and automatic circuit-breaking when
//! budgets are exceeded.

use std::sync::Arc;

use dashmap::DashMap;
use forge_config::CostConfig;
use forge_domain::cost::{
    AgentCostRecord, AgentCostSummary, CostCheckResult, CostSnapshot, ModelPricing,
    UsageAccumulator,
};
use forge_domain::Usage;

/// Central cost tracker shared across the agent and its sub-agents.
///
/// All methods are safe to call concurrently from multiple tasks
/// (lock-free via `DashMap` and atomic counters).
pub struct CostTracker {
    /// Pricing table (custom overrides first, then built-in).
    pricing: Vec<ModelPricing>,
    /// Per-agent cost records.
    agents: DashMap<String, AgentCostRecord>,
    /// Session-wide totals.
    session_total: Arc<UsageAccumulator>,
    /// Session budget in USD (None = unlimited).
    session_budget_usd: Option<f64>,
    /// Warning threshold as a fraction (0.0–1.0).
    warning_threshold: f64,
    /// Per-agent-type budget defaults.
    agent_type_budgets: std::collections::HashMap<String, f64>,
    /// Whether cost tracking is enabled.
    enabled: bool,
}

impl CostTracker {
    /// Create a new tracker from configuration.
    #[must_use]
    pub fn from_config(config: &CostConfig) -> Self {
        // Build pricing table: user overrides first, then built-in
        let mut pricing: Vec<ModelPricing> =
            config.pricing.iter().map(|p| p.to_model_pricing()).collect();
        pricing.extend(forge_config::cost::builtin_pricing());

        Self {
            pricing,
            agents: DashMap::new(),
            session_total: Arc::new(UsageAccumulator::new()),
            session_budget_usd: config.session_budget_usd,
            warning_threshold: config.warning_threshold.clamp(0.0, 1.0),
            agent_type_budgets: config.agent_budgets.clone(),
            enabled: config.enabled,
        }
    }

    /// Create a disabled (no-op) tracker.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            pricing: Vec::new(),
            agents: DashMap::new(),
            session_total: Arc::new(UsageAccumulator::new()),
            session_budget_usd: None,
            warning_threshold: CostConfig::default().warning_threshold,
            agent_type_budgets: std::collections::HashMap::new(),
            enabled: false,
        }
    }

    /// Whether tracking is enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Register a new agent for cost tracking.
    ///
    /// If `budget_usd` is `None`, the per-agent-type default is used (if configured).
    pub fn register_agent(
        &self,
        agent_id: &str,
        agent_type: &str,
        model: &str,
        budget_usd: Option<f64>,
    ) {
        let budget = budget_usd.or_else(|| self.agent_type_budgets.get(agent_type).copied());
        let record = AgentCostRecord::new(
            agent_id.to_string(),
            agent_type.to_string(),
            model.to_string(),
            budget,
        );
        self.agents.insert(agent_id.to_string(), record);
    }

    /// Record token usage for an agent and check budget.
    ///
    /// Returns the cost check result (Ok/Warning/BudgetExceeded).
    /// Also accumulates into the session total.
    pub fn record_usage(&self, agent_id: &str, usage: &Usage, model: &str) -> CostCheckResult {
        if !self.enabled {
            return CostCheckResult::Ok;
        }

        let cache_read = usage.cache_read_input_tokens.unwrap_or(0);
        let cache_write = usage.cache_creation_input_tokens.unwrap_or(0);

        // Find pricing for this model
        let cost = self
            .find_pricing(model)
            .map_or(0.0, |p| {
                p.calculate_cost(usage.input_tokens, usage.output_tokens, cache_read, cache_write)
            });

        // Update session total
        self.session_total.add(
            usage.input_tokens,
            usage.output_tokens,
            cache_read,
            cache_write,
            cost,
        );

        // Update agent record
        if let Some(record) = self.agents.get(agent_id) {
            record.usage.add(
                usage.input_tokens,
                usage.output_tokens,
                cache_read,
                cache_write,
                cost,
            );

            // Check agent budget
            if let Some(limit) = record.budget_limit_usd {
                let current = record.usage.cost_usd();
                if current >= limit {
                    return CostCheckResult::BudgetExceeded {
                        current_usd: current,
                        limit_usd: limit,
                    };
                }
                let pct = current / limit;
                if pct >= self.warning_threshold {
                    return CostCheckResult::Warning {
                        current_usd: current,
                        limit_usd: limit,
                        percentage: pct * 100.0,
                    };
                }
            }
        }

        // Check session budget
        if let Some(limit) = self.session_budget_usd {
            let current = self.session_total.cost_usd();
            if current >= limit {
                return CostCheckResult::BudgetExceeded {
                    current_usd: current,
                    limit_usd: limit,
                };
            }
            let pct = current / limit;
            if pct >= self.warning_threshold {
                return CostCheckResult::Warning {
                    current_usd: current,
                    limit_usd: limit,
                    percentage: pct * 100.0,
                };
            }
        }

        CostCheckResult::Ok
    }

    /// Get the current cost for a specific agent.
    #[must_use]
    pub fn agent_cost(&self, agent_id: &str) -> Option<f64> {
        self.agents.get(agent_id).map(|r| r.usage.cost_usd())
    }

    /// Get the current session-wide cost.
    #[must_use]
    pub fn session_cost(&self) -> f64 {
        self.session_total.cost_usd()
    }

    /// Take a snapshot of the full cost state.
    #[must_use]
    pub fn snapshot(&self) -> CostSnapshot {
        let agents: Vec<AgentCostSummary> = self
            .agents
            .iter()
            .map(|entry| {
                let r = entry.value();
                AgentCostSummary {
                    agent_id: r.agent_id.clone(),
                    agent_type: r.agent_type.clone(),
                    model: r.model.clone(),
                    usage: r.usage.snapshot(),
                    budget_limit_usd: r.budget_limit_usd,
                }
            })
            .collect();

        CostSnapshot {
            agents,
            session: self.session_total.snapshot(),
            session_budget_usd: self.session_budget_usd,
        }
    }

    /// Find the first matching pricing entry for a model.
    fn find_pricing(&self, model: &str) -> Option<&ModelPricing> {
        self.pricing.iter().find(|p| p.matches(model))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CostConfig {
        let mut config = CostConfig::default();
        config.session_budget_usd = Some(1.0);
        config.agent_budgets.insert("explore".to_string(), 0.1);
        config
    }

    #[test]
    fn test_from_config() {
        let tracker = CostTracker::from_config(&test_config());
        assert!(tracker.is_enabled());
        assert_eq!(tracker.session_budget_usd, Some(1.0));
    }

    #[test]
    fn test_disabled_tracker() {
        let tracker = CostTracker::disabled();
        assert!(!tracker.is_enabled());

        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        let result = tracker.record_usage("agent-1", &usage, "claude-sonnet-4");
        assert_eq!(result, CostCheckResult::Ok);
    }

    #[test]
    fn test_register_and_record() {
        let tracker = CostTracker::from_config(&test_config());
        tracker.register_agent("a1", "explore", "claude-sonnet-4-5-20250929", None);

        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_input_tokens: Some(100),
            cache_creation_input_tokens: None,
        };
        let result = tracker.record_usage("a1", &usage, "claude-sonnet-4-5-20250929");

        // Cost should be recorded
        let cost = tracker.agent_cost("a1").expect("agent should exist");
        assert!(cost > 0.0);
        assert!(tracker.session_cost() > 0.0);

        // With small usage, should be Ok
        assert_eq!(result, CostCheckResult::Ok);
    }

    #[test]
    fn test_agent_budget_exceeded() {
        let tracker = CostTracker::from_config(&test_config());
        // Budget for explore = $0.10
        tracker.register_agent("a1", "explore", "claude-sonnet-4-5-20250929", None);

        // Record a lot of usage to exceed $0.10
        for _ in 0..20 {
            let usage = Usage {
                input_tokens: 100_000,
                output_tokens: 50_000,
                ..Default::default()
            };
            tracker.record_usage("a1", &usage, "claude-sonnet-4-5-20250929");
        }

        let cost = tracker.agent_cost("a1").expect("agent should exist");
        assert!(cost > 0.1, "Cost {cost} should exceed budget $0.10");

        // Next record should return BudgetExceeded
        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        let result = tracker.record_usage("a1", &usage, "claude-sonnet-4-5-20250929");
        assert!(matches!(result, CostCheckResult::BudgetExceeded { .. }));
    }

    #[test]
    fn test_session_budget_exceeded() {
        let mut config = CostConfig::default();
        config.session_budget_usd = Some(0.01); // Very small budget
        let tracker = CostTracker::from_config(&config);
        tracker.register_agent("a1", "general-purpose", "claude-sonnet-4-5-20250929", None);

        // Record enough to exceed $0.01
        for _ in 0..10 {
            let usage = Usage {
                input_tokens: 100_000,
                output_tokens: 50_000,
                ..Default::default()
            };
            tracker.record_usage("a1", &usage, "claude-sonnet-4-5-20250929");
        }

        let result = tracker.record_usage(
            "a1",
            &Usage { input_tokens: 1000, output_tokens: 500, ..Default::default() },
            "claude-sonnet-4-5-20250929",
        );
        assert!(matches!(
            result,
            CostCheckResult::BudgetExceeded { .. } | CostCheckResult::Warning { .. }
        ));
    }

    #[test]
    fn test_snapshot() {
        let tracker = CostTracker::from_config(&test_config());
        tracker.register_agent("a1", "explore", "claude-sonnet-4", None);
        tracker.register_agent("a2", "plan", "claude-sonnet-4", None);

        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        tracker.record_usage("a1", &usage, "claude-sonnet-4");

        let snap = tracker.snapshot();
        assert_eq!(snap.agents.len(), 2);
        assert_eq!(snap.session_budget_usd, Some(1.0));
        assert!(snap.session.cost_usd > 0.0);
    }

    #[test]
    fn test_custom_budget_override() {
        let tracker = CostTracker::from_config(&test_config());
        // Override default budget
        tracker.register_agent("a1", "explore", "claude-sonnet-4", Some(5.0));

        let record = tracker.agents.get("a1").expect("agent should exist");
        assert_eq!(record.budget_limit_usd, Some(5.0));
    }

    #[test]
    fn test_unknown_model_pricing() {
        let tracker = CostTracker::from_config(&test_config());
        tracker.register_agent("a1", "explore", "unknown-model-xyz", None);

        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        let result = tracker.record_usage("a1", &usage, "unknown-model-xyz");
        // Should still work, just with 0 cost
        assert_eq!(result, CostCheckResult::Ok);
        assert_eq!(tracker.agent_cost("a1"), Some(0.0));
    }

    #[test]
    fn test_unregistered_agent_usage() {
        let tracker = CostTracker::from_config(&test_config());
        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        // Recording for an unregistered agent should still update session total
        let result = tracker.record_usage("nonexistent", &usage, "claude-sonnet-4-5-20250929");
        assert_eq!(result, CostCheckResult::Ok);
        assert!(tracker.session_cost() > 0.0);
    }
}
