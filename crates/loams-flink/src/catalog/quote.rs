//! Flink SQL quoting and the `WITH (…)` option list (D1980).
//!
//! Every identifier is backtick-quoted with backticks doubled, every string
//! literal single-quoted with quotes doubled, and option keys are checked
//! against a conservative alphabet. Nothing here concatenates caller text
//! into SQL unquoted.
//!
//! Option values may hold secrets ([`Secret`]). A value is rendered twice:
//! for Flink, with secrets resolved, and for people (logs, API echoes, the
//! CLI), with every secret replaced by a marker (D1985).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

/// Why SQL could not be generated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SqlError {
    #[error("empty identifier")]
    EmptyIdent,
    #[error("identifier {0:?} has a control character")]
    IdentControl(String),
    #[error("string value for {what} has a control character other than tab or newline")]
    LiteralControl { what: String },
    #[error("option key {0:?} is not [A-Za-z0-9_.-]+")]
    BadOptionKey(String),
    #[error("option {0:?} is given twice")]
    DuplicateOption(String),
    #[error("data type {0:?} is not a plain Flink type")]
    BadType(String),
    #[error("secret {0:?} is not available")]
    MissingSecret(String),
    #[error("{0}")]
    Invalid(String),
}

/// Quote an identifier: `` `a``b` `` for ``a`b``.
pub fn quote_ident(name: &str) -> Result<String, SqlError> {
    if name.is_empty() {
        return Err(SqlError::EmptyIdent);
    }
    if name.chars().any(char::is_control) {
        return Err(SqlError::IdentControl(name.to_string()));
    }
    let mut out = String::with_capacity(name.len() + 2);
    out.push('`');
    for c in name.chars() {
        if c == '`' {
            out.push('`');
        }
        out.push(c);
    }
    out.push('`');
    Ok(out)
}

/// Quote a multi-part name, each part on its own: `` `cat`.`db`.`t` ``.
pub fn quote_name(parts: &[&str]) -> Result<String, SqlError> {
    if parts.is_empty() {
        return Err(SqlError::EmptyIdent);
    }
    let quoted = parts
        .iter()
        .map(|p| quote_ident(p))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(quoted.join("."))
}

/// Quote a string literal: `'it''s'`. Tab, newline and carriage return are
/// kept (Flink accepts them inside a literal); other control characters,
/// NUL included, are refused.
pub fn quote_literal(value: &str, what: &str) -> Result<String, SqlError> {
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
    {
        return Err(SqlError::LiteralControl {
            what: what.to_string(),
        });
    }
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        if c == '\'' {
            out.push('\'');
        }
        out.push(c);
    }
    out.push('\'');
    Ok(out)
}

/// Check an option key (`properties.bootstrap.servers`, `scan.startup.mode`,
/// `header.X-Iceberg-Access-Delegation`).
pub fn check_option_key(key: &str) -> Result<(), SqlError> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        return Err(SqlError::BadOptionKey(key.to_string()));
    }
    Ok(())
}

/// A secret value: a reference resolved when a session opens (what saved
/// jobs keep), or the value itself (local development, tests).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Secret {
    /// The name of a project secret.
    Ref(String),
    /// The value, given inline. Never persisted by the job store.
    Inline(String),
}

impl Secret {
    pub fn reference(name: impl Into<String>) -> Self {
        Self::Ref(name.into())
    }

    pub fn inline(value: impl Into<String>) -> Self {
        Self::Inline(value.into())
    }

    fn marker(&self) -> String {
        match self {
            Self::Ref(name) => format!("${{secret:{name}}}"),
            Self::Inline(_) => "***".to_string(),
        }
    }
}

/// How a secret is escaped inside the value that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escape {
    /// As is.
    None,
    /// Inside a double-quoted JAAS option (`password="…"`): backslash and
    /// double quote are backslash-escaped.
    JaasQuoted,
}

/// Escape for a double-quoted JAAS option value.
pub fn jaas_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '\\' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// One piece of an option value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Secret { secret: Secret, escape: Escape },
}

/// An option value: text with secrets in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptValue(pub Vec<Part>);

