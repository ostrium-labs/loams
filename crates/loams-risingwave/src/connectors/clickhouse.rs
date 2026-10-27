//! Sinks into a ClickHouse-compatible endpoint through RisingWave's
//! ClickHouse sink. **Not a default path** (design 81 R10).
//!
//! The House speaks ClickHouse's HTTP interface on port 8123. RisingWave's
//! `clickhouse` sink inserts over that interface in batches and, for
//! `type = 'upsert'`, needs a `ReplacingMergeTree`-family target (which the
//! House maps to a Fluss primary-key table, D344). The House authorizes a
//! Loams principal (its `--principals` list).
//!
//! The House reads engine tables through Iceberg: engines write Iceberg via
//! Lakekeeper and the House reads them as lake tables (R10). No engine joins
//! the House's `house-link` network, so this sink has no default address: a
//! deployment that wants it points `url` at loams-gateway's HTTP route to
//! the House, with the instance's own scoped credential (R5). It is kept for
//! that later route and for users' own ClickHouse servers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    Credential, Ddl, DdlError, ObjectName, Options, RenderContext, SinkFrom, SinkMode, sink_head,
};

/// The House, and the principal the sink acts as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HouseEndpoint {
    pub url: String,
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<Credential>,
    #[serde(default = "default_database")]
    pub database: String,
}

fn default_database() -> String {
    "default".to_owned()
}

impl HouseEndpoint {
    /// An endpoint (`http://host:8123` or the gateway's HTTP route), a
    /// user and its password.
    pub fn new(
        url: impl Into<String>,
        user: impl Into<String>,
        password: Option<Credential>,
    ) -> Self {
        Self {
            url: url.into(),
            user: user.into(),
            password,
            database: default_database(),
        }
    }

    pub fn with_database(mut self, database: impl Into<String>) -> Self {
        self.database = database.into();
        self
    }
}

/// `CREATE SINK … WITH (connector = 'clickhouse', …);`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HouseSink {
    pub name: ObjectName,
    #[serde(default)]
    pub if_not_exists: bool,
    pub from: SinkFrom,
    pub endpoint: HouseEndpoint,
    pub table: String,
    pub mode: SinkMode,
    /// Commit every N checkpoints (batches inserts) [default: RisingWave's].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_checkpoint_interval: Option<u32>,
    /// For an upsert into a `ReplacingMergeTree(ver, is_deleted)` table: the
    /// `is_deleted` column RisingWave sets on deletes
    /// (`clickhouse.delete.column`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_column: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

impl HouseSink {
    pub fn new(
        name: impl Into<ObjectName>,
        from: SinkFrom,
        endpoint: HouseEndpoint,
        table: impl Into<String>,
        mode: SinkMode,
    ) -> Self {
        Self {
            name: name.into(),
            if_not_exists: false,
            from,
            endpoint,
            table: table.into(),
            mode,
            commit_checkpoint_interval: None,
            delete_column: None,
            extra: BTreeMap::new(),
        }
    }

    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    pub fn commit_every(mut self, checkpoints: u32) -> Self {
        self.commit_checkpoint_interval = Some(checkpoints);
        self
    }

    pub fn delete_column(mut self, column: impl Into<String>) -> Self {
        self.delete_column = Some(column.into());
        self
    }

    pub fn option(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra.insert(key.into(), value.into());
        self
    }
}

impl Ddl for HouseSink {
    fn render(&self, ctx: &RenderContext<'_>) -> Result<String, DdlError> {
        if self.endpoint.url.is_empty() {
            return Err(DdlError::Missing("House url"));
        }
        if self.endpoint.user.is_empty() {
            return Err(DdlError::Missing("House user"));
        }
        if self.table.is_empty() {
            return Err(DdlError::Missing("table"));
        }
        if self.delete_column.is_some() && !matches!(self.mode, SinkMode::Upsert { .. }) {
            return Err(DdlError::Invalid(
                "delete_column is for upsert sinks".to_owned(),
            ));
        }
        if self.commit_checkpoint_interval == Some(0) {
            return Err(DdlError::Invalid(
                "commit_checkpoint_interval must be at least 1".to_owned(),
            ));
        }
        let mut options = Options::new();
        options.set("connector", "clickhouse");
        self.mode.apply(&mut options)?;
        options.set("clickhouse.url", self.endpoint.url.clone());
        options.set("clickhouse.user", self.endpoint.user.clone());
        if let Some(password) = &self.endpoint.password {
            options.set_credential("clickhouse.password", password);
        }
        options.set("clickhouse.database", self.endpoint.database.clone());
        options.set("clickhouse.table", self.table.clone());
        options.set_opt("clickhouse.delete.column", self.delete_column.clone());
        options.set_opt(
            "commit_checkpoint_interval",
            self.commit_checkpoint_interval.map(|n| n.to_string()),
        );
        options.extend(&self.extra);
        Ok(format!(
            "{}\n{};",
            sink_head(&self.name, self.if_not_exists, &self.from)?,
            options.render_with(ctx)?
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::testing::ctx;

    #[test]
    fn append_only_sink_into_the_house() {
        let sink = HouseSink::new(
            "clicks_house",
            SinkFrom::relation("clicks_per_minute"),
            HouseEndpoint::new(
                "https://house.engine-gateway.internal:8443",
                "eng_01jbzq7m3k8x2v5n9c4t6w0r1y",
                Some(Credential::env("RW_HOUSE_PASSWORD")),
            )
            .with_database("web"),
            "clicks_per_minute",
            SinkMode::ForceAppendOnly,
        )
        .commit_every(5);
        assert_eq!(
            sink.render(&ctx()).unwrap(),
            r#"CREATE SINK "clicks_house" FROM "clicks_per_minute"
WITH (
    connector = 'clickhouse',
    type = 'append-only',
    force_append_only = 'true',
    clickhouse.url = 'https://house.engine-gateway.internal:8443',
    clickhouse.user = 'eng_01jbzq7m3k8x2v5n9c4t6w0r1y',
    clickhouse.password = '${RW_HOUSE_PASSWORD}',
    clickhouse.database = 'web',
    clickhouse.table = 'clicks_per_minute',
    commit_checkpoint_interval = '5'
);"#
        );
    }

    #[test]
    fn upsert_sink_with_a_delete_column() {
        let sink = HouseSink::new(
            "users_house",
            SinkFrom::relation("users"),
            HouseEndpoint::new("http://ch:8123", "svc", Some(Credential::env("K"))),
            "users",
            SinkMode::upsert(["id"]),
        )
        .delete_column("is_deleted");
        let sql = sink.render(&ctx()).unwrap();
        assert!(
            sql.contains("    type = 'upsert',\n    primary_key = 'id',\n"),
            "{sql}"
        );
        assert!(
            sql.contains("clickhouse.delete.column = 'is_deleted'"),
            "{sql}"
        );
        assert!(!sql.contains("commit_checkpoint_interval"), "{sql}");
    }

    #[test]
    fn house_checks() {
        let base = || {
            HouseSink::new(
                "s",
                SinkFrom::relation("m"),
                HouseEndpoint::new("http://ch:8123", "p", None),
                "t",
                SinkMode::AppendOnly,
            )
        };
        assert!(base().delete_column("d").render(&ctx()).is_err());
        assert!(base().commit_every(0).render(&ctx()).is_err());
        let mut no_user = base();
        no_user.endpoint.user.clear();
        assert!(no_user.render(&ctx()).is_err());
    }
}
