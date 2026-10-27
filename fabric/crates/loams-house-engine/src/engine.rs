//! [`Engine`]: plans a statement with `loams-house-sql` and runs it with
//! DataFusion over the engine's catalog.

use std::sync::Arc;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::prelude::{SessionConfig, SessionContext};
use loams_house_sql::{Registry, prepare};
use loams_house_types::ChError;

use crate::catalog::MemCatalog;

/// A statement's answer.
#[derive(Debug)]
pub struct Answer {
    /// The output columns, with ClickHouse types where the plan kept them.
    pub schema: SchemaRef,
    /// The rows.
    pub batches: Vec<RecordBatch>,
    /// The statement's `FORMAT`, if it named one.
    pub format: Option<String>,
}

/// The Rust engine.
#[derive(Debug)]
pub struct Engine {
    /// The tables.
    pub catalog: Arc<MemCatalog>,
    registry: Registry,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// An engine with an empty catalog.
    pub fn new() -> Self {
        Self {
            catalog: Arc::new(MemCatalog::default()),
            registry: crate::registry::registry(),
        }
    }

    /// Runs one `SELECT` with `params` (`param_<name>` values) and request
    /// `settings`, `database` being the current database.
    ///
    /// # Errors
    ///
    /// ClickHouse's error for what the engine refuses or cannot run.
    pub async fn query(
        &self,
        sql: &str,
        params: &[(String, String)],
        settings: &[(String, String)],
        database: &str,
    ) -> Result<Answer, ChError> {
        let prepared = prepare(sql, params, self.catalog.as_ref(), &self.registry, database)?;
        // No setting is honoured yet (plan HE1 Task 37, D1805): refused, never
        // ignored.
        if let Some((name, _)) = settings.iter().chain(&prepared.settings).next() {
            return Err(ChError::not_implemented(format!(
                "The setting {name} is not supported by the Loams House engine yet (plan HE1 Task 37)"
            )));
        }
        let ctx = SessionContext::new_with_config(SessionConfig::new().with_target_partitions(1));
        let frame = ctx
            .execute_logical_plan(prepared.plan)
            .await
            .map_err(loams_house_sql::plan::ch_error)?;
        let schema: SchemaRef = Arc::new(frame.schema().as_arrow().clone());
        let batches = frame
            .collect()
            .await
            .map_err(loams_house_sql::plan::ch_error)?;
        Ok(Answer {
            schema,
            batches,
            format: prepared.format,
        })
    }
}
