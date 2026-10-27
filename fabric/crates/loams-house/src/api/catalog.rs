//! What `loams.house.v1`'s browsing RPCs read (HS1 Task 8): databases, tables,
//! the query log and pipes.
//!
//! Their sources are other tasks' — the catalog (Task 10, [`LakeCatalog`]), the
//! query log (Task 23) and pipes (Task 16) — so the front reads them through
//! [`ApiCatalog`]. A House with no catalog answers from [`FixtureCatalog`]: its
//! namespace's `default` database, with no tables, or, in tests, the fixtures a
//! test fills in. A source that does not exist yet answers `None`, and
//! the RPC answers `unimplemented` (reason `not_implemented`) naming the task that
//! serves it, so a client can tell "nothing yet" from "empty".

use std::collections::BTreeMap;

use loams_house_proto::loams::house::v1 as pb;

use crate::catalog::{Lake, LakeTable};
use crate::ddl::{ChTable, TableClass};
use crate::errors::{ChError, HouseError};

/// The read side of the House's metadata, per namespace.
#[async_trait::async_trait]
pub trait ApiCatalog: Send + Sync + std::fmt::Debug {
    /// The namespace's databases, by name.
    async fn databases(&self, namespace: u64) -> Result<Vec<pb::Database>, HouseError>;
    /// A database's tables, by name; `81` when there is no such database.
    async fn tables(&self, namespace: u64, database: &str) -> Result<Vec<pb::Table>, HouseError>;
    /// One table; `81` or `60` when it does not exist.
    async fn describe(
        &self,
        namespace: u64,
        database: &str,
        table: &str,
    ) -> Result<pb::DescribeTableResponse, HouseError>;
    /// The namespace's query log, newest first, or `None` until it exists.
    fn query_log(&self, namespace: u64) -> Option<Vec<pb::QueryRecord>>;
    /// The namespace's pipes, by name, or `None` where there are none to have
    /// (a House without a catalog).
    async fn pipes(&self, namespace: u64) -> Result<Option<Vec<pb::Pipe>>, HouseError>;
}

/// The task that serves the query log (§49 §15).
pub const QUERY_LOG_TASK: &str = "HS1 Task 23";

/// The catalog until Tasks 10, 16 and 23: every namespace has `default`, and
/// whatever a test adds.
#[derive(Debug, Default, Clone)]
pub struct FixtureCatalog {
    /// Tables and their descriptions, by namespace, database and name.
    tables: BTreeMap<(u64, String, String), pb::DescribeTableResponse>,
    /// Extra databases, by namespace and name (`default` always exists).
    databases: BTreeMap<(u64, String), String>,
    query_log: BTreeMap<u64, Vec<pb::QueryRecord>>,
    pipes: BTreeMap<u64, Vec<pb::Pipe>>,
}

impl FixtureCatalog {
    /// What a House with no catalog has: `default`, empty.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Adds a database (with a comment) to a namespace.
    pub fn with_database(mut self, namespace: u64, name: &str, comment: &str) -> Self {
        self.databases
            .insert((namespace, name.to_string()), comment.to_string());
        self
    }

    /// Adds a table; its database is the one its `table` entry names.
    pub fn with_table(mut self, namespace: u64, described: pb::DescribeTableResponse) -> Self {
        let table = described.table.as_option().cloned().unwrap_or_default();
        if table.database != "default" {
            self.databases
                .entry((namespace, table.database.clone()))
                .or_default();
        }
        self.tables
            .insert((namespace, table.database, table.name), described);
        self
    }

    /// Gives a namespace a query log (newest first).
    pub fn with_query_log(mut self, namespace: u64, log: Vec<pb::QueryRecord>) -> Self {
        self.query_log.insert(namespace, log);
        self
    }

    /// Gives a namespace pipes.
    pub fn with_pipes(mut self, namespace: u64, pipes: Vec<pb::Pipe>) -> Self {
        self.pipes.insert(namespace, pipes);
        self
    }

    fn has_database(&self, namespace: u64, database: &str) -> bool {
        database == "default"
            || self
                .databases
                .contains_key(&(namespace, database.to_string()))
    }
}

