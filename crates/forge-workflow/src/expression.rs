//! 表达式求值器
//!
//! 支持简单的表达式语法，用于路由条件判断。

use crate::state::WorkflowState;

/// 表达式求值错误
#[derive(Debug, Clone, thiserror::Error)]
pub enum ExpressionError {
    /// 语法错误
    #[error("Syntax error: {0}")]
    SyntaxError(String),

    /// 变量未找到
    #[error("Variable not found: {0}")]
    VariableNotFound(String),

    /// 类型错误
    #[error("Type error: {0}")]
    TypeError(String),
}

/// 表达式求值器
#[derive(Debug, Default)]
pub struct ExpressionEvaluator;

impl ExpressionEvaluator {
    /// 创建新的求值器
    #[must_use] 
    pub const fn new() -> Self {
        Self
    }

    /// 求值表达式，返回 JSON 值
    ///
    /// # Errors
    ///
    /// 当表达式语法错误、变量未找到或类型不匹配时返回错误。
    pub fn evaluate(
        &self,
        expression: &str,
        state: &WorkflowState,
    ) -> Result<serde_json::Value, ExpressionError> {
        let expr = expression.trim();

        // 处理字面量
        if let Some(value) = Self::parse_literal(expr) {
            return Ok(value);
        }

        // 处理变量引用
        if Self::is_variable_path(expr) {
            return Self::resolve_variable(expr, state);
        }

        // 处理比较表达式
        if let Some(result) = self.evaluate_comparison(expr, state)? {
            return Ok(serde_json::Value::Bool(result));
        }

        // 默认返回表达式本身作为字符串
        Ok(serde_json::Value::String(expr.to_string()))
    }

    /// 解析字面量
    fn parse_literal(expr: &str) -> Option<serde_json::Value> {
        // 布尔值
        if expr == "true" {
            return Some(serde_json::Value::Bool(true));
        }
        if expr == "false" {
            return Some(serde_json::Value::Bool(false));
        }

        // 数字
        if let Ok(n) = expr.parse::<i64>() {
            return Some(serde_json::json!(n));
        }
        if let Ok(n) = expr.parse::<f64>() {
            return Some(serde_json::json!(n));
        }

        // 字符串（带引号）
        if (expr.starts_with('"') && expr.ends_with('"'))
            || (expr.starts_with('\'') && expr.ends_with('\''))
        {
            let s = &expr[1..expr.len() - 1];
            return Some(serde_json::Value::String(s.to_string()));
        }

        None
    }

    /// 检查是否为变量路径
    fn is_variable_path(expr: &str) -> bool {
        expr.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && expr.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    }

    /// 解析变量
    fn resolve_variable(
        path: &str,
        state: &WorkflowState,
    ) -> Result<serde_json::Value, ExpressionError> {
        let parts: Vec<&str> = path.split('.').collect();

        if parts.is_empty() {
            return Err(ExpressionError::VariableNotFound(path.to_string()));
        }

        let root = parts[0];
        let value =
            state.get(root).ok_or_else(|| ExpressionError::VariableNotFound(root.to_string()))?;

        Self::traverse_path(value, &parts[1..])
    }

    /// 遍历 JSON 路径
    fn traverse_path(
        value: &serde_json::Value,
        parts: &[&str],
    ) -> Result<serde_json::Value, ExpressionError> {
        if parts.is_empty() {
            return Ok(value.clone());
        }

        let key = parts[0];
        let next = match value {
            serde_json::Value::Object(map) => map.get(key).cloned(),
            serde_json::Value::Array(arr) => {
                key.parse::<usize>().ok().and_then(|i| arr.get(i).cloned())
            }
            _ => None,
        };

        next.map_or_else(
            || Err(ExpressionError::VariableNotFound(key.to_string())),
            |v| Self::traverse_path(&v, &parts[1..]),
        )
    }

    /// 求值比较表达式
    fn evaluate_comparison(
        &self,
        expr: &str,
        state: &WorkflowState,
    ) -> Result<Option<bool>, ExpressionError> {
        // 支持的操作符
        let operators = ["==", "!=", ">=", "<=", ">", "<"];

        for op in operators {
            if let Some(pos) = expr.find(op) {
                let left = expr[..pos].trim();
                let right = expr[pos + op.len()..].trim();

                let left_val = self.evaluate(left, state)?;
                let right_val = self.evaluate(right, state)?;

                let result = match op {
                    "==" => Self::values_equal(&left_val, &right_val),
                    "!=" => !Self::values_equal(&left_val, &right_val),
                    ">" => Self::compare_values(&left_val, &right_val) > 0,
                    "<" => Self::compare_values(&left_val, &right_val) < 0,
                    ">=" => Self::compare_values(&left_val, &right_val) >= 0,
                    "<=" => Self::compare_values(&left_val, &right_val) <= 0,
                    _ => false,
                };

                return Ok(Some(result));
            }
        }

        Ok(None)
    }

    /// 比较两个值是否相等
    fn values_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
        match (a, b) {
            (serde_json::Value::String(s1), serde_json::Value::String(s2)) => s1 == s2,
            (serde_json::Value::Number(n1), serde_json::Value::Number(n2)) => n1 == n2,
            (serde_json::Value::Bool(b1), serde_json::Value::Bool(b2)) => b1 == b2,
            (serde_json::Value::Null, serde_json::Value::Null) => true,
            _ => a == b,
        }
    }

    /// 比较两个值的大小
    fn compare_values(a: &serde_json::Value, b: &serde_json::Value) -> i32 {
        match (a, b) {
            (serde_json::Value::Number(n1), serde_json::Value::Number(n2)) => {
                let f1 = n1.as_f64().unwrap_or(0.0);
                let f2 = n2.as_f64().unwrap_or(0.0);
                if f1 > f2 {
                    1
                } else if f1 < f2 {
                    -1
                } else {
                    0
                }
            }
            (serde_json::Value::String(s1), serde_json::Value::String(s2)) => s1.cmp(s2) as i32,
            _ => 0,
        }
    }
}
