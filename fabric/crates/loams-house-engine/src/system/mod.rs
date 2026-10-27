//! The `system` tables, metadata statements and safe table functions of the
//! Rust engine (plan HE1 Task 38).
//!
//! * [`SystemTables`]: `system.tables`, `columns`, `databases`, `parts`,
//!   `settings`, `functions`, `clusters`, `macros`, `mutations`,
//!   `distributed_ddl_queue`, `query_log`, `one`, `numbers` (and
//!   `numbers_mt`, `zeros`, `zeros_mt`) as in-memory tables with ClickHouse
//!   26.9's columns ([`shapes`]). A table the front sent ([`inline`], Task
//!   39's `TablePin::Inline`) is served as sent; otherwise the dev server
//!   fills it from its catalog ([`fill`]).
//! * [`meta`]: `DESCRIBE`, `SHOW CREATE` and `EXISTS`.
//! * [`numbers`]: `numbers`, `numbers_mt`, `zeros`, `zeros_mt`.
//! * [`functions`]: `currentDatabase()`, `timezone()`, `serverTimezone()`.
//! * [`settings`]: `system.settings` from Task 37's settings table.

pub mod build;
pub mod fill;
pub mod functions;
pub mod inline;
pub mod meta;
pub mod numbers;
pub mod settings;
pub mod shapes;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use datafusion::catalog::TableProvider;
use datafusion::common::ScalarValue;
use datafusion::datasource::provider_as_source;
use datafusion::logical_expr::TableSource;
use loams_house_sql::Registry;
use loams_house_types::ChError;

pub use inline::InlineTable;
pub use settings::SettingRow;

use crate::catalog::MemCatalog;
use build::Rows;

/// The `system` tables one statement sees, built when it names them.
#[derive(Debug)]
pub struct SystemTables<'a> {
    catalog: &'a MemCatalog,
    registry: &'a Registry,
    settings: Vec<SettingRow>,
    inline: HashMap<String, InlineTable>,
    built: Mutex<HashMap<String, Arc<dyn TableProvider>>>,
}

impl<'a> SystemTables<'a> {
    /// The `system` tables over `catalog`, listing `registry`'s functions and
    /// `settings`; the tables in `inline` are served as the front sent them.
    pub fn new(
        catalog: &'a MemCatalog,
        registry: &'a Registry,
        settings: Vec<SettingRow>,
        inline: HashMap<String, InlineTable>,
    ) -> Self {
        Self {
            catalog,
            registry,
            settings,
            inline,
            built: Mutex::new(HashMap::new()),
        }
    }

    /// `system.<name>`; `None` when the engine does not serve it.
    ///
    /// # Errors
    ///
    /// A table that does not build (a programming error).
    pub fn table(&self, name: &str) -> Option<Result<Arc<dyn TableProvider>, ChError>> {
        if let Some(found) = self.built.lock().ok().and_then(|b| b.get(name).cloned()) {
            return Some(Ok(found));
        }
        let made = self.make(name)?;
        if let Ok(provider) = &made
            && let Ok(mut b) = self.built.lock()
        {
            b.insert(name.to_string(), provider.clone());
        }
        Some(made)
    }

    fn make(&self, name: &str) -> Option<Result<Arc<dyn TableProvider>, ChError>> {
        if let Some(t) = self.inline.get(name) {
            return Some(t.provider().map(|p| p as Arc<dyn TableProvider>));
        }
        if let Some(p) = numbers::system_table(name) {
            return Some(Ok(p));
        }
        let shape = fill::shape(name)?;
        let rows = match name {
            "tables" => fill::tables(self.catalog),
            "columns" => fill::columns(self.catalog),
            "databases" => fill::databases(self.catalog),
            "parts" => fill::parts(self.catalog),
            "functions" => fill::functions(self.registry),
            "settings" => settings::fill(&self.settings),
            "clusters" => fill::clusters(self.catalog),
            // A one-node House has no macros, its mutations are done when
            // their statement returns, no DDL is queued (R9.62), and the dev
            // server keeps no query log: no rows.
            _ => Rows::default(),
        };
        Some(build::table(shape, &rows).map(|t| t as Arc<dyn TableProvider>))
    }
}

/// The catalog one statement plans against: the engine's tables, its
/// `system` tables and its table functions.
#[derive(Debug)]
pub struct QueryCatalog<'a> {
    tables: &'a MemCatalog,
    system: SystemTables<'a>,
    error: Mutex<Option<ChError>>,
}

impl<'a> QueryCatalog<'a> {
    /// `tables` with `system`.
    pub fn new(tables: &'a MemCatalog, system: SystemTables<'a>) -> Self {
        Self {
            tables,
            system,
            error: Mutex::new(None),
        }
    }

    /// The error of a `system` table that failed to build, if one did (the
    /// planner only sees that the table is missing).
    pub fn take_error(&self) -> Option<ChError> {
        self.error.lock().ok().and_then(|mut e| e.take())
    }
}

impl loams_house_sql::Catalog for QueryCatalog<'_> {
    fn table(&self, database: &str, name: &str) -> Option<Arc<dyn TableSource>> {
        if database == "system" {
            return match self.system.table(name)? {
                Ok(p) => Some(provider_as_source(p)),
                Err(e) => {
                    if let Ok(mut slot) = self.error.lock() {
                        *slot = Some(e);
                    }
                    None
                }
            };
        }
        loams_house_sql::Catalog::table(self.tables, database, name)
    }

    fn has_database(&self, database: &str) -> bool {
        self.tables.has_database(database)
    }

    fn table_function(
        &self,
        name: &str,
        args: &[ScalarValue],
    ) -> Option<Result<Arc<dyn TableSource>, ChError>> {
        loams_house_sql::Catalog::table_function(self.tables, name, args)
    }
}
