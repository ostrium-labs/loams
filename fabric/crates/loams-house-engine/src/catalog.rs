//! The engine's in-memory catalog: tables seeded from the oracle (plan HE1
//! Task 23). Task 39 adds the House's pinned lake tables beside it.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use datafusion::datasource::{MemTable, TableProvider, provider_as_source};
use datafusion::logical_expr::TableSource;

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

/// Tables by `(database, name)`.
#[derive(Default)]
pub struct MemCatalog {
    tables: RwLock<HashMap<(String, String), Table>>,
    /// Lake tables read through their pins (plan HE1 Task 26). A lake table
    /// shadows a seeded one of the same name.
    lake: RwLock<HashMap<(String, String), Arc<dyn TableProvider>>>,
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

impl MemCatalog {
    /// Adds or replaces `database.name`.
    pub fn insert(&self, database: &str, name: &str, table: Table) {
        if let Ok(mut t) = self.tables.write() {
            t.insert((database.to_string(), name.to_string()), table);
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
}

impl loams_house_sql::Catalog for MemCatalog {
    fn table(&self, database: &str, name: &str) -> Option<Arc<dyn TableSource>> {
        if let Some(lake) = self.lake(database, name) {
            return Some(provider_as_source(lake));
        }
        self.get(database, name).map(|t| provider_as_source(t.data))
    }
}
