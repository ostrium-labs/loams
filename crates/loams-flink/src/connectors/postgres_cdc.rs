//! Postgres CDC through flink-cdc (`'connector' = 'postgres-cdc'`,
//! Apache-2.0) against a Loams Postgres compute endpoint (D1984).
//!
//! Logical replication with `pgoutput` and the incremental snapshot
//! (parallel, lock-free; it needs a primary key). ENG81 R12: the controller
//! creates the publication under the source's CDC role, so Flink CDC never
//! creates one (`debezium.publication.autocreate.mode = 'disabled'`), and
//! slots are named `loams_fk_<instance26>_<n>` ([`slot_name`]) so two
//! sources never share one and the controller can drop them on delete.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{Column, TableDdl, Watermark};
use crate::catalog::quote::{Secret, SqlError, WithOptions};

fn default_port() -> u16 {
    5432
}

fn default_schema() -> String {
    "public".to_string()
}

fn default_sslmode() -> String {
    "require".to_string()
}

/// Where the CDC source starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CdcStartup {
    /// A snapshot of the table, then the changes after it.
    #[default]
    Initial,
    /// Only changes from now on.
    LatestOffset,
    /// Only the snapshot (a bounded source).
    Snapshot,
}

/// A Postgres table captured as a changelog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostgresCdcTable {
    /// The Flink table name.
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub database: String,
    #[serde(default = "default_schema")]
    pub schema: String,
    pub table: String,
    pub username: String,
    pub password: Secret,
    pub columns: Vec<Column>,
    pub primary_key: Vec<String>,
    #[serde(default)]
    pub watermark: Option<Watermark>,
    /// The replication slot, `loams_fk_<instance26>_<n>` ([`slot_name`]).
    pub slot: String,
    /// The publication the controller created; defaults to the slot name.
    #[serde(default)]
    pub publication: Option<String>,
    #[serde(default)]
    pub startup: CdcStartup,
    /// libpq's `sslmode` for the replication connection.
    #[serde(default = "default_sslmode")]
    pub sslmode: String,
    #[serde(default)]
    pub extra: BTreeMap<String, String>,
}

/// The slot (and default publication) name for source `n` of an instance
/// (ENG81 R12): `loams_fk_<instance26>_<n>`, where `instance26` is the
/// instance id without its `eng_` prefix.
pub fn slot_name(instance_id: &str, n: u32) -> String {
    let id = instance_id.strip_prefix("eng_").unwrap_or(instance_id);
    format!("loams_fk_{}_{n}", id.to_ascii_lowercase())
}

fn check_pg_name(what: &str, name: &str) -> Result<(), SqlError> {
    // Postgres's rule for slot names (and what we use for publications):
    // lowercase letters, digits and underscores, at most 63 bytes.
    if name.is_empty()
        || name.len() > 63
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Err(SqlError::Invalid(format!(
            "{what} {name:?} is not [a-z0-9_]{{1,63}}"
        )));
    }
    Ok(())
}

impl PostgresCdcTable {
    pub fn publication_name(&self) -> &str {
        self.publication.as_deref().unwrap_or(&self.slot)
    }

    pub fn ddl(&self) -> Result<TableDdl, SqlError> {
        if self.primary_key.is_empty() {
            return Err(SqlError::Invalid(format!(
                "postgres-cdc table {:?} needs a primary key (the incremental snapshot splits on it)",
                self.name
            )));
        }
        check_pg_name("replication slot", &self.slot)?;
        check_pg_name("publication", self.publication_name())?;
        if !matches!(
            self.sslmode.as_str(),
            "disable" | "allow" | "prefer" | "require" | "verify-ca" | "verify-full"
        ) {
            return Err(SqlError::Invalid(format!(
                "sslmode {:?} is not a libpq sslmode",
                self.sslmode
            )));
        }
        for (what, v) in [
            ("host", &self.host),
            ("database", &self.database),
            ("schema", &self.schema),
            ("table", &self.table),
            ("username", &self.username),
        ] {
            if v.is_empty() {
                return Err(SqlError::Invalid(format!("postgres-cdc {what} is empty")));
            }
        }
        let mut o = WithOptions::new();
        o.set("connector", "postgres-cdc")
            .set("hostname", self.host.clone())
            .set("port", self.port.to_string())
            .set("username", self.username.clone())
            .set_secret("password", self.password.clone())
            .set("database-name", self.database.clone())
            .set("schema-name", self.schema.clone())
            .set("table-name", self.table.clone())
            .set("slot.name", self.slot.clone())
            .set("decoding.plugin.name", "pgoutput")
            .set("scan.incremental.snapshot.enabled", "true")
            .set(
                "scan.startup.mode",
                match self.startup {
                    CdcStartup::Initial => "initial",
                    CdcStartup::LatestOffset => "latest-offset",
                    CdcStartup::Snapshot => "snapshot",
                },
            )
            .set("debezium.publication.name", self.publication_name())
            .set("debezium.publication.autocreate.mode", "disabled")
            .set("debezium.database.sslmode", self.sslmode.clone())
            .extend(&self.extra);
        Ok(TableDdl {
            name: vec![self.name.clone()],
            temporary: true,
            columns: self.columns.clone(),
            primary_key: self.primary_key.clone(),
            watermark: self.watermark.clone(),
            options: o,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> PostgresCdcTable {
        PostgresCdcTable {
            name: "orders".into(),
            host: "compute1".into(),
            port: 5432,
            database: "shop".into(),
            schema: "public".into(),
            table: "orders".into(),
            username: "flink_cdc".into(),
            password: Secret::reference("pg-cdc"),
            columns: vec![Column::new("id", "BIGINT").not_null()],
            primary_key: vec!["id".into()],
            watermark: None,
            slot: slot_name("eng_01jbx3k9q2w7r5t8y6u4i0p3a", 1),
            publication: None,
            startup: CdcStartup::Initial,
            sslmode: "require".into(),
            extra: BTreeMap::new(),
        }
    }

    #[test]
    fn slot_names_follow_r12_and_are_checked() {
        let mut t = table();
        assert_eq!(t.slot, "loams_fk_01jbx3k9q2w7r5t8y6u4i0p3a_1");
        assert_eq!(t.publication_name(), t.slot);
        assert!(t.ddl().is_ok());
        t.slot = "Bad-Slot".into();
        assert!(t.ddl().is_err());
        let mut t = table();
        t.publication = Some("x;y".into());
        assert!(t.ddl().is_err());
    }

    #[test]
    fn needs_primary_key_and_libpq_sslmode() {
        let mut t = table();
        t.primary_key.clear();
        assert!(t.ddl().is_err());
        let mut t = table();
        t.sslmode = "required".into();
        assert!(t.ddl().is_err());
    }
}
