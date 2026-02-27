//! `AskUserQuestion` tool - Structured user prompting
//!
//! This tool allows the AI to ask the user structured questions with
//! predefined options, improving the interaction experience.

use crate::description::ToolDescriptions;
use crate::{ToolError, ToolExecutionContext, ToolOutput};
use async_trait::async_trait;
use forge_domain::Tool;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::sync::OnceLock;

/// A single option for a question
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    /// The display text for this option
    pub label: String,
    /// Explanation of what this option means
    pub description: String,
}

/// A single question to ask the user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    /// The complete question to ask
    pub question: String,
    /// Short label displayed as a chip/tag (max 12 chars)
    pub header: String,
    /// Available choices (2-4 options)
    pub options: Vec<QuestionOption>,
    /// Whether multiple options can be selected
    #[serde(rename = "multiSelect", default)]
    pub multi_select: bool,
}

/// User's answer to a question
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionAnswer {
    /// The question header
    pub header: String,
    /// Selected option label(s)
    pub selected: Vec<String>,
    /// Custom text if "Other" was selected
    pub custom_text: Option<String>,
}

/// Fallback description when external markdown is not available
const FALLBACK_DESCRIPTION: &str = r"Ask the user structured questions to gather preferences, clarify requirements, or get decisions on implementation choices. Each question has 2-4 predefined options. Users can always select 'Other' to provide custom input. Use multiSelect when choices are not mutually exclusive.";

/// `AskUserQuestion` tool for structured prompting
pub struct AskUserQuestionTool;

