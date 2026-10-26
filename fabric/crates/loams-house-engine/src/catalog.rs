//! The engine's in-memory catalog: tables seeded from the oracle (plan HE1
//! Task 23). Task 39 adds the House's pinned lake tables beside it.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use datafusion::datasource::{MemTable, provider_as_source};
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
        self.get(database, name).map(|t| provider_as_source(t.data))
    }
}
