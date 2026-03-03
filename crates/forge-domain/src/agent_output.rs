//! Typed output envelopes for sub-agent results.
//!
//! Defines the [`AgentOutput`] trait that all structured sub-agent outputs
//! must implement, along with per-agent-type output structs and the
//! generic [`AgentEnvelope`] wrapper.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Core trait
// ---------------------------------------------------------------------------

/// Marker trait for structured sub-agent outputs.
///
/// Every concrete output type must be serializable and carry a schema
/// version so consumers can handle version evolution gracefully.
pub trait AgentOutput: Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static {
    /// Schema version for this output type. Increment when fields change.
    const SCHEMA_VERSION: u16;

    /// Return the JSON Schema for this output type.
    ///
    /// Used to inject output format instructions into sub-agent system prompts
    /// and for runtime validation of LLM-generated JSON.
    fn json_schema() -> Value;

    /// Validate the contents after deserialization.
    ///
    /// Override this for domain-specific invariants beyond what the schema
    /// captures (e.g. "at least one finding must be present").
    ///
    /// # Errors
    ///
    /// Returns a human-readable error string if validation fails.
    fn validate(&self) -> std::result::Result<(), String> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// Generic envelope wrapping a typed sub-agent output.
///
/// This is the wire format returned by sub-agents. The `schema_version`
/// field allows consumers to detect version mismatches and handle them
/// gracefully (e.g. falling back to plain text).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(
    serialize = "T: Serialize",
    deserialize = "T: for<'a> Deserialize<'a>"
))]
pub struct AgentEnvelope<T: AgentOutput> {
    /// Schema version of the payload.
    pub schema_version: u16,
    /// Agent type that produced this output (e.g. "explore", "plan").
    pub agent_type: String,
    /// The structured payload.
    pub payload: T,
    /// Execution metadata.
    pub metadata: EnvelopeMetadata,
}

/// Execution metadata attached to every envelope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvelopeMetadata {
    /// Total tokens consumed.
    pub tokens_used: usize,
    /// Number of tool calls made.
    pub tool_calls: usize,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
    /// Model used for the sub-agent.
    pub model: String,
    /// Estimated cost in USD (if cost tracking is enabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

// ---------------------------------------------------------------------------
// Explore output
// ---------------------------------------------------------------------------

/// Structured output from the Explore sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExploreOutput {
    /// Files found matching the exploration query.
    pub files_found: Vec<String>,
    /// High-level structure summary.
    pub structure_summary: String,
    /// Key patterns or conventions observed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_patterns: Vec<String>,
}

impl AgentOutput for ExploreOutput {
    const SCHEMA_VERSION: u16 = 1;

    fn json_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["files_found", "structure_summary"],
            "properties": {
                "files_found": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File paths found during exploration"
                },
                "structure_summary": {
                    "type": "string",
                    "description": "High-level summary of the codebase structure"
                },
                "key_patterns": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Key patterns or conventions observed"
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Plan output
// ---------------------------------------------------------------------------

/// A single step in a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step number (1-indexed).
    pub step: usize,
    /// Description of this step.
    pub description: String,
    /// Files likely to be touched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Estimated complexity ("low", "medium", "high").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
}

/// Structured output from the Plan sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanOutput {
    /// Ordered list of implementation steps.
    pub steps: Vec<PlanStep>,
    /// Identified risks or challenges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risks: Vec<String>,
    /// Overall estimated complexity ("low", "medium", "high").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_complexity: Option<String>,
}

impl AgentOutput for PlanOutput {
    const SCHEMA_VERSION: u16 = 1;

    fn json_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["steps"],
            "properties": {
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["step", "description"],
                        "properties": {
                            "step": { "type": "integer" },
                            "description": { "type": "string" },
                            "files": { "type": "array", "items": { "type": "string" } },
                            "complexity": { "type": "string" }
                        }
                    },
                    "description": "Ordered implementation steps"
                },
                "risks": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "estimated_complexity": { "type": "string" }
            }
        })
    }

    fn validate(&self) -> std::result::Result<(), String> {
        if self.steps.is_empty() {
            return Err("Plan must have at least one step".to_string());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Research output
// ---------------------------------------------------------------------------

/// A single research finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Title or topic of the finding.
    pub title: String,
    /// Detailed description.
    pub content: String,
    /// Confidence level ("high", "medium", "low").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

/// Structured output from the Research sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResearchOutput {
    /// Research findings.
    pub findings: Vec<Finding>,
    /// Sources consulted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
}

impl AgentOutput for ResearchOutput {
    const SCHEMA_VERSION: u16 = 1;

    fn json_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["findings"],
            "properties": {
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["title", "content"],
                        "properties": {
                            "title": { "type": "string" },
                            "content": { "type": "string" },
                            "confidence": { "type": "string" }
                        }
                    }
                },
                "sources": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Analysis output (DataAnalyst)
// ---------------------------------------------------------------------------

/// Structured output from the DataAnalyst sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisOutput {
    /// Key conclusions from the analysis.
    pub conclusions: Vec<String>,
    /// Data references (file paths, URLs, etc.).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data_references: Vec<String>,
    /// Actionable recommendations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recommendations: Vec<String>,
}

