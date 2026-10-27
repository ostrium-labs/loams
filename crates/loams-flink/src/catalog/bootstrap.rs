//! The session bootstrap script (D1981).
//!
//! A [`SessionSpec`] is what an instance (and a saved job on top of it)
//! asks a session to have: `SET` options, the Iceberg catalog, Postgres
//! catalogs and connector tables. [`SessionSpec::script`] turns it into the
//! ordered statements the SQL Gateway runs before any user statement:
//!
//! 1. `SET` options (runtime mode, checkpointing, time zone, the savepoint
//!    directory, then the spec's own settings, which win);
//! 2. `CREATE CATALOG IF NOT EXISTS` for Iceberg, then each Postgres
//!    catalog;
//! 3. `CREATE TEMPORARY TABLE IF NOT EXISTS` for each connector table.
//!
//! The session stays in `default_catalog.default_database`, so connector
//! tables are named bare and lake tables as `loams.<db>.<table>`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::iceberg::IcebergCatalog;
use super::jdbc::JdbcCatalog;
use super::quote::{RenderCtx, SetOption, SqlError, check_option_key};
use super::{CatalogDdl, check_catalog_name};
use crate::connectors::{TableDdl, TableSpec};

/// The checkpoint interval sessions get: Iceberg commits on checkpoints, so
/// this is also how fresh the lake is.
pub const DEFAULT_CHECKPOINT_INTERVAL: &str = "30 s";

/// What a session needs before user statements.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSpec {
    /// Extra `SET` options; a key here replaces the generated default.
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// `execution.checkpointing.interval`; `None` = 30 s.
    #[serde(default)]
    pub checkpoint_interval: Option<String>,
    /// `execution.checkpointing.savepoint-dir`: the instance's state prefix
    /// on Loams object storage (`s3://<bucket>/engines/<project>/<id>/savepoints`).
    #[serde(default)]
    pub savepoint_dir: Option<String>,
    #[serde(default)]
    pub iceberg: Option<IcebergCatalog>,
    #[serde(default)]
    pub postgres: Vec<JdbcCatalog>,
    #[serde(default)]
    pub tables: Vec<TableSpec>,
}

impl SessionSpec {
    /// This spec with `other`'s catalogs, tables and settings added (a saved
    /// job's own tables on top of the instance's bootstrap). `other` wins
    /// on settings and on the Iceberg catalog; tables and Postgres catalogs
    /// are appended, and duplicates are refused by [`Self::script`].
    pub fn merged(&self, other: &SessionSpec) -> SessionSpec {
        let mut out = self.clone();
        out.settings
            .extend(other.settings.iter().map(|(k, v)| (k.clone(), v.clone())));
        if other.checkpoint_interval.is_some() {
            out.checkpoint_interval
                .clone_from(&other.checkpoint_interval);
        }
        if other.savepoint_dir.is_some() {
            out.savepoint_dir.clone_from(&other.savepoint_dir);
        }
        if other.iceberg.is_some() {
            out.iceberg.clone_from(&other.iceberg);
        }
        out.postgres.extend(other.postgres.iter().cloned());
        out.tables.extend(other.tables.iter().cloned());
        out
    }

    /// The `SET` options, defaults first, the spec's settings replacing
    /// them by key.
    fn set_options(&self) -> Result<Vec<SetOption>, SqlError> {
        let mut defaults: Vec<(String, String)> = vec![
            ("execution.runtime-mode".into(), "streaming".into()),
            (
                "execution.checkpointing.interval".into(),
                self.checkpoint_interval
                    .clone()
                    .unwrap_or_else(|| DEFAULT_CHECKPOINT_INTERVAL.to_string()),
            ),
            ("table.local-time-zone".into(), "UTC".into()),
        ];
        if let Some(dir) = &self.savepoint_dir {
            if !(dir.starts_with("s3://") || dir.starts_with("file://")) {
                return Err(SqlError::Invalid(format!(
                    "savepoint dir {dir:?} is not an s3:// or file:// URI"
                )));
            }
            defaults.push(("execution.checkpointing.savepoint-dir".into(), dir.clone()));
        }
        let mut out = Vec::new();
        for (k, v) in &defaults {
            if !self.settings.contains_key(k) {
                out.push(SetOption::new(k, v.clone()));
            }
        }
        for (k, v) in &self.settings {
            check_option_key(k)?;
            out.push(SetOption::new(k, v.clone()));
        }
        Ok(out)
    }

