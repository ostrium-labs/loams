//! The ops' parameter schemas (the TypeScript zod schemas, ported): strict objects,
//! so a key no schema declares is refused (§19.5).

use serde_json::{Map, Value};

use crate::error::OpError;

/// A parsed params object.
#[derive(Debug)]
pub struct Params(Map<String, Value>);

fn bad(message: String) -> OpError {
    OpError::param(message)
}

impl Params {
    /// `params` must be an object (or absent) whose keys are all in `allowed`.
    ///
    /// # Errors
    ///
    /// `bad_params` for anything else.
    pub fn strict(params: &Value, allowed: &[&str]) -> Result<Self, OpError> {
        let map = match params {
            Value::Null => Map::new(),
            Value::Object(map) => map.clone(),
            _ => return Err(bad("params: Expected object".into())),
        };
        if let Some(key) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(bad(format!("params: Unrecognized key: \"{key}\"")));
        }
        Ok(Self(map))
    }

    fn present(&self, key: &str) -> Option<&Value> {
        match self.0.get(key) {
            None | Some(Value::Null) => None,
            Some(v) => Some(v),
        }
    }

    /// An optional integer of at least 1 (`z.number().int().min(1)`).
    ///
    /// # Errors
    ///
    /// Not an integer, or below 1.
    pub fn int_min1(&self, key: &str) -> Result<Option<i64>, OpError> {
        let Some(v) = self.0.get(key) else {
            return Ok(None);
        };
        let n = match v {
            Value::Number(n) => n
                .as_i64()
                .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
            _ => None,
        };
        match n {
            Some(n) if n >= 1 => Ok(Some(n)),
            Some(_) => Err(bad(format!("{key}: Too small: expected number to be >=1"))),
            None => Err(bad(format!("{key}: Expected integer"))),
        }
    }

    /// An optional string of at most `max` characters.
    ///
    /// # Errors
    ///
    /// Not a string, or too long.
    pub fn str_max(&self, key: &str, max: usize) -> Result<Option<String>, OpError> {
        let Some(v) = self.0.get(key) else {
            return Ok(None);
        };
        match v {
            Value::String(s) if s.chars().count() <= max => Ok(Some(s.clone())),
            Value::String(_) => Err(bad(format!(
                "{key}: Too big: expected string to have <={max} characters"
            ))),
            _ => Err(bad(format!("{key}: Expected string"))),
        }
    }

    /// An optional string of `min..=max` characters.
    ///
    /// # Errors
    ///
    /// Not a string, or out of range.
    pub fn str_range(&self, key: &str, min: usize, max: usize) -> Result<Option<String>, OpError> {
        let s = self.str_max(key, max)?;
        match s {
            Some(s) if s.chars().count() < min => Err(bad(format!(
                "{key}: Too small: expected string to have >={min} characters"
            ))),
            other => Ok(other),
        }
    }

    /// A required string of `min..=max` characters.
    ///
    /// # Errors
    ///
    /// Missing, not a string, or out of range.
    pub fn required_str(&self, key: &str, min: usize, max: usize) -> Result<String, OpError> {
        self.str_range(key, min, max)?
            .ok_or_else(|| bad(format!("{key}: Required")))
    }

    /// An optional string from `values`.
    ///
    /// # Errors
    ///
    /// Not one of them.
    pub fn one_of(&self, key: &str, values: &[&str]) -> Result<Option<String>, OpError> {
        let Some(v) = self.0.get(key) else {
            return Ok(None);
        };
        match v {
            Value::String(s) if values.contains(&s.as_str()) => Ok(Some(s.clone())),
            _ => Err(bad(format!("{key}: Invalid option"))),
        }
    }

    /// An optional string that `check` accepts.
    ///
    /// # Errors
    ///
    /// Not a string, or refused.
    pub fn matching(
        &self,
        key: &str,
        check: impl Fn(&str) -> bool,
    ) -> Result<Option<String>, OpError> {
        let Some(v) = self.0.get(key) else {
            return Ok(None);
        };
        match v {
            Value::String(s) if check(s) => Ok(Some(s.clone())),
            _ => Err(bad(format!("{key}: Invalid string"))),
        }
    }

    /// Whether `key` was given (not absent, not null).
    pub fn has(&self, key: &str) -> bool {
        self.present(key).is_some()
    }
}

/// `Math.min(Math.max(n ?? dflt, 1), 50)`: the ops' page size.
pub fn clamp(n: Option<i64>, dflt: i64) -> i64 {
    n.unwrap_or(dflt).clamp(1, 50)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_objects_refuse_unknown_keys_and_bad_types() {
        assert!(Params::strict(&json!({"evil": 1}), &["q"]).is_err());
        assert!(Params::strict(&json!([1]), &["q"]).is_err());
        let p = Params::strict(&json!({"limit": 0}), &["limit"]).unwrap();
        assert!(p.int_min1("limit").is_err());
        let p = Params::strict(&json!({"limit": 2.5}), &["limit"]).unwrap();
        assert!(p.int_min1("limit").is_err());
        let p = Params::strict(&json!({"limit": 20.0}), &["limit"]).unwrap();
        assert_eq!(p.int_min1("limit").unwrap(), Some(20));
        let p = Params::strict(&Value::Null, &[]).unwrap();
        assert_eq!(p.int_min1("limit").unwrap(), None);
        assert_eq!(clamp(Some(500), 20), 50);
        assert_eq!(clamp(None, 10), 10);
    }
}