impl AskUserQuestionTool {
    /// Create a new `AskUserQuestion` tool
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for AskUserQuestionTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AskUserQuestionTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        static DESC: OnceLock<String> = OnceLock::new();
        DESC.get_or_init(|| ToolDescriptions::get("ask_user", FALLBACK_DESCRIPTION))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "Questions to ask the user (1-4 questions)",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The complete question to ask the user. Should be clear and end with a question mark."
                            },
                            "header": {
                                "type": "string",
                                "description": "Very short label displayed as a chip/tag (max 12 chars). Examples: 'Auth method', 'Library', 'Approach'."
                            },
                            "options": {
                                "type": "array",
                                "description": "Available choices (2-4 options). Each should be distinct. 'Other' is automatically added.",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "Display text for this option (1-5 words)"
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "Explanation of what this option means or implies"
                                        }
                                    },
                                    "required": ["label", "description"]
                                }
                            },
                            "multiSelect": {
                                "type": "boolean",
                                "description": "Set to true to allow multiple selections. Default: false"
                            }
                        },
                        "required": ["question", "header", "options", "multiSelect"]
                    }
                },
                "answers": {
                    "type": "object",
                    "description": "User answers collected by the UI (populated by the system)",
                    "additionalProperties": {
                        "type": "string"
                    }
                }
            },
            "required": ["questions"]
        })
    }

    async fn execute(
        &self,
        params: Value,
        _ctx: &dyn ToolExecutionContext,
    ) -> std::result::Result<ToolOutput, ToolError> {
        // Parse questions
        let questions_value = params.get("questions").cloned().unwrap_or(Value::Array(vec![]));
        let questions: Vec<Question> = serde_json::from_value(questions_value)
            .map_err(|e| ToolError::InvalidParams(format!("Failed to parse questions: {e}")))?;

        // Validate questions
        if questions.is_empty() {
            return Err(ToolError::InvalidParams("At least one question is required".to_string()));
        }

        if questions.len() > 4 {
            return Err(ToolError::InvalidParams("Maximum 4 questions allowed".to_string()));
        }

        for (i, q) in questions.iter().enumerate() {
            if q.header.len() > 12 {
                return Err(ToolError::InvalidParams(format!(
                    "Question {} header '{}' exceeds 12 characters",
                    i + 1,
                    q.header
                )));
            }

            if q.options.len() < 2 {
                return Err(ToolError::InvalidParams(format!(
                    "Question {} must have at least 2 options",
                    i + 1
                )));
            }

            if q.options.len() > 4 {
                return Err(ToolError::InvalidParams(format!(
                    "Question {} cannot have more than 4 options",
                    i + 1
                )));
            }
        }

        // Check if answers are provided (from UI)
        if let Some(answers) = params.get("answers") {
            if !answers.is_null() && answers.is_object() {
                // Format the answers for the AI
                let mut output = String::from("User responses:\n\n");

                for q in &questions {
                    if let Some(answer) = answers.get(&q.header) {
                        let _ = writeln!(output, "**{}**: {}", q.header, answer);
                    }
                }

                return Ok(ToolOutput::success(output));
            }
        }

        // No answers yet - format questions for display
        // This output will be shown to the user by the TUI
        let mut output = String::from("Waiting for user input:\n\n");

        for (i, q) in questions.iter().enumerate() {
            let _ = writeln!(output, "{}. [{}] {}", i + 1, q.header, q.question);

            for (j, opt) in q.options.iter().enumerate() {
                #[allow(clippy::cast_possible_truncation)]
                let _ = writeln!(
                    output,
                    "   {}) {} - {}",
                    (b'a' + j as u8) as char,
                    opt.label,
                    opt.description
                );
            }

            if q.multi_select {
                output.push_str("   (Multiple selections allowed)\n");
            }

            output.push('\n');
        }

        // Return with special data to indicate this needs user input
        let mut result = ToolOutput::success(output);
        result.data = Some(json!({
            "type": "ask_user_question",
            "questions": questions,
            "awaiting_response": true
        }));

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    #[test]
    fn test_question_option_serialization() {
        let opt = QuestionOption {
            label: "Option A".to_string(),
            description: "First option".to_string(),
        };

        let json = serde_json::to_string(&opt).expect("serialize");
        assert!(json.contains("Option A"));

        let parsed: QuestionOption = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.label, "Option A");
    }

    #[test]
    fn test_question_serialization() {
        let q = Question {
            question: "Which library?".to_string(),
            header: "Library".to_string(),
            options: vec![
                QuestionOption {
                    label: "React".to_string(),
                    description: "Popular UI library".to_string(),
                },
                QuestionOption {
                    label: "Vue".to_string(),
                    description: "Progressive framework".to_string(),
                },
            ],
            multi_select: false,
        };

        let json = serde_json::to_string(&q).expect("serialize");
        assert!(json.contains("multiSelect"));

        let parsed: Question = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.header, "Library");
        assert!(!parsed.multi_select);
    }

    #[tokio::test]
    async fn test_ask_user_tool_basic() {
        let tool = AskUserQuestionTool::new();
        let ctx = ToolContext::default();

        let params = json!({
            "questions": [
                {
                    "question": "Which database should we use?",
                    "header": "Database",
                    "options": [
                        {"label": "PostgreSQL", "description": "Relational database"},
                        {"label": "MongoDB", "description": "Document database"}
                    ],
                    "multiSelect": false
                }
            ]
        });

        let result = tool.execute(params, &ctx).await.expect("should succeed");
        assert!(!result.is_error);
        assert!(result.content.contains("Database"));
        assert!(result.content.contains("PostgreSQL"));
        assert!(result.data.is_some());
    }

    #[tokio::test]
    async fn test_ask_user_tool_with_answers() {
        let tool = AskUserQuestionTool::new();
        let ctx = ToolContext::default();

        let params = json!({
            "questions": [
                {
                    "question": "Which database?",
                    "header": "Database",
                    "options": [
                        {"label": "PostgreSQL", "description": "Relational"},
                        {"label": "MongoDB", "description": "Document"}
                    ],
                    "multiSelect": false
                }
            ],
            "answers": {
                "Database": "PostgreSQL"
            }
        });

        let result = tool.execute(params, &ctx).await.expect("should succeed");
        assert!(!result.is_error);
        assert!(result.content.contains("User responses"));
        assert!(result.content.contains("PostgreSQL"));
    }

    #[tokio::test]
    async fn test_ask_user_tool_validation_no_questions() {
        let tool = AskUserQuestionTool::new();
        let ctx = ToolContext::default();

        let params = json!({
            "questions": []
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ask_user_tool_validation_header_too_long() {
        let tool = AskUserQuestionTool::new();
        let ctx = ToolContext::default();

        let params = json!({
            "questions": [
                {
                    "question": "Test?",
                    "header": "This header is way too long",
                    "options": [
                        {"label": "A", "description": "Option A"},
                        {"label": "B", "description": "Option B"}
                    ],
                    "multiSelect": false
                }
            ]
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ask_user_tool_validation_too_few_options() {
        let tool = AskUserQuestionTool::new();
        let ctx = ToolContext::default();

        let params = json!({
            "questions": [
                {
                    "question": "Test?",
                    "header": "Test",
                    "options": [
                        {"label": "Only one", "description": "Single option"}
                    ],
                    "multiSelect": false
                }
            ]
        });

        let result = tool.execute(params, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_ask_user_tool_multi_select() {
        let tool = AskUserQuestionTool::new();
        let ctx = ToolContext::default();

        let params = json!({
            "questions": [
                {
                    "question": "Which features do you want?",
                    "header": "Features",
                    "options": [
                        {"label": "Auth", "description": "Authentication"},
                        {"label": "API", "description": "REST API"},
                        {"label": "UI", "description": "User interface"}
                    ],
                    "multiSelect": true
                }
            ]
        });

        let result = tool.execute(params, &ctx).await.expect("should succeed");
        assert!(!result.is_error);
        assert!(result.content.contains("Multiple selections allowed"));
    }
}
