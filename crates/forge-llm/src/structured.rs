//! Structured output / JSON mode support.
//!
//! Provides utilities for requesting structured JSON responses from LLM providers.

use serde_json::Value;

/// Name used for the synthetic tool in the Anthropic tool-use-as-structured-output pattern.
pub const STRUCTURED_TOOL_NAME: &str = "__structured_output";

/// Build a system prompt instruction that tells the LLM to respond with JSON
/// matching the given schema.
pub fn build_schema_instruction(schema: &Value) -> String {
    let schema_str = serde_json::to_string_pretty(schema).unwrap_or_else(|_| schema.to_string());
    format!(
        "\n\n[Structured Output]\n\
         You MUST respond with a valid JSON object that conforms to the following JSON Schema.\n\
         Do NOT include any text before or after the JSON. Output ONLY the JSON object.\n\n\
         ```json\n{schema_str}\n```"
    )
}

/// Build the OpenAI `response_format` value for structured output.
pub fn build_openai_response_format(schema: &Value) -> Value {
    serde_json::json!({
        "type": "json_schema",
        "json_schema": {
            "name": "structured_response",
            "strict": true,
            "schema": schema
        }
    })
}

/// Build a synthetic tool definition for the Anthropic tool-use-as-structured-output pattern.
pub fn build_anthropic_structured_tool(schema: &Value) -> Value {
    serde_json::json!({
        "name": STRUCTURED_TOOL_NAME,
        "description": "Return a structured JSON response matching the required schema. \
                        You MUST call this tool with the response data.",
        "input_schema": schema
    })
}

/// Build the `tool_choice` value that forces the LLM to use the structured output tool.
pub fn build_anthropic_tool_choice() -> Value {
    serde_json::json!({
        "type": "tool",
        "name": STRUCTURED_TOOL_NAME
    })
}

/// Validate that `text` is valid JSON and perform basic schema checks.
pub fn validate_json_response(text: &str, schema: &Value) -> std::result::Result<Value, String> {
    let value: Value =
        serde_json::from_str(text.trim()).map_err(|e| format!("Invalid JSON: {e}"))?;
    validate_value(&value, schema)?;
    Ok(value)
}

/// Recursively validate a `Value` against a JSON Schema (basic subset).
fn validate_value(value: &Value, schema: &Value) -> std::result::Result<(), String> {
    if let Some(expected_type) = schema.get("type").and_then(|t| t.as_str()) {
        let actual_type = json_type_name(value);
        let type_matches =
            actual_type == expected_type || (expected_type == "number" && actual_type == "integer");
        if !type_matches {
            return Err(format!("Expected type \"{expected_type}\", got \"{actual_type}\""));
        }
    }

    if let Some(obj) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
            for req in required {
                if let Some(field_name) = req.as_str() {
                    if !obj.contains_key(field_name) {
                        return Err(format!("Missing required field: \"{field_name}\""));
                    }
                }
            }
        }
        if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
            for (key, prop_schema) in properties {
                if let Some(prop_value) = obj.get(key) {
                    validate_value(prop_value, prop_schema)?;
                }
            }
        }
    }

    if let Some(arr) = value.as_array() {
        if let Some(items_schema) = schema.get("items") {
            for (i, item) in arr.iter().enumerate() {
                validate_value(item, items_schema).map_err(|e| format!("Array item [{i}]: {e}"))?;
            }
        }
    }

    if let Some(enum_values) = schema.get("enum").and_then(|e| e.as_array()) {
        if !enum_values.contains(value) {
            return Err(format!(
                "Value {} not in allowed enum values",
                serde_json::to_string(value).unwrap_or_default()
            ));
        }
    }

    Ok(())
}

/// Map a `serde_json::Value` to its JSON Schema type name.
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_build_schema_instruction() {
        let schema = json!({
            "type": "object",
            "properties": { "name": {"type": "string"} },
            "required": ["name"]
        });
        let instruction = build_schema_instruction(&schema);
        assert!(instruction.contains("[Structured Output]"));
        assert!(instruction.contains("\"name\""));
    }

    #[test]
    fn test_build_openai_response_format() {
        let schema = json!({"type": "object"});
        let rf = build_openai_response_format(&schema);
        assert_eq!(rf["type"], "json_schema");
        assert_eq!(rf["json_schema"]["name"], "structured_response");
    }

    #[test]
    fn test_build_anthropic_structured_tool() {
        let schema = json!({"type": "object", "properties": {"x": {"type": "integer"}}});
        let tool = build_anthropic_structured_tool(&schema);
        assert_eq!(tool["name"], STRUCTURED_TOOL_NAME);
    }

    #[test]
    fn test_validate_valid_object() {
        let schema = json!({
            "type": "object",
            "properties": { "name": {"type": "string"}, "age": {"type": "integer"} },
            "required": ["name"]
        });
        let result = validate_json_response(r#"{"name": "Alice", "age": 30}"#, &schema);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_missing_required() {
        let schema = json!({
            "type": "object",
            "properties": { "name": {"type": "string"} },
            "required": ["name"]
        });
        let result = validate_json_response(r#"{"age": 30}"#, &schema);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Missing required field"));
    }

    #[test]
    fn test_validate_wrong_type() {
        let schema = json!({"type": "object"});
        let result = validate_json_response(r#""just a string""#, &schema);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_invalid_json() {
        let schema = json!({"type": "object"});
        let result = validate_json_response("not json at all", &schema);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_array_items() {
        let schema = json!({"type": "array", "items": {"type": "string"}});
        assert!(validate_json_response(r#"["a", "b", "c"]"#, &schema).is_ok());
        let err = validate_json_response(r#"["a", 42]"#, &schema);
        assert!(err.is_err());
        assert!(err.unwrap_err().contains("Array item [1]"));
    }

    #[test]
    fn test_validate_enum() {
        let schema = json!({"type": "string", "enum": ["red", "green", "blue"]});
        assert!(validate_json_response(r#""red""#, &schema).is_ok());
        assert!(validate_json_response(r#""yellow""#, &schema).is_err());
    }

    #[test]
    fn test_validate_number_vs_integer() {
        let int_schema = json!({"type": "integer"});
        assert!(validate_json_response("42", &int_schema).is_ok());

        let num_schema = json!({"type": "number"});
        assert!(validate_json_response("3.14", &num_schema).is_ok());
        assert!(validate_json_response("42", &num_schema).is_ok());
        assert!(validate_json_response("3.14", &int_schema).is_err());
    }
}
