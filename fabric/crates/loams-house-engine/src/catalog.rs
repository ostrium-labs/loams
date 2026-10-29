//! The engine's in-memory catalog: tables seeded from the oracle (plan HE1
//! Task 23). Task 39 adds the House's pinned lake tables beside it.
//!
//! Beside each table's rows it keeps what `system.tables`, `system.columns`,
//! `DESCRIBE` and `SHOW CREATE` print about it ([`TableInfo`], plan HE1 Task
//! 38), and the databases that exist.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

use datafusion::common::ScalarValue;
use datafusion::datasource::{MemTable, TableProvider, provider_as_source};
use datafusion::logical_expr::TableSource;
use loams_house_types::ChError;

/// A table and what the House knows about it besides its rows.
#[derive(Debug, Clone)]
pub struct Table {
    /// The rows, with each field's ClickHouse type in its metadata.
    pub data: Arc<MemTable>,
    /// Its ClickHouse engine (`MergeTree`, `ReplacingMergeTree`, …), for
    /// `FINAL` (Task 27).
    pub engine: String,
    /// Its sorting key, as `system.tables.sorting_key` prints it.
    pub sorting_key: String,
}

/// A column as `system.columns` and `DESCRIBE` print it (plan HE1 Task 38).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColumnInfo {
    /// Its name.
    pub name: String,
    /// Its ClickHouse type, as `toTypeName` prints it.
    pub ty: String,
    /// `DEFAULT`, `MATERIALIZED`, `ALIAS`, `EPHEMERAL` or empty.
    pub default_kind: String,
    /// The default's expression.
    pub default_expression: String,
    /// The column's comment.
    pub comment: String,
    /// The codec as `DESCRIBE` prints it (`ZSTD(1)`), empty for none.
    pub codec: String,
    /// The column's `TTL` expression.
    pub ttl: String,
    /// Whether the partition key names it.
    pub is_in_partition_key: bool,
    /// Whether the sorting key names it.
    pub is_in_sorting_key: bool,
    /// Whether the primary key names it.
    pub is_in_primary_key: bool,
    /// Whether the sampling key names it.
    pub is_in_sampling_key: bool,
}

/// What `system.tables` prints about a table beyond its engine and sorting
/// key (plan HE1 Task 38). Empty fields print empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableInfo {
    /// Its UUID, hyphenated; empty for the zero UUID.
    pub uuid: String,
    /// The engine clause (`MergeTree PARTITION BY … ORDER BY …`).
    pub engine_full: String,
    /// The partition key.
    pub partition_key: String,
    /// The primary key.
    pub primary_key: String,
    /// The sampling key.
    pub sampling_key: String,
    /// The one-line `CREATE` statement `system.tables` shows.
    pub create_table_query: String,
    /// The formatted `CREATE` statement `SHOW CREATE TABLE` prints (the
    /// House's stored `loams.ch.ddl`); `create_table_query` when empty.
    pub show_create: String,
    /// A view's `SELECT`.
    pub as_select: String,
    /// The table's comment.
    pub comment: String,
    /// Every column, `ALIAS` and `MATERIALIZED` ones included, in order;
    /// empty when only the rows' schema is known.
    pub columns: Vec<ColumnInfo>,
}

/// Tables by `(database, name)`.
#[derive(Default)]
pub struct MemCatalog {
    tables: RwLock<HashMap<(String, String), Table>>,
    /// Lake tables read through their pins (plan HE1 Task 26). A lake table
    /// shadows a seeded one of the same name.
    lake: RwLock<HashMap<(String, String), Arc<dyn TableProvider>>>,
    info: RwLock<HashMap<(String, String), TableInfo>>,
    databases: RwLock<BTreeSet<String>>,
}

impl std::fmt::Debug for MemCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<_> = self
            .tables
            .read()
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default();
        f.debug_struct("MemCatalog")
            .field("tables", &names)
            .finish()
    }
}

/// The databases a ClickHouse server always has, besides `default`.
pub const ENGINE_DATABASES: &[&str] = &["system", "information_schema", "INFORMATION_SCHEMA"];

