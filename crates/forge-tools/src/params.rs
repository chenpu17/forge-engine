//! Parameter extraction utilities for tools
//!
//! Provides helper functions to extract and validate parameters from JSON values,
//! reducing boilerplate code in tool implementations.

use forge_domain::ToolError;
use serde_json::Value;

/// Result type alias using `forge_domain::ToolError`.
type Result<T> = std::result::Result<T, ToolError>;

/// Extract a required string parameter
///
/// # Errors
/// Returns `ToolError::InvalidParams` if the parameter is missing or not a string
pub fn required_str<'a>(params: &'a Value, key: &str) -> Result<&'a str> {
    params[key].as_str().ok_or_else(|| ToolError::InvalidParams(format!("{key} is required")))
}

/// Extract an optional string parameter
#[must_use]
pub fn optional_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params[key].as_str()
}

/// Extract an optional u64 parameter with a default value
#[must_use]
pub fn optional_u64(params: &Value, key: &str, default: u64) -> u64 {
    params[key].as_u64().unwrap_or(default)
}

/// Extract an optional i64 parameter with a default value
#[must_use]
pub fn optional_i64(params: &Value, key: &str, default: i64) -> i64 {
    params[key].as_i64().unwrap_or(default)
}

/// Extract an optional usize parameter with a default value
#[must_use]
pub fn optional_usize(params: &Value, key: &str, default: usize) -> usize {
    #[allow(clippy::cast_possible_truncation)]
    params[key].as_u64().map_or(default, |v| v as usize)
}

/// Extract an optional bool parameter with a default value
#[must_use]
pub fn optional_bool(params: &Value, key: &str, default: bool) -> bool {
    params[key].as_bool().unwrap_or(default)
}

/// Extract a string array parameter (returns empty vec if not present)
#[must_use]
pub fn string_array(params: &Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

/// Extract an optional f64 parameter with a default value
#[must_use]
pub fn optional_f64(params: &Value, key: &str, default: f64) -> f64 {
    params[key].as_f64().unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_required_str() {
        let params = json!({"name": "test"});
        assert_eq!(required_str(&params, "name").unwrap(), "test");
        assert!(required_str(&params, "missing").is_err());
    }

    #[test]
    fn test_optional_str() {
        let params = json!({"name": "test"});
        assert_eq!(optional_str(&params, "name"), Some("test"));
        assert_eq!(optional_str(&params, "missing"), None);
    }

    #[test]
    fn test_optional_u64() {
        let params = json!({"count": 42});
        assert_eq!(optional_u64(&params, "count", 0), 42);
        assert_eq!(optional_u64(&params, "missing", 10), 10);
    }

    #[test]
    fn test_optional_bool() {
        let params = json!({"enabled": true});
        assert!(optional_bool(&params, "enabled", false));
        assert!(!optional_bool(&params, "missing", false));
    }

    #[test]
    fn test_string_array() {
        let params = json!({"tags": ["a", "b", "c"]});
        assert_eq!(string_array(&params, "tags"), vec!["a", "b", "c"]);
        assert!(string_array(&params, "missing").is_empty());
    }
}
