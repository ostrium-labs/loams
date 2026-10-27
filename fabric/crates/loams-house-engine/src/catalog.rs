//! In-memory, pinned tables used by the development oracle.
use datafusion::{
    datasource::{MemTable, provider_as_source},
    logical_expr::TableSource,
};
use loams_house_sql::Catalog;
use std::{collections::BTreeMap, sync::Arc};

/// A snapshot of the oracle's tables. Replaced as a unit between corpora.
#[derive(Debug, Default)]
pub struct MemoryCatalog {
    tables: BTreeMap<(String, String), Arc<MemTable>>,
}
impl MemoryCatalog {
    /// Adds a table to the snapshot.
    pub fn insert(&mut self, database: String, name: String, table: MemTable) {
        self.tables.insert((database, name), Arc::new(table));
    }
}
impl Catalog for MemoryCatalog {
    fn table(&self, database: &str, name: &str) -> Option<Arc<dyn TableSource>> {
        self.tables
            .get(&(database.into(), name.into()))
            .cloned()
            .map(|t| provider_as_source(t))
    }
}