impl MemCatalog {
    /// Adds or replaces `database.name` (and creates `database`).
    pub fn insert(&self, database: &str, name: &str, table: Table) {
        if let Ok(mut t) = self.tables.write() {
            t.insert((database.to_string(), name.to_string()), table);
        }
        self.add_database(database);
    }

    /// Records what `system.tables` and `DESCRIBE` print about
    /// `database.name`.
    pub fn set_info(&self, database: &str, name: &str, info: TableInfo) {
        if let Ok(mut i) = self.info.write() {
            i.insert((database.to_string(), name.to_string()), info);
        }
    }

    /// Creates `database` if it does not exist.
    pub fn add_database(&self, database: &str) {
        if let Ok(mut d) = self.databases.write() {
            d.insert(database.to_string());
        }
    }

    /// Adds or replaces the lake table `database.name`.
    pub fn insert_lake(&self, database: &str, name: &str, table: Arc<dyn TableProvider>) {
        if let Ok(mut t) = self.lake.write() {
            t.insert((database.to_string(), name.to_string()), table);
        }
    }

    /// The lake table `database.name`.
    pub fn lake(&self, database: &str, name: &str) -> Option<Arc<dyn TableProvider>> {
        self.lake
            .read()
            .ok()?
            .get(&(database.to_string(), name.to_string()))
            .cloned()
    }

    /// The table `database.name`.
    pub fn get(&self, database: &str, name: &str) -> Option<Table> {
        self.tables
            .read()
            .ok()?
            .get(&(database.to_string(), name.to_string()))
            .cloned()
    }

    /// What is recorded about `database.name` (the default when nothing is).
    pub fn info(&self, database: &str, name: &str) -> TableInfo {
        self.info
            .read()
            .ok()
            .and_then(|i| i.get(&(database.to_string(), name.to_string())).cloned())
            .unwrap_or_default()
    }

    /// Every table, sorted by database and name.
    pub fn list(&self) -> Vec<(String, String, Table)> {
        let mut out: Vec<_> = self
            .tables
            .read()
            .map(|t| {
                t.iter()
                    .map(|((d, n), table)| (d.clone(), n.clone(), table.clone()))
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        out
    }

    /// The user databases: `default`, every database a table is in, and every
    /// one created; sorted. The engine's own (`system`, …) are not listed.
    pub fn databases(&self) -> Vec<String> {
        let mut out: BTreeSet<String> =
            self.databases.read().map(|d| d.clone()).unwrap_or_default();
        out.insert("default".to_string());
        out.into_iter()
            .filter(|d| !ENGINE_DATABASES.contains(&d.as_str()))
            .collect()
    }

    /// Whether `database` exists, the engine's own included.
    pub fn has_database(&self, database: &str) -> bool {
        ENGINE_DATABASES.contains(&database) || self.databases().iter().any(|d| d == database)
    }
}

/// A table's row count and the bytes its batches hold in memory.
pub fn stats(table: &MemTable) -> (u64, u64) {
    let mut rows = 0u64;
    let mut bytes = 0u64;
    for partition in &table.batches {
        if let Ok(batches) = partition.try_read() {
            for b in batches.iter() {
                rows += b.num_rows() as u64;
                bytes += b.get_array_memory_size() as u64;
            }
        }
    }
    (rows, bytes)
}

impl loams_house_sql::Catalog for MemCatalog {
    fn table(&self, database: &str, name: &str) -> Option<Arc<dyn TableSource>> {
        if let Some(lake) = self.lake(database, name) {
            return Some(provider_as_source(lake));
        }
        if database == "system" {
            return crate::system::numbers::system_table(name).map(provider_as_source);
        }
        self.get(database, name).map(|t| provider_as_source(t.data))
    }

    fn has_database(&self, database: &str) -> bool {
        MemCatalog::has_database(self, database)
    }

    fn table_function(
        &self,
        name: &str,
        args: &[ScalarValue],
    ) -> Option<Result<Arc<dyn TableSource>, ChError>> {
        crate::system::numbers::table_function(name, args).map(|r| r.map(provider_as_source))
    }
}
