//! The JSON Schema a structured-output call is held to.
//!
//! Every provider validates the model's answer here, whatever the server
//! promised: `response_format: json_schema`, a tool's `input_schema` and the
//! instruction fallback all end in [`JsonSchema::validate`].

use std::fmt;
use std::sync::Arc;

use serde_json::Value;

use crate::error::LlmError;

/// A compiled JSON Schema with a name (the OpenAI `json_schema.name` and the
/// Anthropic tool name).
#[derive(Clone)]
pub struct JsonSchema {
    name: String,
    schema: Value,
    strict: bool,
    validator: Arc<jsonschema::Validator>,
}

impl fmt::Debug for JsonSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonSchema")
            .field("name", &self.name)
            .field("strict", &self.strict)
            .finish_non_exhaustive()
    }
}

/// `^[a-zA-Z0-9_-]{1,64}$`, the name both providers accept.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl JsonSchema {
    /// Compiles `schema`. The name must match `^[a-zA-Z0-9_-]{1,64}$`.
    /// External `$ref`s are not resolved (the crate is built without the
    /// HTTP and file resolvers).
    pub fn new(name: &str, schema: Value) -> Result<Self, LlmError> {
        if !valid_name(name) {
            return Err(LlmError::InvalidConfig(
                "schema name must match ^[a-zA-Z0-9_-]{1,64}$".to_string(),
            ));
        }
        let validator = jsonschema::validator_for(&schema)
            .map_err(|_| LlmError::InvalidConfig("schema does not compile".to_string()))?;
        Ok(Self {
            name: name.to_string(),
            schema,
            strict: false,
            validator: Arc::new(validator),
        })
    }

    /// Asks providers that support it for strict schema adherence
    /// (`strict: true`). The schema must then meet the provider's strict
    /// subset (`additionalProperties: false`, every property required).
    #[must_use]
    pub fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// The schema's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The schema itself.
    #[must_use]
    pub fn schema(&self) -> &Value {
        &self.schema
    }

    /// Whether strict adherence is requested.
    #[must_use]
    pub fn strict(&self) -> bool {
        self.strict
    }

    /// Validates `value`. The error names the first failing instance path
    /// and the schema keyword, never the value.
    pub fn validate(&self, value: &Value) -> Result<(), LlmError> {
        match self.validator.validate(value) {
            Ok(()) => Ok(()),
            Err(e) => {
                let path = e.instance_path().to_string();
                let path = if path.is_empty() {
                    "/".to_string()
                } else {
                    path
                };
                Err(LlmError::InvalidOutput(format!(
                    "{path}: {}",
                    e.kind().keyword()
                )))
            }
        }
    }

    /// Parses a model's text answer as JSON and validates it. A Markdown
    /// code fence around the object is accepted (the instruction fallback).
    pub fn parse_text(&self, text: &str) -> Result<Value, LlmError> {
        let value: Value = serde_json::from_str(strip_fence(text))
            .map_err(|_| LlmError::InvalidOutput("not JSON".to_string()))?;
        self.validate(&value)?;
        Ok(value)
    }
}

/// The text inside a single ```` ```json ```` (or bare ```` ``` ````) fence,
/// else the trimmed text.
fn strip_fence(text: &str) -> &str {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    match rest.strip_suffix("```") {
        Some(inner) => inner.trim(),
        None => t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fences_are_stripped() {
        assert_eq!(strip_fence("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(strip_fence("  {\"a\":1} "), "{\"a\":1}");
        assert_eq!(strip_fence("```\n[]\n```"), "[]");
    }

    #[test]
    fn names_are_checked() {
        assert!(JsonSchema::new("facts_v1", json!({"type": "object"})).is_ok());
        assert!(JsonSchema::new("bad name", json!({"type": "object"})).is_err());
        assert!(JsonSchema::new("", json!({"type": "object"})).is_err());
    }
}
