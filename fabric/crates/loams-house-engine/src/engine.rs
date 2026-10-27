//! [`Engine`]: plans a statement with `loams-house-sql` and runs it with
//! DataFusion over the engine's catalog.

use std::sync::Arc;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::common::DataFusionError;
use datafusion::execution::SessionStateBuilder;
use loams_house_sql::{Registry, prepare};
use loams_house_types::ChError;

use crate::catalog::MemCatalog;
use crate::settings::{Settings, Shape, memory_limit_exceeded, merging_engine, timeout_exceeded};

/// A statement's answer.
#[derive(Debug)]
pub struct Answer {
    /// The output columns, with ClickHouse types where the plan kept them.
    pub schema: SchemaRef,
    /// The rows.
    pub batches: Vec<RecordBatch>,
    /// The statement's `FORMAT`, if it named one.
    pub format: Option<String>,
    /// The settings it ran with: the formats read [`Settings::text`].
    pub settings: Settings,
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

    /// The engine's functions (planning a lake table's catalog expressions).
    pub fn registry(&self) -> &Registry {
        &self.registry
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
        // The request's settings first, as ClickHouse checks them before it
        // parses the statement (D1805: refused, never ignored).
        Settings::resolve(settings)?;
        let prepared = prepare(sql, params, self.catalog.as_ref(), &self.registry, database)?;
        let applied = Settings::resolve(settings.iter().chain(&prepared.settings))?;
        let shape = Shape::of(
            sql,
            &prepared.plan,
            &prepared.resolution,
            !params.is_empty(),
            |name| self.merging(name, database),
        );
        applied.check(&shape)?;
        let memory = applied.max_memory_usage;
        // A DataFusion error as ClickHouse's; past `max_memory_usage`, 241.
        let err = move |e: DataFusionError| {
            let exhausted = match e.find_root() {
                DataFusionError::ResourcesExhausted(m) => Some(m.clone()),
                _ => None,
            };
            match exhausted {
                Some(m) => memory_limit_exceeded(&m, memory),
                None => loams_house_sql::plan::ch_error(e),
            }
        };
        let names = prepared.names;
        // DataFusion's defaults with ClickHouse's type rules in the analyzer
        // (plan HE1 Task 24), under the statement's settings (Task 37:
        // `max_threads`, `max_block_size`, `max_memory_usage`). The plan is
        // analyzed once; the answer's schema is the analyzed plan's, whose
        // fields carry the ClickHouse types (R24.1), and the optimized plan's
        // batches are relabelled with it.
        let state = SessionStateBuilder::new()
            .with_config(applied.config())
            .with_runtime_env(applied.runtime()?)
            .with_default_features()
            .with_analyzer_rules(loams_house_sql::coerce::analyzer_rules())
            .build();
        let plan = prepared.plan;
        let run = async {
            let analyzed = state
                .analyzer()
                .execute_and_check(plan, state.config_options(), |_, _| {})
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
            Ok::<_, ChError>((schema, batches))
        };
        let started = std::time::Instant::now();
        let (schema, batches) = match applied.max_execution_time {
            Some(limit) => tokio::time::timeout(limit, run)
                .await
                .map_err(|_| timeout_exceeded(started.elapsed(), limit))??,
            None => run.await?,
        };
        applied.check_result(&batches)?;
        let (schema, batches) = match names {
            Some(names) => named(schema, batches, &names)?,
            None => (schema, batches),
        };
        Ok(Answer {
            schema,
            batches,
            format: prepared.format,
            settings: applied,
        })
    }

    /// Whether the table `name` (`[db, t]` or `[t]` in `database`) is a
    /// merging engine, which `SETTINGS final = 1` would change (Task 27).
    fn merging(&self, name: &[String], database: &str) -> bool {
        let (db, table) = match name {
            [db, t] => (db.as_str(), t.as_str()),
            [t] => (database, t.as_str()),
            _ => return false,
        };
        self.catalog
            .get(db, table)
            .is_some_and(|t| merging_engine(&t.engine))
    }
}

/// The answer's columns under ClickHouse's names, which may repeat (plan HE1
/// Task 25: the plan names duplicates apart).
fn named(
    schema: SchemaRef,
    batches: Vec<RecordBatch>,
    names: &[String],
) -> Result<(SchemaRef, Vec<RecordBatch>), ChError> {
    if names.len() != schema.fields().len() {
        return Ok((schema, batches));
    }
    let fields: Vec<_> = schema
        .fields()
        .iter()
        .zip(names)
        .map(|(f, n)| f.as_ref().clone().with_name(n))
        .collect();
    let renamed = Arc::new(
        datafusion::arrow::datatypes::Schema::new(fields).with_metadata(schema.metadata().clone()),
    );
    let batches = batches
        .into_iter()
        .map(|b| RecordBatch::try_new(renamed.clone(), b.columns().to_vec()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ChError::logical_error(e.to_string()))?;
    Ok((renamed, batches))
}
