//! [`Engine`]: plans a statement with `loams-house-sql` and runs it with
//! DataFusion over the engine's catalog.

use std::sync::Arc;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::execution::SessionStateBuilder;
use datafusion::prelude::SessionConfig;
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
        // DataFusion's defaults with ClickHouse's type rules in the analyzer
        // (plan HE1 Task 24). The plan is analyzed once; the answer's schema
        // is the analyzed plan's, whose fields carry the ClickHouse types
        // (R24.1), and the optimized plan's batches are relabelled with it.
        let state = SessionStateBuilder::new()
            .with_config(SessionConfig::new().with_target_partitions(1))
            .with_default_features()
            .with_analyzer_rules(loams_house_sql::coerce::analyzer_rules())
            .build();
        let err = loams_house_sql::plan::ch_error;
        let analyzed = state
            .analyzer()
            .execute_and_check(prepared.plan, state.config_options(), |_, _| {})
            .map_err(err)?;
        let schema: SchemaRef = Arc::new(analyzed.schema().as_arrow().clone());
        let optimized = state
            .optimizer()
            .optimize(analyzed, &state, |_, _| {})
            .map_err(err)?;
        let physical = state
            .query_planner()
            .create_physical_plan(&optimized, &state)
            .await
            .map_err(err)?;
        let batches = datafusion::physical_plan::collect(physical, state.task_ctx())
            .await
            .map_err(err)?
            .into_iter()
            .map(|b| RecordBatch::try_new(schema.clone(), b.columns().to_vec()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ChError::logical_error(e.to_string()))?;
        Ok(Answer {
            schema,
            batches,
            format: prepared.format,
        })
    }
}