fn unknown_database(database: &str) -> HouseError {
    HouseError::from(ChError::unknown_database(format!(
        "Database {database} does not exist"
    )))
}

#[async_trait::async_trait]
impl ApiCatalog for FixtureCatalog {
    async fn databases(&self, namespace: u64) -> Result<Vec<pb::Database>, HouseError> {
        let mut names: BTreeMap<String, String> = self
            .databases
            .iter()
            .filter(|((ns, _), _)| *ns == namespace)
            .map(|((_, name), comment)| (name.clone(), comment.clone()))
            .collect();
        names.entry("default".to_string()).or_default();
        Ok(names
            .into_iter()
            .map(|(name, comment)| pb::Database {
                table_count: self
                    .tables
                    .keys()
                    .filter(|(ns, db, _)| *ns == namespace && *db == name)
                    .count() as u64,
                name,
                comment,
                ..Default::default()
            })
            .collect())
    }

    async fn tables(&self, namespace: u64, database: &str) -> Result<Vec<pb::Table>, HouseError> {
        if !self.has_database(namespace, database) {
            return Err(unknown_database(database));
        }
        Ok(self
            .tables
            .iter()
            .filter(|((ns, db, _), _)| *ns == namespace && db == database)
            .filter_map(|(_, described)| described.table.as_option().cloned())
            .collect())
    }

    async fn describe(
        &self,
        namespace: u64,
        database: &str,
        table: &str,
    ) -> Result<pb::DescribeTableResponse, HouseError> {
        if !self.has_database(namespace, database) {
            return Err(unknown_database(database));
        }
        self.tables
            .get(&(namespace, database.to_string(), table.to_string()))
            .cloned()
            .ok_or_else(|| {
                HouseError::from(ChError::unknown_table(format!(
                    "Table {database}.{table} does not exist"
                )))
            })
    }

    fn query_log(&self, namespace: u64) -> Option<Vec<pb::QueryRecord>> {
        self.query_log.get(&namespace).cloned()
    }

    async fn pipes(&self, namespace: u64) -> Result<Option<Vec<pb::Pipe>>, HouseError> {
        Ok(self.pipes.get(&namespace).cloned())
    }
}

/// The browsing RPCs over the catalog (HS1 Task 10): databases and tables from
/// the [`Lake`]; the query log (Task 23) and pipes (Task 16) stay their tasks'.
#[derive(Debug)]
pub struct LakeCatalog(pub std::sync::Arc<Lake>);

/// How many snapshots `DescribeTable` lists, newest first.
pub const DESCRIBED_SNAPSHOTS: usize = 20;

/// A time in Unix milliseconds as a `Timestamp`.
pub(crate) fn timestamp(ms: i64) -> loams_house_proto::google::protobuf::Timestamp {
    loams_house_proto::google::protobuf::Timestamp {
        seconds: ms.div_euclid(1000),
        nanos: (ms.rem_euclid(1000) * 1_000_000) as i32,
        ..Default::default()
    }
}

