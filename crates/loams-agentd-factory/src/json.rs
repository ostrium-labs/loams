//! The TypeScript ops' projection helpers (`rec`, `arr`, `str`, `num`, `numbers`),
//! with JavaScript's semantics: a missing or mistyped field is simply absent.

use serde_json::{Map, Number, Value};

/// An object, or an empty one.
pub fn rec(value: &Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map.clone(),
        _ => Map::new(),
    }
}

/// A field of an object (`undefined` as `None`).
pub fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|map| map.get(key))
}

/// An array's items as objects (a non-object item is `{}`), or none.
pub fn arr(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Object(_) => item.clone(),
                _ => Value::Object(Map::new()),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A string field.
pub fn str_of(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

/// A number field.
pub fn num_of(value: Option<&Value>) -> Option<Number> {
    match value {
        Some(Value::Number(n)) => Some(n.clone()),
        _ => None,
    }
}

/// The numeric fields of an object, in order.
pub fn numbers(value: Option<&Value>) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(Value::Object(map)) = value {
        for (key, v) in map {
            if let Value::Number(n) = v {
                out.insert(key.clone(), Value::Number(n.clone()));
            }
        }
    }
    out
}

/// JavaScript's `String(n)` for a JSON number.
pub fn js_string(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    match n.as_f64() {
        Some(f) if f.fract() == 0.0 && f.abs() < 1e21 => format!("{f:.0}"),
        Some(f) => f.to_string(),
        None => n.to_string(),
    }
}

/// An object built field by field; `None` leaves the key out (as `JSON.stringify`
/// drops `undefined`).
#[derive(Debug, Default)]
pub struct Obj(Map<String, Value>);

impl Obj {
    pub fn new() -> Self {
        Self(Map::new())
    }

    pub fn put(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.0.insert(key.to_owned(), value.into());
        self
    }

    pub fn opt(mut self, key: &str, value: Option<impl Into<Value>>) -> Self {
        if let Some(value) = value {
            self.0.insert(key.to_owned(), value.into());
        }
        self
    }

    pub fn extend(mut self, map: Map<String, Value>) -> Self {
        for (key, value) in map {
            self.0.insert(key, value);
        }
        self
    }

    pub fn done(self) -> Value {
        Value::Object(self.0)
    }
}

/// The first `max` characters of `text` (JavaScript's `slice` by UTF-16 code units;
/// the same for the BMP).
pub fn slice_chars(text: &str, max: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in text.chars() {
        units += c.len_utf16();
        if units > max {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn projections_follow_javascript() {
        let v = json!({"a": 1, "b": "x", "c": [1, {"d": 2}], "n": 2.5});
        assert_eq!(str_of(get(&v, "b")), Some("x".into()));
        assert_eq!(str_of(get(&v, "a")), None);
        assert_eq!(arr(get(&v, "c")), vec![json!({}), json!({"d": 2})]);
        assert_eq!(Value::Object(numbers(Some(&v))), json!({"a": 1, "n": 2.5}));
        assert_eq!(js_string(&Number::from(42)), "42");
        assert_eq!(slice_chars("héllo", 2), "hé");
        assert_eq!(
            Obj::new().put("a", 1).opt("b", None::<String>).done(),
            json!({"a": 1})
        );
    }
}