impl OptValue {
    pub fn text(value: impl Into<String>) -> Self {
        Self(vec![Part::Text(value.into())])
    }

    pub fn secret(secret: Secret) -> Self {
        Self(vec![Part::Secret {
            secret,
            escape: Escape::None,
        }])
    }

    /// The secret references this value needs.
    pub fn refs(&self) -> impl Iterator<Item = &str> {
        self.0.iter().filter_map(|p| match p {
            Part::Secret {
                secret: Secret::Ref(name),
                ..
            } => Some(name.as_str()),
            _ => None,
        })
    }

    /// Whether an inline secret value is in this value (saved jobs refuse
    /// them, D1985).
    pub fn has_inline_secret(&self) -> bool {
        self.0.iter().any(|p| {
            matches!(
                p,
                Part::Secret {
                    secret: Secret::Inline(_),
                    ..
                }
            )
        })
    }

    pub fn has_secret(&self) -> bool {
        self.0.iter().any(|p| matches!(p, Part::Secret { .. }))
    }

    fn resolve(&self, ctx: &RenderCtx<'_>) -> Result<String, SqlError> {
        let mut out = String::new();
        for part in &self.0 {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Secret { secret, escape } => {
                    if ctx.redact {
                        out.push_str(&secret.marker());
                        continue;
                    }
                    let value = match secret {
                        Secret::Inline(v) => v.as_str(),
                        Secret::Ref(name) => ctx
                            .secrets
                            .and_then(|m| m.get(name))
                            .map(String::as_str)
                            .ok_or_else(|| SqlError::MissingSecret(name.clone()))?,
                    };
                    match escape {
                        Escape::None => out.push_str(value),
                        Escape::JaasQuoted => out.push_str(&jaas_escape(value)),
                    }
                }
            }
        }
        Ok(out)
    }
}

/// Rendering context: the resolved secrets, or redaction.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderCtx<'a> {
    pub secrets: Option<&'a BTreeMap<String, String>>,
    pub redact: bool,
}

impl<'a> RenderCtx<'a> {
    /// For people: every secret is a marker; nothing needs resolving.
    pub fn redacted() -> Self {
        Self {
            secrets: None,
            redact: true,
        }
    }

    /// For Flink: references resolved from `secrets`.
    pub fn resolved(secrets: &'a BTreeMap<String, String>) -> Self {
        Self {
            secrets: Some(secrets),
            redact: false,
        }
    }
}

/// An ordered `WITH (…)` option list. Order is insertion order, so output is
/// deterministic and goldens hold; a key given twice is refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WithOptions(Vec<(String, OptValue)>);

impl WithOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) -> &mut Self {
        self.0.push((key.to_string(), OptValue::text(value)));
        self
    }

    pub fn set_opt(&mut self, key: &str, value: Option<impl Into<String>>) -> &mut Self {
        if let Some(v) = value {
            self.set(key, v);
        }
        self
    }

    pub fn set_value(&mut self, key: &str, value: OptValue) -> &mut Self {
        self.0.push((key.to_string(), value));
        self
    }

    pub fn set_secret(&mut self, key: &str, secret: Secret) -> &mut Self {
        self.set_value(key, OptValue::secret(secret))
    }

    /// Add caller-supplied extra options after the generated ones. A key the
    /// builder already set is refused at render time (duplicate).
    pub fn extend(&mut self, extra: &BTreeMap<String, String>) -> &mut Self {
        for (k, v) in extra {
            self.set(k, v.clone());
        }
        self
    }

    pub fn get(&self, key: &str) -> Option<&OptValue> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }

    pub fn refs(&self) -> impl Iterator<Item = &str> {
        self.0.iter().flat_map(|(_, v)| v.refs())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn has_inline_secret(&self) -> bool {
        self.0.iter().any(|(_, v)| v.has_inline_secret())
    }

    /// `WITH (\n  'k' = 'v',\n  …\n)`.
    pub fn render(&self, ctx: &RenderCtx<'_>) -> Result<String, SqlError> {
        let mut seen = std::collections::BTreeSet::new();
        let mut out = String::from("WITH (\n");
        for (i, (key, value)) in self.0.iter().enumerate() {
            check_option_key(key)?;
            if !seen.insert(key.as_str()) {
                return Err(SqlError::DuplicateOption(key.clone()));
            }
            let v = value.resolve(ctx)?;
            let _ = write!(
                out,
                "  {} = {}",
                quote_literal(key, key)?,
                quote_literal(&v, key)?
            );
            out.push_str(if i + 1 == self.0.len() { "\n" } else { ",\n" });
        }
        out.push(')');
        Ok(out)
    }
}

