//! [`Engine`]: plans a statement with `loams-house-sql` and runs it with
//! DataFusion over the engine's catalog.

use std::sync::Arc;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::common::DataFusionError;
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
        let ctx = applied.session()?;
        let memory = applied.max_memory_usage;
        let exec_error = move |e: DataFusionError| {
            let exhausted = match e.find_root() {
                DataFusionError::ResourcesExhausted(m) => Some(m.clone()),
                _ => None,
            };
            match exhausted {
                Some(m) => memory_limit_exceeded(&m, memory),
                None => loams_house_sql::plan::ch_error(e),
            }
        };
        let plan = prepared.plan;
        let run = async {
            let frame = ctx.execute_logical_plan(plan).await.map_err(exec_error)?;
            let schema: SchemaRef = Arc::new(frame.schema().as_arrow().clone());
            let batches = frame.collect().await.map_err(exec_error)?;
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