fn summary_number(table: &LakeTable, key: &str) -> u64 {
    table
        .metadata
        .current_snapshot()
        .and_then(|s| s.summary().additional_properties.get(key).cloned())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn api_table(database: &str, name: &str, table: &LakeTable) -> pb::Table {
    let ch = ChTable::from_lake(database, name, table);
    let class = match ch.class {
        TableClass::Lake => pb::TableClass::TABLE_CLASS_LAKE,
        TableClass::Realtime => pb::TableClass::TABLE_CLASS_REALTIME,
        TableClass::External => pb::TableClass::TABLE_CLASS_EXTERNAL,
        TableClass::Pipe => pb::TableClass::TABLE_CLASS_PIPE,
        // A catalog view (HE1 R9.13) has no class of its own in loams.house.v1 yet.
        TableClass::View => pb::TableClass::TABLE_CLASS_UNSPECIFIED,
    };
    let modified = table
        .metadata
        .current_snapshot()
        .map(|s| s.timestamp_ms())
        .unwrap_or_else(|| table.metadata.last_updated_ms());
    pb::Table {
        database: database.to_string(),
        name: name.to_string(),
        table_class: class.into(),
        engine: ch.engine,
        total_rows: summary_number(table, "total-records"),
        total_bytes: summary_number(table, "total-files-size"),
        partition_by: ch.partition_by,
        order_by: ch.order_by,
        last_modified: buffa::MessageField::some(timestamp(modified)),
        comment: ch.comment,
        ..Default::default()
    }
}

#[async_trait::async_trait]
impl ApiCatalog for LakeCatalog {
    async fn databases(&self, namespace: u64) -> Result<Vec<pb::Database>, HouseError> {
        let mut out = Vec::new();
        for database in self.0.databases(namespace).await? {
            let table_count = self.0.table_names(namespace, &database.name).await?.len() as u64;
            out.push(pb::Database {
                name: database.name,
                table_count,
                comment: database.comment,
                ..Default::default()
            });
        }
        Ok(out)
    }

    async fn tables(&self, namespace: u64, database: &str) -> Result<Vec<pb::Table>, HouseError> {
        self.0.require_database(namespace, database).await?;
        let mut out = Vec::new();
        for name in self.0.table_names(namespace, database).await? {
            if let Some(table) = self.0.load(namespace, database, &name).await? {
                out.push(api_table(database, &name, &table));
            }
        }
        Ok(out)
    }

    async fn describe(
        &self,
        namespace: u64,
        database: &str,
        table: &str,
    ) -> Result<pb::DescribeTableResponse, HouseError> {
        self.0.require_database(namespace, database).await?;
        let loaded = self
            .0
            .load(namespace, database, table)
            .await?
            .ok_or_else(|| {
                HouseError::from(ChError::unknown_table(format!(
                    "Table {database}.{table} does not exist"
                )))
            })?;
        let ch = ChTable::from_lake(database, table, &loaded);
        let mut snapshots: Vec<_> = loaded.metadata.snapshots().collect();
        snapshots.sort_by_key(|s| std::cmp::Reverse(s.timestamp_ms()));
        Ok(pb::DescribeTableResponse {
            table: buffa::MessageField::some(api_table(database, table, &loaded)),
            columns: ch
                .columns
                .iter()
                .map(|c| pb::ColumnDescription {
                    name: c.name.clone(),
                    clickhouse_type: c.ch_type.clone(),
                    default_kind: c.default_kind.clone(),
                    default_expression: c.default_expression.clone(),
                    comment: c.comment.clone(),
                    ..Default::default()
                })
                .collect(),
            create_table_query: ch.ddl.clone(),
            snapshots: snapshots
                .into_iter()
                .take(DESCRIBED_SNAPSHOTS)
                .map(|s| pb::TableSnapshot {
                    id: s.snapshot_id(),
                    committed_at: buffa::MessageField::some(timestamp(s.timestamp_ms())),
                    operation: s.summary().operation.as_str().to_string(),
                    added_rows: s
                        .summary()
                        .additional_properties
                        .get("added-records")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    fn query_log(&self, _namespace: u64) -> Option<Vec<pb::QueryRecord>> {
        None
    }

    /// One entry per materialized view over a pipe (HS1 Task 16), named
    /// `<database>.<view>`, with this front's view of its runs.
    async fn pipes(&self, namespace: u64) -> Result<Option<Vec<pb::Pipe>>, HouseError> {
        let rows = crate::pipes::rows(&self.0, namespace).await?;
        Ok(Some(
            rows.into_iter()
                .filter(|row| row.over_pipe)
                .map(|row| pb::Pipe {
                    name: format!("{}.{}", row.database, row.name),
                    source: row.source,
                    target: row.target,
                    lag_records: row.state.lag_records,
                    last_commit_at: match row.state.last_commit_ms {
                        Some(ms) => buffa::MessageField::some(timestamp(ms)),
                        None => buffa::MessageField::none(),
                    },
                    last_error: row.state.last_error,
                    ..Default::default()
                })
                .collect(),
        ))
    }
}