impl AgentOutput for AnalysisOutput {
    const SCHEMA_VERSION: u16 = 1;

    fn json_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["conclusions"],
            "properties": {
                "conclusions": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "data_references": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "recommendations": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        })
    }

    fn validate(&self) -> std::result::Result<(), String> {
        if self.conclusions.is_empty() {
            return Err("Analysis must have at least one conclusion".to_string());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// GeneralPurpose output
// ---------------------------------------------------------------------------

/// Structured output from the GeneralPurpose sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralOutput {
    /// Summary of what was accomplished.
    pub summary: String,
    /// Files that were modified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files_modified: Vec<String>,
    /// Actions taken during execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions_taken: Vec<String>,
}

impl AgentOutput for GeneralOutput {
    const SCHEMA_VERSION: u16 = 1;

    fn json_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["summary"],
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Summary of what was accomplished"
                },
                "files_modified": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "actions_taken": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Writer output
// ---------------------------------------------------------------------------

/// Structured output from the Writer sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriterOutput {
    /// The written content.
    pub content: String,
    /// Content format (e.g. "markdown", "plain", "html").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Approximate word count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_count: Option<usize>,
}

impl AgentOutput for WriterOutput {
    const SCHEMA_VERSION: u16 = 1;

    fn json_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "required": ["content"],
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The written content"
                },
                "format": {
                    "type": "string",
                    "description": "Content format (markdown, plain, html)"
                },
                "word_count": {
                    "type": "integer",
                    "description": "Approximate word count"
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Helper: try-parse from text
// ---------------------------------------------------------------------------

/// Attempt to parse structured output from a sub-agent's text response.
///
/// Looks for a JSON code block (```json ... ```) or raw JSON object in the
/// text. Returns `None` if no valid JSON is found or if deserialization fails.
pub fn try_parse_output<T: AgentOutput>(text: &str) -> Option<T> {
    // Try extracting from markdown code fence first
    if let Some(json_str) = extract_json_block(text) {
        if let Ok(parsed) = serde_json::from_str::<T>(json_str) {
            if parsed.validate().is_ok() {
                return Some(parsed);
            }
        }
    }

    // Try parsing the entire text as JSON
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<T>(trimmed) {
            if parsed.validate().is_ok() {
                return Some(parsed);
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Version-compatible consumption
// ---------------------------------------------------------------------------

/// Result of attempting to consume structured output with version checking.
#[derive(Debug)]
pub enum ConsumeResult<T> {
    /// Successfully parsed and version matches.
    Ok(T),
    /// Parsed successfully but schema version is newer than expected.
    /// The consumer should decide whether to use the data or fall back.
    VersionMismatch { parsed: T, expected: u16, actual: u16 },
    /// No structured data available.
    NoData,
    /// Structured data present but failed to deserialize into `T`.
    ParseError(String),
}

/// Consume structured output from a [`ToolOutput`](crate::ToolOutput) with version checking.
///
/// This is the primary API for orchestrator-side type-safe consumption of
/// sub-agent results. It checks the `schema_version` against the expected
/// version from `T::SCHEMA_VERSION` and returns a [`ConsumeResult`].
///
/// # Examples
///
/// ```ignore
/// let result = consume_structured::<ExploreOutput>(&tool_output.data, tool_output.schema_version);
/// match result {
///     ConsumeResult::Ok(explore) => { /* use explore.files_found */ }
///     ConsumeResult::VersionMismatch { parsed, .. } => { /* use with caution */ }
///     ConsumeResult::NoData => { /* fall back to text */ }
///     ConsumeResult::ParseError(e) => { /* log and fall back */ }
/// }
/// ```
pub fn consume_structured<T: AgentOutput>(
    data: &Option<Value>,
    schema_version: Option<u16>,
) -> ConsumeResult<T> {
    let value = match data {
        Some(v) => v,
        None => return ConsumeResult::NoData,
    };

    match serde_json::from_value::<T>(value.clone()) {
        Ok(parsed) => {
            if let Err(e) = parsed.validate() {
                return ConsumeResult::ParseError(format!("validation failed: {e}"));
            }
            match schema_version {
                // No version info (legacy data) — assume compatible
                None => ConsumeResult::Ok(parsed),
                // Version present — check for exact match
                Some(actual) if actual != T::SCHEMA_VERSION => ConsumeResult::VersionMismatch {
                    parsed,
                    expected: T::SCHEMA_VERSION,
                    actual,
                },
                Some(_) => ConsumeResult::Ok(parsed),
            }
        }
        Err(e) => ConsumeResult::ParseError(e.to_string()),
    }
}

/// Extract the content of the first ```json ... ``` code block.
fn extract_json_block(text: &str) -> Option<&str> {
    let start_marker = "```json";
    let end_marker = "```";

    let start = text.find(start_marker)?;
    let content_start = start + start_marker.len();
    let rest = &text[content_start..];
    let end = rest.find(end_marker)?;
    let content = rest[..end].trim();
    if content.is_empty() { None } else { Some(content) }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_explore_output_serde() {
        let output = ExploreOutput {
            files_found: vec!["src/main.rs".to_string(), "src/lib.rs".to_string()],
            structure_summary: "A Rust workspace with 3 crates".to_string(),
            key_patterns: vec!["Uses async/await".to_string()],
        };
        let json = serde_json::to_string(&output).expect("serialize");
        let parsed: ExploreOutput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.files_found.len(), 2);
        assert_eq!(parsed.structure_summary, output.structure_summary);
    }

    #[test]
    fn test_plan_output_validate() {
        let empty_plan = PlanOutput {
            steps: vec![],
            risks: vec![],
            estimated_complexity: None,
        };
        assert!(empty_plan.validate().is_err());

        let valid_plan = PlanOutput {
            steps: vec![PlanStep {
                step: 1,
                description: "Add feature".to_string(),
                files: vec![],
                complexity: None,
            }],
            risks: vec![],
            estimated_complexity: Some("low".to_string()),
        };
        assert!(valid_plan.validate().is_ok());
    }

    #[test]
    fn test_analysis_output_validate() {
        let empty = AnalysisOutput {
            conclusions: vec![],
            data_references: vec![],
            recommendations: vec![],
        };
        assert!(empty.validate().is_err());

        let valid = AnalysisOutput {
            conclusions: vec!["Performance is within bounds".to_string()],
            data_references: vec![],
            recommendations: vec![],
        };
        assert!(valid.validate().is_ok());
    }

    #[test]
    fn test_envelope_serde() {
        let envelope = AgentEnvelope {
            schema_version: ExploreOutput::SCHEMA_VERSION,
            agent_type: "explore".to_string(),
            payload: ExploreOutput {
                files_found: vec!["a.rs".to_string()],
                structure_summary: "Small project".to_string(),
                key_patterns: vec![],
            },
            metadata: EnvelopeMetadata {
                tokens_used: 500,
                tool_calls: 3,
                duration_ms: 1200,
                model: "claude-sonnet-4".to_string(),
                cost_usd: Some(0.003),
            },
        };

        let json = serde_json::to_string_pretty(&envelope).expect("serialize");
        let parsed: AgentEnvelope<ExploreOutput> =
            serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.agent_type, "explore");
        assert_eq!(parsed.payload.files_found.len(), 1);
        assert_eq!(parsed.metadata.tokens_used, 500);
    }

    #[test]
    fn test_json_schema_has_required_fields() {
        let schema = ExploreOutput::json_schema();
        let required = schema["required"].as_array().expect("required array");
        assert!(required.iter().any(|v| v == "files_found"));
        assert!(required.iter().any(|v| v == "structure_summary"));
    }

    #[test]
    fn test_try_parse_from_code_block() {
        let text = r#"Here is my analysis:

```json
{
    "files_found": ["src/main.rs"],
    "structure_summary": "A simple project"
}
```

That's the result."#;

        let output: Option<ExploreOutput> = try_parse_output(text);
        assert!(output.is_some());
        let output = output.expect("parsed");
        assert_eq!(output.files_found, vec!["src/main.rs"]);
    }

    #[test]
    fn test_try_parse_from_raw_json() {
        let text = r#"{"files_found": ["a.rs"], "structure_summary": "test"}"#;
        let output: Option<ExploreOutput> = try_parse_output(text);
        assert!(output.is_some());
    }

    #[test]
    fn test_try_parse_invalid_json() {
        let text = "This is just plain text without any JSON.";
        let output: Option<ExploreOutput> = try_parse_output(text);
        assert!(output.is_none());
    }

    #[test]
    fn test_try_parse_validation_failure() {
        // Empty steps should fail validation for PlanOutput
        let text = r#"{"steps": []}"#;
        let output: Option<PlanOutput> = try_parse_output(text);
        assert!(output.is_none());
    }

    #[test]
    fn test_writer_output() {
        let output = WriterOutput {
            content: "Hello world".to_string(),
            format: Some("markdown".to_string()),
            word_count: Some(2),
        };
        let json = serde_json::to_string(&output).expect("serialize");
        let parsed: WriterOutput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.word_count, Some(2));
    }

    #[test]
    fn test_general_output() {
        let output = GeneralOutput {
            summary: "Fixed the bug".to_string(),
            files_modified: vec!["src/bug.rs".to_string()],
            actions_taken: vec!["Edited file".to_string(), "Ran tests".to_string()],
        };
        let json = serde_json::to_string(&output).expect("serialize");
        let parsed: GeneralOutput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.actions_taken.len(), 2);
    }

    #[test]
    fn test_research_output() {
        let output = ResearchOutput {
            findings: vec![Finding {
                title: "API design".to_string(),
                content: "Uses REST with JSON".to_string(),
                confidence: Some("high".to_string()),
            }],
            sources: vec!["https://docs.example.com".to_string()],
        };
        let json = serde_json::to_string(&output).expect("serialize");
        let parsed: ResearchOutput = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.findings.len(), 1);
        assert_eq!(parsed.sources.len(), 1);
    }

    #[test]
    fn test_extract_json_block() {
        assert_eq!(
            extract_json_block("text\n```json\n{\"a\": 1}\n```\nmore"),
            Some("{\"a\": 1}")
        );
        assert_eq!(extract_json_block("no code block"), None);
        assert_eq!(extract_json_block("```json\n```"), None); // empty block
    }

    #[test]
    fn test_consume_structured_ok() {
        let data = Some(serde_json::json!({
            "files_found": ["src/main.rs"],
            "structure_summary": "A project"
        }));
        let result = consume_structured::<ExploreOutput>(&data, Some(1));
        assert!(matches!(result, ConsumeResult::Ok(_)));
        if let ConsumeResult::Ok(output) = result {
            assert_eq!(output.files_found, vec!["src/main.rs"]);
        }
    }

    #[test]
    fn test_consume_structured_no_data() {
        let result = consume_structured::<ExploreOutput>(&None, None);
        assert!(matches!(result, ConsumeResult::NoData));
    }

    #[test]
    fn test_consume_structured_version_mismatch() {
        let data = Some(serde_json::json!({
            "files_found": ["a.rs"],
            "structure_summary": "test"
        }));
        // Simulate a newer schema version (999) than ExploreOutput::SCHEMA_VERSION (1)
        let result = consume_structured::<ExploreOutput>(&data, Some(999));
        assert!(matches!(result, ConsumeResult::VersionMismatch { .. }));
        if let ConsumeResult::VersionMismatch { expected, actual, .. } = result {
            assert_eq!(expected, 1);
            assert_eq!(actual, 999);
        }
    }

    #[test]
    fn test_consume_structured_parse_error() {
        let data = Some(serde_json::json!({"invalid_field": 42}));
        let result = consume_structured::<ExploreOutput>(&data, Some(1));
        assert!(matches!(result, ConsumeResult::ParseError(_)));
    }

    #[test]
    fn test_consume_structured_validation_failure() {
        // PlanOutput requires non-empty steps
        let data = Some(serde_json::json!({"steps": []}));
        let result = consume_structured::<PlanOutput>(&data, Some(1));
        assert!(matches!(result, ConsumeResult::ParseError(_)));
    }

    #[test]
    fn test_consume_structured_no_version_treated_as_zero() {
        let data = Some(serde_json::json!({
            "files_found": ["a.rs"],
            "structure_summary": "test"
        }));
        // schema_version=None → legacy data, treated as compatible → Ok
        let result = consume_structured::<ExploreOutput>(&data, None);
        assert!(matches!(result, ConsumeResult::Ok(_)));
    }

    #[test]
    fn test_consume_structured_older_version_mismatch() {
        let data = Some(serde_json::json!({
            "files_found": ["a.rs"],
            "structure_summary": "test"
        }));
        // Explicit old version 0 triggers mismatch
        let result = consume_structured::<ExploreOutput>(&data, Some(0));
        assert!(matches!(result, ConsumeResult::VersionMismatch { .. }));
        if let ConsumeResult::VersionMismatch { expected, actual, .. } = result {
            assert_eq!(expected, 1);
            assert_eq!(actual, 0);
        }
    }
}
