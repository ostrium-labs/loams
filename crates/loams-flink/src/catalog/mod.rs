//! The session bootstrap: catalogs and `SET` options a Flink SQL Gateway
//! session runs before any user statement (D1981).

pub mod bootstrap;
pub mod iceberg;
pub mod jdbc;
pub mod quote;

pub use bootstrap::{BootstrapScript, SessionSpec};
pub use iceberg::{IcebergAuth, IcebergCatalog, S3Access, S3Storage};
pub use jdbc::JdbcCatalog;
pub use quote::{
    OptValue, Part, RenderCtx, Secret, SetOption, SqlError, WithOptions, quote_ident,
    quote_literal, quote_name,
};

/// `CREATE CATALOG IF NOT EXISTS `name` WITH (…)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDdl {
    pub name: String,
    pub options: WithOptions,
}

impl CatalogDdl {
    pub fn render(&self, ctx: &RenderCtx<'_>) -> Result<String, SqlError> {
        Ok(format!(
            "CREATE CATALOG IF NOT EXISTS {} {}",
            quote_ident(&self.name)?,
            self.options.render(ctx)?
        ))
    }
}

/// Catalog names are plain lowercase words, so users can type them
/// unquoted (`loams.analytics.t`).
pub(crate) fn check_catalog_name(name: &str) -> Result<(), SqlError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(SqlError::Invalid(format!(
            "catalog name {name:?} is not [a-z][a-z0-9_]{{0,63}}"
        )))
    }
}

/// `http://…` or `https://…`, no whitespace.
pub(crate) fn check_http_url(what: &str, url: &str) -> Result<(), SqlError> {
    let ok = (url.starts_with("http://") || url.starts_with("https://"))
        && url.len() > "https://".len()
        && !url.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        Ok(())
    } else {
        Err(SqlError::Invalid(format!(
            "{what} {url:?} is not an http(s) URL"
        )))
    }
}