/// `SET 'key' = 'value'`. Values may hold secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetOption {
    pub key: String,
    pub value: OptValue,
}

impl SetOption {
    pub fn new(key: &str, value: impl Into<String>) -> Self {
        Self {
            key: key.to_string(),
            value: OptValue::text(value),
        }
    }

    pub fn render(&self, ctx: &RenderCtx<'_>) -> Result<String, SqlError> {
        check_option_key(&self.key)?;
        let v = self.value.resolve(ctx)?;
        Ok(format!(
            "SET {} = {}",
            quote_literal(&self.key, &self.key)?,
            quote_literal(&v, &self.key)?
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idents_double_backticks() {
        assert_eq!(quote_ident("a").as_deref(), Ok("`a`"));
        assert_eq!(quote_ident("a`b").as_deref(), Ok("`a``b`"));
        assert_eq!(quote_ident("select").as_deref(), Ok("`select`"));
        assert_eq!(quote_ident(""), Err(SqlError::EmptyIdent));
        assert!(quote_ident("a\0b").is_err());
        assert_eq!(
            quote_name(&["loams", "an.alytics", "t"]).as_deref(),
            Ok("`loams`.`an.alytics`.`t`")
        );
    }

    #[test]
    fn literals_double_quotes_and_refuse_nul() {
        assert_eq!(quote_literal("it's", "x").as_deref(), Ok("'it''s'"));
        assert_eq!(quote_literal("a\nb", "x").as_deref(), Ok("'a\nb'"));
        assert_eq!(quote_literal("''", "x").as_deref(), Ok("''''''"));
        assert!(quote_literal("a\0", "x").is_err());
        assert!(quote_literal("a\u{1b}[0m", "x").is_err());
    }

    #[test]
    fn option_keys() {
        assert!(check_option_key("properties.bootstrap.servers").is_ok());
        assert!(check_option_key("header.X-Iceberg-Access-Delegation").is_ok());
        assert!(check_option_key("a'b").is_err());
        assert!(check_option_key("a b").is_err());
        assert!(check_option_key("").is_err());
    }

    #[test]
    fn with_options_render_and_refuse_duplicates() {
        let mut w = WithOptions::new();
        w.set("connector", "kafka").set("topic", "o'neil");
        assert_eq!(
            w.render(&RenderCtx::redacted()).as_deref(),
            Ok("WITH (\n  'connector' = 'kafka',\n  'topic' = 'o''neil'\n)")
        );
        w.set("topic", "again");
        assert_eq!(
            w.render(&RenderCtx::redacted()),
            Err(SqlError::DuplicateOption("topic".into()))
        );
    }

    #[test]
    fn secrets_resolve_redact_and_escape() {
        let v = OptValue(vec![
            Part::Text("password=\"".into()),
            Part::Secret {
                secret: Secret::reference("kafka"),
                escape: Escape::JaasQuoted,
            },
            Part::Text("\";".into()),
        ]);
        let mut secrets = BTreeMap::new();
        secrets.insert("kafka".to_string(), "p\"w\\'d".to_string());
        assert_eq!(
            v.resolve(&RenderCtx::resolved(&secrets)).as_deref(),
            Ok("password=\"p\\\"w\\\\'d\";")
        );
        assert_eq!(
            v.resolve(&RenderCtx::redacted()).as_deref(),
            Ok("password=\"${secret:kafka}\";")
        );
        let empty = BTreeMap::new();
        assert_eq!(
            v.resolve(&RenderCtx::resolved(&empty)),
            Err(SqlError::MissingSecret("kafka".into()))
        );
        assert_eq!(v.refs().collect::<Vec<_>>(), vec!["kafka"]);
    }
}
