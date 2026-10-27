//! SQL over collections (plan M1.2 Task 10): a DataFusion catalog per
//! namespace ([`NamespaceCatalog`] → `collections` → [`CollectionProvider`]),
//! the search table functions with Spice's names and argument order
//! (Ruling 23, D56), read-only statement execution and the JSON form of
//! results.

mod catalog;
mod exprs;
pub mod graph_algos;
pub mod graph_ddl;
pub mod graph_udtf;
mod json_rows;
mod params;
mod provider;
mod udtf;

use std::fmt;
use std::sync::Arc;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::error::DataFusionError;
use datafusion::execution::SendableRecordBatchStream;
use datafusion::execution::context::SQLOptions;
use datafusion::logical_expr::ScalarUDF;
use datafusion::prelude::{DataFrame, SessionConfig, SessionContext};
use futures::StreamExt;
use loams_common::NamespaceId;
use loams_common::meta::Collection;

pub use catalog::{CollectionsSchema, NamespaceCatalog};
pub use exprs::expr_to_query;
pub use graph_ddl::{CreateGraph, GraphDdlError, is_create_graph, parse_create_graph};
pub use json_rows::{rows_to_json, value_to_json};
pub use params::{ParamsError, plan_read_only_with_params};
pub use provider::{CollectionProvider, CollectionScanExec, collection_arrow_schema};
pub use udtf::{
    HybridSearchFunction, RerankFunction, RetrieverDescriptorUdf, RrfFunction, TextSearchFunction,
    VectorSearchFunction,
};

use crate::error::ServiceError;
use crate::hot::RequestHot;
use crate::ir::ReadConsistency;
use crate::read::ReadView;
use crate::service::{CollectionService, SqlConfig};

/// The one schema of every namespace catalog.
pub const COLLECTIONS_SCHEMA: &str = "collections";

/// The search table functions (Ruling 23, D56).
pub const SQL_SEARCH_FUNCTIONS: [&str; 5] = [
    "vector_search",
    "text_search",
    "hybrid_search",
    "rrf",
    "rerank",
];

/// A search table function's default `k`: Spice's `text_search` default.
pub const SQL_DEFAULT_K: usize = 1_000;

/// DDL, DML and statements disallowed (Global Constraints).
pub fn read_only_options() -> SQLOptions {
    SQLOptions::new()
        .with_allow_ddl(false)
        .with_allow_dml(false)
        .with_allow_statements(false)
}

/// The rows of one statement, at most `SqlConfig.max_rows`.
#[derive(Clone, Debug)]
pub struct SqlResult {
    pub schema: SchemaRef,
    pub batches: Vec<RecordBatch>,
    /// More rows existed than were returned.
    pub truncated: bool,
}

/// What every table and table function of one context reads with: the
/// service, the namespace, the context's consistency and the hot scope
/// captured when the context was made.
#[derive(Clone)]
pub(crate) struct SqlScope {
    pub(crate) service: Arc<CollectionService>,
    pub(crate) ns: String,
    pub(crate) consistency: ReadConsistency,
    pub(crate) hot: RequestHot,
    /// Who the statement is for: the graph and algorithm functions check it (GR1 R24.12).
    pub(crate) caller: crate::graph::GraphCaller,
}

impl fmt::Debug for SqlScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SqlScope")
            .field("ns", &self.ns)
            .field("consistency", &self.consistency)
            .field("hot", &self.hot.enabled)
            .finish_non_exhaustive()
    }
}

impl SqlScope {
    /// The view of `collection`, resolved now.
    pub(crate) async fn view(
        &self,
        ns_id: NamespaceId,
        collection: &Collection,
    ) -> Result<ReadView, ServiceError> {
        self.service
            .view_for(ns_id, collection, &self.consistency, &self.hot)
            .await
    }
}

