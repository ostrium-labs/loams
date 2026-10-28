//! The JDBC catalog on Loams Postgres (D1984).
//!
//! flink-connector-jdbc's catalog (`'type' = 'jdbc'`) lists a Postgres
//! database's tables as Flink tables, read and written through the JDBC
//! connector. Its `base-url` is `jdbc:postgresql://<host>:<port>/` with no
//! database and no parameters: the catalog appends the database name.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::quote::{Secret, SqlError, WithOptions};
use super::{CatalogDdl, check_catalog_name};

/// The catalog name sessions get unless the spec says otherwise.
pub const DEFAULT_PG_CATALOG: &str = "loams_pg";

fn default_name() -> String {
    DEFAULT_PG_CATALOG.to_string()
}

fn default_port() -> u16 {
    5432
}

/// A JDBC catalog for one Loams Postgres database (a branch's compute
/// endpoint).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JdbcCatalog {
    #[serde(default = "default_name")]
    pub name: String,
    /// The compute endpoint's host on the private network.
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Secret,
    /// More catalog options, after the generated ones.
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

impl JdbcCatalog {
    pub fn base_url(&self) -> Result<String, SqlError> {
        let host_ok = !self.host.is_empty()
            && self
                .host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
        let ipv6 = self.host.len() > 2
            && self.host.starts_with('[')
            && self.host.ends_with(']')
            && self.host[1..self.host.len() - 1]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b':');
        if !(host_ok || ipv6) {
            return Err(SqlError::Invalid(format!(
                "postgres host {:?} is not a host name or address",
                self.host
            )));
        }
        Ok(format!("jdbc:postgresql://{}:{}/", self.host, self.port))
    }

    pub fn ddl(&self) -> Result<CatalogDdl, SqlError> {
        check_catalog_name(&self.name)?;
        if self.database.is_empty() || self.database.contains('/') || self.database.contains('?') {
            return Err(SqlError::Invalid(format!(
                "postgres database {:?} is empty or has '/' or '?'",
                self.database
            )));
        }
        if self.username.is_empty() {
            return Err(SqlError::Invalid("postgres username is empty".into()));
        }
        let mut o = WithOptions::new();
        o.set("type", "jdbc")
            .set("base-url", self.base_url()?)
            .set("default-database", self.database.clone())
            .set("username", self.username.clone())
            .set_secret("password", self.password.clone())
            .extend(&self.extra);
        Ok(CatalogDdl {
            name: self.name.clone(),
            options: o,
        })
    }
}
