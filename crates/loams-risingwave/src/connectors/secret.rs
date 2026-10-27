//! `CREATE SECRET`, for a RisingWave that has a licence key.
//!
//! **Premium, never a default** (design 81 R4): RisingWave's secret
//! management is a paid-tier feature, and Loams runs RisingWave without a
//! licence key. Managed instances use scoped service credentials written
//! inline instead (R5; see [`super::Credential`]). The builder stays
//! for users who bring their own licensed RisingWave: the `meta` backend
//! encrypts the value with the meta node's key and keeps it out of every
//! statement's stored text, and options reference it as `SECRET "name"`.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{Ddl, DdlError, ObjectName, REDACTED, RenderContext, quote_literal};

/// `CREATE SECRET [IF NOT EXISTS] name WITH (backend = 'meta') AS 'value';`
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSecret {
    pub name: ObjectName,
    #[serde(default)]
    pub if_not_exists: bool,
    /// The secret itself. Never printed by `Debug` or by [`Self::redacted_sql`].
    pub value: String,
}

impl CreateSecret {
    pub fn new(name: impl Into<ObjectName>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            if_not_exists: false,
            value: value.into(),
        }
    }

    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    fn render_value(&self, value: &str) -> Result<String, DdlError> {
        if self.value.is_empty() {
            return Err(DdlError::Missing("secret value"));
        }
        Ok(format!(
            "CREATE SECRET {}{} WITH (backend = 'meta') AS {};",
            if self.if_not_exists {
                "IF NOT EXISTS "
            } else {
                ""
            },
            self.name.render()?,
            quote_literal(value)?
        ))
    }

    /// The statement with the value replaced by `<redacted>`, for logs and
    /// `loams risingwave render`.
    pub fn redacted_sql(&self) -> Result<String, DdlError> {
        self.render_value(REDACTED)
    }
}

impl Ddl for CreateSecret {
    fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        if ctx.is_redacted() {
            self.redacted_sql()
        } else {
            self.render_value(&self.value)
        }
    }
}

impl fmt::Debug for CreateSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateSecret")
            .field("name", &self.name)
            .field("if_not_exists", &self.if_not_exists)
            .field("value", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_render_with_the_meta_backend() {
        let secret = CreateSecret::new("loams_api_key", "lk_test_it's").if_not_exists();
        assert_eq!(
            secret.to_sql().unwrap(),
            "CREATE SECRET IF NOT EXISTS \"loams_api_key\" WITH (backend = 'meta') AS 'lk_test_it''s';"
        );
        assert_eq!(
            secret.redacted_sql().unwrap(),
            "CREATE SECRET IF NOT EXISTS \"loams_api_key\" WITH (backend = 'meta') AS '<redacted>';"
        );
        assert_eq!(
            secret.to_redacted_sql().unwrap(),
            secret.redacted_sql().unwrap()
        );
        assert!(!format!("{secret:?}").contains("lk_test"));
    }

    #[test]
    fn an_empty_secret_is_refused() {
        assert!(CreateSecret::new("x", "").to_sql().is_err());
    }
}