/// Rule 1: the read-only context of namespace `ns`.
pub(crate) fn context(
    service: Arc<CollectionService>,
    ns: &str,
    consistency: ReadConsistency,
    caller: crate::graph::GraphCaller,
) -> SessionContext {
    let hot = service.request_hot();
    let scope = SqlScope {
        service,
        ns: ns.to_string(),
        consistency,
        hot,
        caller,
    };
    let ctx = SessionContext::new_with_config(
        SessionConfig::new()
            .with_default_catalog_and_schema(ns, COLLECTIONS_SCHEMA)
            .with_information_schema(true),
    );
    ctx.register_catalog(ns, Arc::new(NamespaceCatalog::new(scope.clone())));
    ctx.register_udtf(
        "vector_search",
        Arc::new(VectorSearchFunction::new(scope.clone())),
    );
    ctx.register_udtf(
        "text_search",
        Arc::new(TextSearchFunction::new(scope.clone())),
    );
    ctx.register_udtf(
        "hybrid_search",
        Arc::new(HybridSearchFunction::new(scope.clone())),
    );
    ctx.register_udtf("rrf", Arc::new(RrfFunction::new(scope.clone())));
    ctx.register_udtf("rerank", Arc::new(RerankFunction));
    // The graph table functions (GR1 Task 21), answered by Loams Graph when the server runs it.
    for function in [
        graph_udtf::GraphFunction::Expand,
        graph_udtf::GraphFunction::Neighbors,
        graph_udtf::GraphFunction::Degree,
        graph_udtf::GraphFunction::ShortestPath,
    ] {
        ctx.register_udtf(
            function.name(),
            Arc::new(graph_udtf::GraphTableFunction::new(scope.clone(), function)),
        );
    }
    // The graph algorithms (GR1 Task 22), on Loams Graph's adjacency index.
    for function in graph_algos::AlgorithmFunction::ALL {
        ctx.register_udtf(
            function.name(),
            Arc::new(graph_algos::AlgorithmTableFunction::new(
                scope.clone(),
                function,
            )),
        );
    }
    for name in udtf::DESCRIPTOR_FUNCTIONS {
        ctx.register_udf(ScalarUDF::new_from_impl(RetrieverDescriptorUdf::new(name)));
    }
    ctx
}

/// The service error an error carries, if it carries one.
fn carried(err: &DataFusionError) -> Option<ServiceError> {
    if let DataFusionError::External(inner) = err.find_root() {
        return inner.downcast_ref::<ServiceError>().cloned();
    }
    None
}

/// A planning error: the service error it carries, else `InvalidArgument`.
pub fn planning(err: DataFusionError) -> ServiceError {
    carried(&err).unwrap_or_else(|| ServiceError::InvalidArgument(format!("sql: {err}")))
}

/// An execution error: the service error it carries; DataFusion's own
/// errors of the statement (arithmetic, casts, plans) are the caller's.
pub(crate) fn execution(err: DataFusionError) -> ServiceError {
    if let Some(service) = carried(&err) {
        return service;
    }
    match err.find_root() {
        DataFusionError::ResourcesExhausted(_) | DataFusionError::Internal(_) => {
            ServiceError::Internal(format!("sql: {err}"))
        }
        _ => ServiceError::InvalidArgument(format!("sql: {err}")),
    }
}

/// Rule 6: plans `sql` read-only in `ctx` and collects at most
/// `config.max_rows` rows within `config.timeout`.
pub async fn run_read_only(
    ctx: &SessionContext,
    sql: &str,
    config: &SqlConfig,
) -> Result<SqlResult, ServiceError> {
    run_bounded(config, plan_read_only(ctx, sql)).await
}

/// Rule 6 for a caller that plans its own frame (a wire protocol's prepared
/// statements): awaits `frame`, then collects at most `config.max_rows` rows,
/// the whole of it within `config.timeout`.
pub async fn run_bounded<F>(config: &SqlConfig, frame: F) -> Result<SqlResult, ServiceError>
where
    F: Future<Output = Result<DataFrame, ServiceError>>,
{
    let run = async {
        let frame = frame.await?;
        collect_bounded(frame, config.max_rows).await
    };
    tokio::time::timeout(config.timeout, run)
        .await
        .map_err(|_| ServiceError::Timeout)?
}

