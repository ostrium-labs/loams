//! Listing schema v1 (design §73 §3.6, D1455; MK1 Task 1).
//!
//! `schemas/marketplace/listing.v1.json` is the source of truth for what a
//! listing may contain; it is compiled into this crate so agentd, the index
//! tooling and the conformance runner check listings against the same rules.

use std::fmt;
use std::sync::OnceLock;

use serde_json::Value;

/// The schema's text, as published at [`SCHEMA_ID`].
pub const SCHEMA_JSON: &str = include_str!("../../../schemas/marketplace/listing.v1.json");

/// The schema's `$id`.
pub const SCHEMA_ID: &str = "https://loams.dev/schemas/listing/v1.json";

/// The parsed schema.
pub fn schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| serde_json::from_str(SCHEMA_JSON).expect("listing.v1.json is JSON"))
}

fn validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        jsonschema::draft202012::new(schema()).expect("listing.v1.json is a valid 2020-12 schema")
    })
}

/// One way a listing breaks the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// JSON Pointer to the offending value (empty for the whole listing).
    pub instance_path: String,
    /// JSON Pointer to the schema rule that failed.
    pub schema_path: String,
    pub message: String,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = if self.instance_path.is_empty() {
            "/"
        } else {
            &self.instance_path
        };
        write!(f, "{at}: {}", self.message)
    }
}

/// Checks a listing's JSON form against schema v1. `Ok` means valid; `Err`
/// lists every issue the validator found.
pub fn validate(listing: &Value) -> Result<(), Vec<Issue>> {
    let issues: Vec<Issue> = validator()
        .iter_errors(listing)
        .map(|error| Issue {
            instance_path: error.instance_path().to_string(),
            schema_path: error.schema_path().to_string(),
            message: error.to_string(),
        })
        .collect();
    if issues.is_empty() {
        Ok(())
    } else {
        Err(issues)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn schema_id_matches_the_file() {
        assert_eq!(schema()["$id"], SCHEMA_ID);
    }

    #[test]
    fn an_empty_object_is_invalid() {
        let issues = validate(&serde_json::json!({})).unwrap_err();
        assert!(issues.iter().any(|i| i.message.contains("\"id\"")));
    }
}
