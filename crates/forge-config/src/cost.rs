//! Cost tracking configuration.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

const fn default_enabled() -> bool {
    true
}

const fn default_warning_threshold() -> f64 {
    0.8
}

// ---------------------------------------------------------------------------
// Top-level cost config
// ---------------------------------------------------------------------------

/// Cost tracking configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostConfig {
    /// Whether cost tracking is enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Overall session budget limit in USD. `None` means no limit.
    #[serde(default)]
    pub session_budget_usd: Option<f64>,

    /// Warning threshold as a fraction (0.0–1.0). Default: 0.8 (80%).
    #[serde(default = "default_warning_threshold")]
    pub warning_threshold: f64,

    /// Custom model pricing overrides.
    ///
    /// When empty, built-in pricing is used.
    #[serde(default)]
    pub pricing: Vec<PricingEntry>,

    /// Per-agent-type budget limits in USD.
    #[serde(default)]
    pub agent_budgets: HashMap<String, f64>,
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            session_budget_usd: None,
            warning_threshold: default_warning_threshold(),
            pricing: Vec::new(),
            agent_budgets: HashMap::new(),
        }
    }
}

impl CostConfig {
    /// Get the budget limit for a specific agent type, if configured.
    #[must_use]
    pub fn agent_budget(&self, agent_type: &str) -> Option<f64> {
        self.agent_budgets.get(agent_type).copied()
    }
}

// ---------------------------------------------------------------------------
// Pricing entry
// ---------------------------------------------------------------------------

/// Custom pricing for a model or model pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingEntry {
    /// Model name or glob pattern (e.g. `"claude-sonnet-*"`).
    pub model_pattern: String,
    /// Cost per million input tokens (USD).
    pub input_cost_per_mtok: f64,
    /// Cost per million output tokens (USD).
    pub output_cost_per_mtok: f64,
    /// Cost per million cache-read tokens (USD).
    #[serde(default)]
    pub cache_read_per_mtok: Option<f64>,
    /// Cost per million cache-write tokens (USD).
    #[serde(default)]
    pub cache_write_per_mtok: Option<f64>,
}