/// Plans `sql` read-only in `ctx` (rules 1 and 6) without running it.
///
/// The search table functions plan synchronously against the catalog cache,
/// so the namespaces of the context are refreshed first: a collection
/// created just before the statement is visible to it. Every served surface
/// (REST, Flight SQL) plans through here.
pub async fn plan_read_only(ctx: &SessionContext, sql: &str) -> Result<DataFrame, ServiceError> {
    refresh_catalogs(ctx).await?;
    // `sql_with_options`, split so a refused plan is told apart.
    let plan = ctx
        .state()
        .create_logical_plan(sql)
        .await
        .map_err(planning)?;
    verify_read_only(&plan)?;
    ctx.execute_logical_plan(plan).await.map_err(planning)
}

/// Refreshes the namespaces of `ctx`, so a collection created just before a
/// statement is visible to it.
async fn refresh_catalogs(ctx: &SessionContext) -> Result<(), ServiceError> {
    for name in ctx.catalog_names() {
        if let Some(catalog) = ctx.catalog(&name)
            && let Some(catalog) = catalog.downcast_ref::<NamespaceCatalog>()
        {
            catalog.refresh().await?;
        }
    }
    Ok(())
}

/// Rule 1: only queries plan.
fn verify_read_only(plan: &datafusion::logical_expr::LogicalPlan) -> Result<(), ServiceError> {
    read_only_options().verify_plan(plan).map_err(|err| {
        ServiceError::InvalidArgument(format!("only read-only queries are allowed: {err}"))
    })
}

/// Starts executing a frame [`plan_read_only`] planned, as a stream.
pub async fn execute_read_only(
    frame: DataFrame,
) -> Result<SendableRecordBatchStream, ServiceError> {
    frame.execute_stream().await.map_err(planning)
}

async fn collect_bounded(frame: DataFrame, max_rows: usize) -> Result<SqlResult, ServiceError> {
    let mut stream = execute_read_only(frame).await?;
    let schema = stream.schema();
    let mut batches = Vec::new();
    let mut rows = 0;
    let mut truncated = false;
    while let Some(batch) = stream.next().await {
        let batch = batch.map_err(execution)?;
        if batch.num_rows() == 0 {
            continue;
        }
        if rows == max_rows {
            truncated = true;
            break;
        }
        let take = batch.num_rows().min(max_rows - rows);
        if take < batch.num_rows() {
            truncated = true;
        }
        rows += take;
        batches.push(batch.slice(0, take));
        if truncated {
            break;
        }
    }
    Ok(SqlResult {
        schema,
        batches,
        truncated,
    })
}

/// A result as one Arrow IPC stream: its schema, then its batches
/// (`loams.sql.v1`'s `QueryArrow`, API2 Task 1). `max_bytes` caps the stream;
/// past it the answer is `None`, which the caller turns into
/// `result_too_large`.
pub fn arrow_ipc(result: &SqlResult, max_bytes: usize) -> Result<Option<Vec<u8>>, ServiceError> {
    use datafusion::arrow::ipc::writer::StreamWriter;
    let internal = |err: datafusion::arrow::error::ArrowError| {
        ServiceError::Internal(format!("arrow ipc: {err}"))
    };
    let mut writer = StreamWriter::try_new(Vec::new(), &result.schema).map_err(internal)?;
    for batch in &result.batches {
        writer.write(batch).map_err(internal)?;
        if writer.get_ref().len() > max_bytes {
            return Ok(None);
        }
    }
    writer.finish().map_err(internal)?;
    let bytes = writer.into_inner().map_err(internal)?;
    Ok((bytes.len() <= max_bytes).then_some(bytes))
}
