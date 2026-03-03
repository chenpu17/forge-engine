//! 模板渲染引擎
//!
//! 支持简单的 `{{variable}}` 语法，从 `WorkflowState` 中获取值。

use std::sync::OnceLock;

use regex::Regex;

use crate::state::WorkflowState;

/// 模板变量正则表达式
fn template_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"\{\{\s*([a-zA-Z_][a-zA-Z0-9_\.]*)\s*\}\}")
            .unwrap_or_else(|e| panic!("Invalid template regex: {e}"))
    })
}

/// 模板渲染错误
#[derive(Debug, Clone, thiserror::Error)]
pub enum TemplateError {
    /// 变量未找到
    #[error("Variable not found: {0}")]
    VariableNotFound(String),

    /// 无效的变量路径
    #[error("Invalid variable path: {0}")]
    InvalidPath(String),
}

/// 模板渲染器
#[derive(Debug, Default)]
pub struct TemplateRenderer {
    /// 是否严格模式（变量不存在时报错）
    strict: bool,
}

impl TemplateRenderer {
    /// 创建新的渲染器
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置严格模式
    #[must_use]
    pub const fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// 渲染模板
    ///
    /// # Errors
    ///
    /// 当模板中引用的变量不存在（严格模式）或路径无效时返回错误。
    pub fn render(&self, template: &str, state: &WorkflowState) -> Result<String, TemplateError> {
        let mut result = template.to_string();

        for cap in template_regex().captures_iter(template) {
            let full_match = cap.get(0).map_or("", |m| m.as_str());
            let var_path = cap.get(1).map_or("", |m| m.as_str());

            let value = self.resolve_path(var_path, state)?;
            result = result.replace(full_match, &value);
        }

        Ok(result)
    }

    /// 解析变量路径
    fn resolve_path(&self, path: &str, state: &WorkflowState) -> Result<String, TemplateError> {
        let parts: Vec<&str> = path.split('.').collect();

        if parts.is_empty() {
            return Err(TemplateError::InvalidPath(path.to_string()));
        }

        // 获取根变量
        let root = parts[0];
        let value = if let Some(v) = state.get(root) {
            v.clone()
        } else {
            if self.strict {
                return Err(TemplateError::VariableNotFound(root.to_string()));
            }
            return Ok(format!("{{{{{path}}}}}"));
        };

        // 遍历路径
        let result = Self::traverse_path_inner(&value, &parts[1..], self.strict)?;
        Ok(Self::value_to_string_inner(&result))
    }

    /// 遍历 JSON 路径
    fn traverse_path_inner(
        value: &serde_json::Value,
        parts: &[&str],
        strict: bool,
    ) -> Result<serde_json::Value, TemplateError> {
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
            || {
                if strict {
                    Err(TemplateError::InvalidPath(key.to_string()))
                } else {
                    Ok(serde_json::Value::Null)
                }
            },
            |v| Self::traverse_path_inner(&v, &parts[1..], strict),
        )
    }

    /// 将 JSON 值转换为字符串
    fn value_to_string_inner(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Null => String::new(),
            _ => value.to_string(),
        }
    }
}