impl PricingEntry {
    /// Convert to the domain `ModelPricing` type.
    #[must_use]
    pub fn to_model_pricing(&self) -> forge_domain::ModelPricing {
        forge_domain::ModelPricing {
            model_pattern: self.model_pattern.clone(),
            input_cost_per_mtok: self.input_cost_per_mtok,
            output_cost_per_mtok: self.output_cost_per_mtok,
            cache_read_per_mtok: self.cache_read_per_mtok,
            cache_write_per_mtok: self.cache_write_per_mtok,
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in pricing table
// ---------------------------------------------------------------------------

/// Built-in pricing table for commonly used models.
///
/// Users can override these via `[cost.pricing]` in their config.
#[must_use]
pub fn builtin_pricing() -> Vec<forge_domain::ModelPricing> {
    vec![
        // Anthropic Claude
        forge_domain::ModelPricing {
            model_pattern: "claude-opus-*".to_string(),
            input_cost_per_mtok: 15.0,
            output_cost_per_mtok: 75.0,
            cache_read_per_mtok: Some(1.5),
            cache_write_per_mtok: Some(18.75),
        },
        forge_domain::ModelPricing {
            model_pattern: "claude-sonnet-*".to_string(),
            input_cost_per_mtok: 3.0,
            output_cost_per_mtok: 15.0,
            cache_read_per_mtok: Some(0.3),
            cache_write_per_mtok: Some(3.75),
        },
        forge_domain::ModelPricing {
            model_pattern: "claude-haiku-*".to_string(),
            input_cost_per_mtok: 0.8,
            output_cost_per_mtok: 4.0,
            cache_read_per_mtok: Some(0.08),
            cache_write_per_mtok: Some(1.0),
        },
        // Legacy Claude model names
        forge_domain::ModelPricing {
            model_pattern: "claude-3-5-sonnet-*".to_string(),
            input_cost_per_mtok: 3.0,
            output_cost_per_mtok: 15.0,
            cache_read_per_mtok: Some(0.3),
            cache_write_per_mtok: Some(3.75),
        },
        forge_domain::ModelPricing {
            model_pattern: "claude-3-5-haiku-*".to_string(),
            input_cost_per_mtok: 0.8,
            output_cost_per_mtok: 4.0,
            cache_read_per_mtok: Some(0.08),
            cache_write_per_mtok: Some(1.0),
        },
        // OpenAI GPT-4o-mini (must be before gpt-4o* to match more-specific pattern first)
        forge_domain::ModelPricing {
            model_pattern: "gpt-4o-mini*".to_string(),
            input_cost_per_mtok: 0.15,
            output_cost_per_mtok: 0.6,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
        },
        // OpenAI GPT-4o
        forge_domain::ModelPricing {
            model_pattern: "gpt-4o*".to_string(),
            input_cost_per_mtok: 2.5,
            output_cost_per_mtok: 10.0,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
        },
        // OpenAI o1-mini (must be before o1* to match more-specific pattern first)
        forge_domain::ModelPricing {
            model_pattern: "o1-mini*".to_string(),
            input_cost_per_mtok: 3.0,
            output_cost_per_mtok: 12.0,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
        },
        // OpenAI o1
        forge_domain::ModelPricing {
            model_pattern: "o1*".to_string(),
            input_cost_per_mtok: 15.0,
            output_cost_per_mtok: 60.0,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
        },
        // OpenAI o3-mini (must be before o3* to match more-specific pattern first)
        forge_domain::ModelPricing {
            model_pattern: "o3-mini*".to_string(),
            input_cost_per_mtok: 1.1,
            output_cost_per_mtok: 4.4,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
        },
        // OpenAI o3
        forge_domain::ModelPricing {
            model_pattern: "o3*".to_string(),
            input_cost_per_mtok: 10.0,
            output_cost_per_mtok: 40.0,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_config_default() {
        let config = CostConfig::default();
        assert!(config.enabled);
        assert!(config.session_budget_usd.is_none());
        assert!((config.warning_threshold - 0.8).abs() < f64::EPSILON);
        assert!(config.pricing.is_empty());
        assert!(config.agent_budgets.is_empty());
    }

    #[test]
    fn test_cost_config_serde_roundtrip() {
        let mut config = CostConfig::default();
        config.session_budget_usd = Some(10.0);
        config.agent_budgets.insert("explore".to_string(), 0.5);
        config.agent_budgets.insert("general-purpose".to_string(), 2.0);
        config.pricing.push(PricingEntry {
            model_pattern: "my-model-*".to_string(),
            input_cost_per_mtok: 1.0,
            output_cost_per_mtok: 5.0,
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
        });

        let toml_str = toml::to_string(&config).expect("serialize");
        let parsed: CostConfig = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(parsed.session_budget_usd, Some(10.0));
        assert_eq!(parsed.agent_budget("explore"), Some(0.5));
        assert_eq!(parsed.agent_budget("general-purpose"), Some(2.0));
        assert_eq!(parsed.agent_budget("plan"), None);
        assert_eq!(parsed.pricing.len(), 1);
    }

    #[test]
    fn test_pricing_entry_to_model_pricing() {
        let entry = PricingEntry {
            model_pattern: "test-*".to_string(),
            input_cost_per_mtok: 2.0,
            output_cost_per_mtok: 10.0,
            cache_read_per_mtok: Some(0.2),
            cache_write_per_mtok: None,
        };
        let mp = entry.to_model_pricing();
        assert_eq!(mp.model_pattern, "test-*");
        assert!((mp.input_cost_per_mtok - 2.0).abs() < f64::EPSILON);
        assert_eq!(mp.cache_read_per_mtok, Some(0.2));
    }

    #[test]
    fn test_builtin_pricing_covers_common_models() {
        let pricing = builtin_pricing();
        // Should have entries for Claude and OpenAI families
        assert!(pricing.iter().any(|p| p.matches("claude-sonnet-4-5-20250929")));
        assert!(pricing.iter().any(|p| p.matches("claude-opus-4")));
        assert!(pricing.iter().any(|p| p.matches("claude-haiku-3-5")));
        assert!(pricing.iter().any(|p| p.matches("gpt-4o-2024")));
        assert!(pricing.iter().any(|p| p.matches("o1-preview")));
    }

    #[test]
    fn test_cost_config_toml_from_string() {
        let toml_str = r#"
            enabled = true
            session_budget_usd = 5.0
            warning_threshold = 0.75

            [[pricing]]
            model_pattern = "custom-model"
            input_cost_per_mtok = 1.0
            output_cost_per_mtok = 5.0

            [agent_budgets]
            explore = 0.50
            plan = 1.00
            "general-purpose" = 2.00
        "#;
        let config: CostConfig = toml::from_str(toml_str).expect("parse");
        assert!(config.enabled);
        assert_eq!(config.session_budget_usd, Some(5.0));
        assert!((config.warning_threshold - 0.75).abs() < f64::EPSILON);
        assert_eq!(config.pricing.len(), 1);
        assert_eq!(config.agent_budget("explore"), Some(0.5));
        assert_eq!(config.agent_budget("plan"), Some(1.0));
        assert_eq!(config.agent_budget("general-purpose"), Some(2.0));
    }

    #[test]
    fn test_builtin_pricing_gpt4o_mini_before_gpt4o() {
        let pricing = builtin_pricing();
        // gpt-4o-mini must match its own specific entry, not the gpt-4o* catch-all
        let mini_idx = pricing.iter().position(|p| p.matches("gpt-4o-mini-2024-07-18")).unwrap();
        let gpt4o_idx = pricing.iter().position(|p| p.model_pattern == "gpt-4o*").unwrap();
        assert!(
            mini_idx < gpt4o_idx,
            "gpt-4o-mini entry must appear before gpt-4o to avoid incorrect matching"
        );
        // Verify pricing is correct for gpt-4o-mini
        assert!((pricing[mini_idx].input_cost_per_mtok - 0.15).abs() < f64::EPSILON);
    }
}
