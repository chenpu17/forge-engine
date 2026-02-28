//! Verifier pipeline for generation-validation separation.

use crate::{AgentError, Result, VerifierConfig, VerifierMode, VerifierPolicy};
use forge_domain::{AgentEvent, ToolCall, ToolResult};
use tokio::sync::mpsc;

/// Outcome of a verifier evaluation on a tool call result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifierDecision {
    /// The tool call result passed all checks.
    Pass,
    /// The result raised non-blocking warnings.
    Warn {
        /// Human-readable warning message.
        message: String,
    },
    /// The result was blocked by the verifier.
    Fail {
        /// Human-readable reason for the block.
        reason: String,
    },
}

/// Deterministic verification pipeline for tool call results.
#[derive(Debug, Clone)]
pub struct VerifierPipeline {
    config: VerifierConfig,
    enabled: bool,
}

impl VerifierPipeline {
    /// Create a new pipeline with the given configuration and enabled flag.
    #[must_use]
    pub const fn new(config: VerifierConfig, enabled: bool) -> Self {
        Self { config, enabled }
    }

    /// Evaluate a tool call result against the configured verification rules.
    #[must_use]
    pub fn evaluate(
        &self,
        call: &ToolCall,
        result: &ToolResult,
        is_readonly: bool,
    ) -> VerifierDecision {
        if !self.enabled || !self.config.enabled {
            return VerifierDecision::Pass;
        }

        let mut issues = Vec::new();

        // Deterministic checks (always on in both modes)
        if !is_readonly && result.is_error {
            issues.push(format!("write-capable tool {} returned error", call.name));
        }

        let output_lower = result.output.to_lowercase();
        if !is_readonly
            && !result.is_error
            && (output_lower.contains("permission denied")
                || output_lower.contains("operation not permitted")
                || output_lower.contains("segmentation fault")
                || output_lower.contains("panic")
                || output_lower.contains("fatal:"))
        {
            issues.push(format!(
                "suspicious output from write-capable tool {}: {}",
                call.name, result.output
            ));
        }

        if matches!(self.config.mode, VerifierMode::Hybrid)
            && !result.is_error
            && !is_readonly
            && output_lower.contains("warning")
        {
            issues.push(format!(
                "hybrid verifier warning for tool {}: output contains warning markers",
                call.name
            ));
        }

        if issues.is_empty() {
            return VerifierDecision::Pass;
        }

        match self.config.policy {
            VerifierPolicy::WarnOnly => VerifierDecision::Warn {
                message: format!("Verifier warnings: {}", issues.join("; ")),
            },
            VerifierPolicy::FailClosed => VerifierDecision::Fail {
                reason: format!("Verifier blocked: {}", issues.join("; ")),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Verifier stats & apply helper (extracted from core_loop.rs)
// ---------------------------------------------------------------------------

/// Tracks verifier evaluation statistics across the agent loop.
#[derive(Default)]
pub(crate) struct VerifierStats {
    pub evaluated: usize,
    pub warnings: usize,
    pub blocked: usize,
}

/// Evaluate a single tool call result through the verifier pipeline and emit events.
pub(crate) async fn apply_verifier_decision(
    verifier: &VerifierPipeline,
    call: &ToolCall,
    result: &ToolResult,
    is_readonly: bool,
    tx: &mpsc::Sender<Result<AgentEvent>>,
    stats: &mut VerifierStats,
) -> Result<()> {
    stats.evaluated += 1;
    let decision = verifier.evaluate(call, result, is_readonly);
    match decision {
        VerifierDecision::Pass => return Ok(()),
        VerifierDecision::Warn { message } => {
            stats.warnings += 1;
            let _ = tx
                .send(Ok(AgentEvent::Recovery {
                    action: "Verifier warning".to_string(),
                    suggestion: Some(message),
                }))
                .await;
        }
        VerifierDecision::Fail { reason } => {
            stats.blocked += 1;
            let _ = tx.send(Ok(AgentEvent::Error { message: reason.clone() })).await;
            return Err(AgentError::PlanningError(reason));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str) -> ToolCall {
        ToolCall { id: "t1".to_string(), name: name.to_string(), input: json!({}) }
    }

    #[test]
    fn test_verifier_disabled_passes() {
        let pipeline = VerifierPipeline::new(VerifierConfig::default(), false);
        let decision = pipeline.evaluate(&call("write"), &ToolResult::error("t1", "boom"), false);
        assert_eq!(decision, VerifierDecision::Pass);
    }

    #[test]
    fn test_verifier_warn_only_warns() {
        let mut cfg = VerifierConfig::default();
        cfg.policy = VerifierPolicy::WarnOnly;
        let pipeline = VerifierPipeline::new(cfg, true);
        let decision = pipeline.evaluate(&call("write"), &ToolResult::error("t1", "boom"), false);
        assert!(matches!(decision, VerifierDecision::Warn { .. }));
    }

    #[test]
    fn test_verifier_fail_closed_blocks() {
        let mut cfg = VerifierConfig::default();
        cfg.policy = VerifierPolicy::FailClosed;
        let pipeline = VerifierPipeline::new(cfg, true);
        let decision = pipeline.evaluate(&call("write"), &ToolResult::error("t1", "boom"), false);
        assert!(matches!(decision, VerifierDecision::Fail { .. }));
    }
}