    /// The bootstrap script.
    pub fn script(&self) -> Result<BootstrapScript, SqlError> {
        let mut statements = Vec::new();
        for s in self.set_options()? {
            statements.push(BootstrapStatement::Set(s));
        }
        let mut catalogs = BTreeSet::new();
        catalogs.insert("default_catalog".to_string());
        if let Some(ice) = &self.iceberg {
            let ddl = ice.ddl()?;
            if !catalogs.insert(ddl.name.clone()) {
                return Err(SqlError::Invalid(format!(
                    "catalog {:?} given twice",
                    ddl.name
                )));
            }
            statements.push(BootstrapStatement::Catalog(ddl));
        }
        for pg in &self.postgres {
            check_catalog_name(&pg.name)?;
            let ddl = pg.ddl()?;
            if !catalogs.insert(ddl.name.clone()) {
                return Err(SqlError::Invalid(format!(
                    "catalog {:?} given twice (name each Postgres catalog)",
                    ddl.name
                )));
            }
            statements.push(BootstrapStatement::Catalog(ddl));
        }
        let mut tables = BTreeSet::new();
        for t in &self.tables {
            if !tables.insert(t.name().to_string()) {
                return Err(SqlError::Invalid(format!(
                    "table {:?} given twice",
                    t.name()
                )));
            }
            statements.push(BootstrapStatement::Table(t.ddl()?));
        }
        Ok(BootstrapScript { statements })
    }
}

/// One bootstrap statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapStatement {
    Set(SetOption),
    Catalog(CatalogDdl),
    Table(TableDdl),
}

impl BootstrapStatement {
    pub fn render(&self, ctx: &RenderCtx<'_>) -> Result<String, SqlError> {
        match self {
            Self::Set(s) => s.render(ctx),
            Self::Catalog(c) => c.render(ctx),
            Self::Table(t) => t.render(ctx),
        }
    }

    fn refs(&self) -> Vec<String> {
        match self {
            Self::Set(s) => s.value.refs().map(str::to_string).collect(),
            Self::Catalog(c) => c.options.refs().map(str::to_string).collect(),
            Self::Table(t) => t.options.refs().map(str::to_string).collect(),
        }
    }
}

/// The ordered statements a session runs first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootstrapScript {
    pub statements: Vec<BootstrapStatement>,
}

impl BootstrapScript {
    /// The secret references the script needs resolved.
    pub fn secret_refs(&self) -> BTreeSet<String> {
        self.statements.iter().flat_map(|s| s.refs()).collect()
    }

    /// Whether any statement carries an inline secret (D1985: saved jobs
    /// keep references only).
    pub fn has_inline_secret(&self) -> bool {
        self.statements.iter().any(|s| match s {
            BootstrapStatement::Set(o) => o.value.has_inline_secret(),
            BootstrapStatement::Catalog(c) => c.options.has_inline_secret(),
            BootstrapStatement::Table(t) => t.options.has_inline_secret(),
        })
    }

    /// Each statement, without a trailing `;` (what the SQL Gateway takes).
    pub fn render(&self, ctx: &RenderCtx<'_>) -> Result<Vec<String>, SqlError> {
        self.statements.iter().map(|s| s.render(ctx)).collect()
    }

    /// One script, each statement ended with `;`, blank lines between
    /// (what people read, and what `sql-client.sh -i` takes).
    pub fn render_script(&self, ctx: &RenderCtx<'_>) -> Result<String, SqlError> {
        let mut out = String::new();
        for (i, s) in self.render(ctx)?.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(s);
            out.push_str(";\n");
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::quote::Secret;

    #[test]
    fn settings_replace_defaults() {
        let mut spec = SessionSpec::default();
        spec.settings
            .insert("table.local-time-zone".into(), "Asia/Kolkata".into());
        let script = spec.script().unwrap_or_default();
        let sql = script.render(&RenderCtx::redacted()).unwrap_or_default();
        assert_eq!(
            sql,
            vec![
                "SET 'execution.runtime-mode' = 'streaming'",
                "SET 'execution.checkpointing.interval' = '30 s'",
                "SET 'table.local-time-zone' = 'Asia/Kolkata'",
            ]
        );
    }

    #[test]
    fn duplicate_catalogs_and_tables_are_refused() {
        let pg = JdbcCatalog {
            name: "loams_pg".into(),
            host: "compute1".into(),
            port: 5432,
            database: "app".into(),
            username: "u".into(),
            password: Secret::reference("pg"),
            extra: BTreeMap::new(),
        };
        let spec = SessionSpec {
            postgres: vec![pg.clone(), pg],
            ..SessionSpec::default()
        };
        assert!(spec.script().is_err());
    }

    #[test]
    fn savepoint_dir_must_be_a_uri() {
        let spec = SessionSpec {
            savepoint_dir: Some("/tmp/sp".into()),
            ..SessionSpec::default()
        };
        assert!(spec.script().is_err());
    }

    #[test]
    fn merged_appends_tables_and_overrides_settings() {
        let mut a = SessionSpec::default();
        a.settings.insert("k".into(), "1".into());
        let mut b = SessionSpec::default();
        b.settings.insert("k".into(), "2".into());
        b.checkpoint_interval = Some("10 s".into());
        let m = a.merged(&b);
        assert_eq!(m.settings.get("k").map(String::as_str), Some("2"));
        assert_eq!(m.checkpoint_interval.as_deref(), Some("10 s"));
    }
}
