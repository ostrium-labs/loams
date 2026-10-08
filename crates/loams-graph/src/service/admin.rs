//! `GraphAdminService` over the graph catalog, and the data plane's entry point that opens a
//! catalog graph lazily (GR1 Task 4).
//!
//! The catalog ([`GraphCatalog`]) is the source of truth for which graphs exist; the engine holds
//! the ones that are open. A graph is opened on its first statement (or `GetSchema`), from
//! `<data_dir>/graphs/<id>/`, so a restarted server serves every graph its catalog lists without
//! opening any of them up front.

use std::sync::Arc;
use std::time::Duration;

use buffa_types::google::protobuf::Timestamp;
use connectrpc::{ConnectError, ErrorCode};
use loams_proto::loams::graph::v1 as pb;
use loams_proto::loams::operations::v1 as ops;

use super::errors::{map_catalog, map_engine, refuse};
use super::{data, engine_info};
use crate::catalog::{
    CatalogError, CatalogState, GraphCatalog, GraphLimits, GraphMeta, GraphMode, NewGraph,
};
use crate::engine::{Engine, Graph, GraphState, OpenSpec};

/// How long a deleted graph's storage is kept before it is purged, by default.
pub const DEFAULT_RETENTION_HOLD: Duration = Duration::from_secs(24 * 3600);

/// The fields `UpdateGraph`'s mask may name.
const UPDATABLE: [&str; 3] = ["languages", "limits", "replicas"];

/// `GraphAdminService`, and the data plane over catalog graphs.
#[derive(Debug, Clone)]
pub struct GraphAdmin {
    engine: Arc<Engine>,
    catalog: GraphCatalog,
    retention_hold: Duration,
}

fn language_name(language: pb::QueryLanguage) -> &'static str {
    use pb::QueryLanguage as L;
    match language {
        L::Unspecified | L::Gql => "GQL",
        L::Cypher => "CYPHER",
        L::SqlPgq => "SQL_PGQ",
        L::Gremlin => "GREMLIN",
        L::Graphql => "GRAPHQL",
        L::Sparql => "SPARQL",
    }
}

fn language_value(name: &str) -> pb::QueryLanguage {
    use pb::QueryLanguage as L;
    match name {
        "CYPHER" => L::Cypher,
        "SQL_PGQ" => L::SqlPgq,
        "GREMLIN" => L::Gremlin,
        "GRAPHQL" => L::Graphql,
        "SPARQL" => L::Sparql,
        _ => L::Gql,
    }
}

/// The languages of a request, checked against this build and named.
fn languages(
    requested: &[buffa::EnumValue<pb::QueryLanguage>],
) -> Result<Vec<String>, ConnectError> {
    let mut out = Vec::new();
    for language in requested {
        data::check_language(language.as_known(), language)?;
        let name = language.as_known().map_or("GQL", language_name).to_string();
        if !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

fn limits_from(limits: Option<&pb::GraphLimits>) -> GraphLimits {
    limits.map_or_else(GraphLimits::default, |l| GraphLimits {
        timeout_ms: l.timeout_ms,
        max_rows: l.max_rows,
        max_result_bytes: l.max_result_bytes,
        memory_bytes: l.memory_bytes,
        max_path_hops: l.max_path_hops,
    })
}

fn limits_to(limits: &GraphLimits) -> pb::GraphLimits {
    pb::GraphLimits {
        timeout_ms: limits.timeout_ms,
        max_rows: limits.max_rows,
        max_result_bytes: limits.max_result_bytes,
        memory_bytes: limits.memory_bytes,
        max_path_hops: limits.max_path_hops,
        ..Default::default()
    }
}

/// A short operation id: `op-` and 26 hex characters (D146).
fn operation_id() -> String {
    let hex = format!("{:032x}", u128::from(ulid::Ulid::generate()));
    format!("op-{}", &hex[..26])
}

impl GraphAdmin {
    /// Admin over `engine` and `catalog`, with the default retention hold.
    #[must_use]
    pub fn new(engine: Arc<Engine>, catalog: GraphCatalog) -> Self {
        Self {
            engine,
            catalog,
            retention_hold: DEFAULT_RETENTION_HOLD,
        }
    }

    /// The same admin with another retention hold.
    #[must_use]
    pub fn with_retention_hold(mut self, hold: Duration) -> Self {
        self.retention_hold = hold;
        self
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// The catalog.
    #[must_use]
    pub fn catalog(&self) -> &GraphCatalog {
        &self.catalog
    }

    /// Fills a `Graph` message: the catalog record, with the engine's state and counts when the
    /// graph is open.
    fn message(&self, meta: &GraphMeta) -> pb::Graph {
        let open = self
            .open_graph(&meta.namespace, &meta.name)
            .filter(|g| g.id().is_some_and(|id| id.to_string() == meta.id));
        let state = match (meta.state, open.as_ref().map(|g| g.state())) {
            (CatalogState::Deleting { .. }, _) => pb::GraphState::Deleting,
            (_, Some(GraphState::Poisoned)) => pb::GraphState::Reloading,
            (_, Some(GraphState::Failed)) => pb::GraphState::Failed,
            (_, Some(GraphState::Ready)) => pb::GraphState::Ready,
            // Listed and not open: it is served, and opens on its next statement. (EVICTED is
            // Task 14's, for a graph closed to free memory.)
            (_, None) => pb::GraphState::Ready,
        };
        let stats = open.as_ref().map(|g| {
            let (nodes, edges) = g.counts();
            pb::GraphStats {
                nodes,
                edges,
                ..Default::default()
            }
        });
        pb::Graph {
            namespace: meta.namespace.clone(),
            name: meta.name.clone(),
            id: meta.id.clone(),
            mode: match meta.mode {
                GraphMode::Owned => pb::GraphMode::Owned,
                GraphMode::Linked => pb::GraphMode::Linked,
            }
            .into(),
            languages: meta
                .languages
                .iter()
                .map(|l| language_value(l).into())
                .collect(),
            limits: limits_to(&meta.limits).into(),
            replicas: meta.replicas,
            state: state.into(),
            stats: stats.into(),
            create_time: Timestamp {
                seconds: i64::try_from(meta.created_at_ms / 1000).unwrap_or(i64::MAX),
                nanos: i32::try_from((meta.created_at_ms % 1000) * 1_000_000).unwrap_or(0),
                ..Default::default()
            }
            .into(),
            version: meta.version,
            ..Default::default()
        }
    }

    /// The engine's open graph under a name, if any.
    fn open_graph(&self, namespace: &str, name: &str) -> Option<Arc<Graph>> {
        self.engine
            .list(Some(namespace))
            .ok()?
            .into_iter()
            .find(|g| g.name() == name)
    }

    /// Opens a catalog graph for a statement: from its own storage, lazily, and never a stale
    /// graph of the same name that a delete could not close.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND`/`graph_not_found` when the catalog has no such graph, or the engine's error.
    pub async fn open(&self, namespace: &str, name: &str) -> Result<Arc<Graph>, ConnectError> {
        let meta = self
            .catalog
            .get_by_name(namespace, name)
            .await
            .map_err(map_catalog)?;
        let id = meta.graph_id().map_err(map_catalog)?;
        for _ in 0..2 {
            let graph = Graph::open_or_existing(&self.engine, namespace, name, || {
                OpenSpec::for_catalog(&self.engine, id)
            })
            .map_err(map_engine)?;
            if graph.id() == Some(id) {
                return self.engine.reopen_if_poisoned(graph).map_err(map_engine);
            }
            // An older graph of this name is still open: close it and open this one.
            drop(graph);
            self.engine.close(namespace, name).map_err(|err| {
                refuse(ErrorCode::Unavailable, "graph_reloading", err.to_string())
            })?;
        }
        Err(refuse(
            ErrorCode::Unavailable,
            "graph_reloading",
            format!("graph {namespace}/{name} is being replaced; retry"),
        ))
    }

    /// `GetEngineInfo`.
    pub async fn get_engine_info(&self, _req: pb::GetEngineInfoRequest) -> pb::EngineInfo {
        engine_info(&self.engine)
    }

    /// `CreateGraph`: idempotent by `idempotency_key`; the name must be free.
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT` for a bad name, `UNIMPLEMENTED` for LINKED or a language this build
    /// lacks, `ALREADY_EXISTS`, or `UNAVAILABLE` when the catalog is.
    pub async fn create_graph(
        &self,
        req: pb::CreateGraphRequest,
    ) -> Result<pb::Graph, ConnectError> {
        crate::engine::validate_names(&req.namespace, &req.name).map_err(map_engine)?;
        if req.mode.as_known() == Some(pb::GraphMode::Linked) {
            return Err(refuse(
                ErrorCode::Unimplemented,
                "not_implemented",
                "LINKED graphs are not served yet (GR1 Task 17)",
            ));
        }
        let new = NewGraph {
            mode: GraphMode::Owned,
            languages: languages(&req.languages)?,
            limits: limits_from(req.limits.as_option()),
            replicas: req.replicas,
            idempotency_key: (!req.idempotency_key.is_empty()).then(|| req.idempotency_key.clone()),
        };
        let meta = self
            .catalog
            .create(&req.namespace, &req.name, new)
            .await
            .map_err(map_catalog)?;
        Ok(self.message(&meta))
    }

    /// `GetGraph`.
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT`, `NOT_FOUND` or `UNAVAILABLE`.
    pub async fn get_graph(&self, req: pb::GetGraphRequest) -> Result<pb::Graph, ConnectError> {
        let meta = self
            .catalog
            .get_by_name(&req.namespace, &req.name)
            .await
            .map_err(map_catalog)?;
        Ok(self.message(&meta))
    }

    /// `ListGraphs` (AIP-158).
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT` for a bad namespace, size or token, or `UNAVAILABLE`.
    pub async fn list_graphs(
        &self,
        req: pb::ListGraphsRequest,
    ) -> Result<pb::ListGraphsResponse, ConnectError> {
        let page = self
            .catalog
            .list(&req.namespace, req.page_size, &req.page_token)
            .await
            .map_err(map_catalog)?;
        Ok(pb::ListGraphsResponse {
            graphs: page.graphs.iter().map(|g| self.message(g)).collect(),
            next_page_token: page.next_page_token.unwrap_or_default(),
            ..Default::default()
        })
    }

    /// `UpdateGraph`: the fields `update_mask` names (`languages`, `limits`, `replicas`; all
    /// three when the mask is empty), at `expected_version` when given.
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT` for another field or a bad value, `NOT_FOUND`, `ABORTED` with
    /// `graph_catalog_version_mismatch`, or `UNAVAILABLE`.
    pub async fn update_graph(
        &self,
        req: pb::UpdateGraphRequest,
    ) -> Result<pb::Graph, ConnectError> {
        let graph = req.graph.as_option().cloned().unwrap_or_default();
        let mut paths: Vec<String> = req
            .update_mask
            .as_option()
            .map(|m| m.paths.clone())
            .unwrap_or_default();
        if paths.is_empty() {
            paths = UPDATABLE.iter().map(ToString::to_string).collect();
        }
        if let Some(bad) = paths.iter().find(|p| !UPDATABLE.contains(&p.as_str())) {
            return Err(refuse(
                ErrorCode::InvalidArgument,
                "invalid_argument",
                format!(
                    "update_mask path {bad:?} is not updatable; only languages, limits and replicas are"
                ),
            ));
        }
        let new_languages = languages(&graph.languages)?;
        let new_limits = limits_from(graph.limits.as_option());
        let meta = self
            .catalog
            .update(
                &graph.namespace,
                &graph.name,
                req.expected_version,
                |meta| {
                    for path in &paths {
                        match path.as_str() {
                            "languages" => {
                                meta.languages = if new_languages.is_empty() {
                                    vec!["GQL".to_string()]
                                } else {
                                    new_languages.clone()
                                };
                            }
                            "limits" => meta.limits = new_limits,
                            "replicas" => meta.replicas = graph.replicas,
                            _ => return Err(CatalogError::Invalid(format!("path {path}"))),
                        }
                    }
                    Ok(())
                },
            )
            .await
            .map_err(map_catalog)?;
        Ok(self.message(&meta))
    }

    /// `DeleteGraph`: the graph leaves the catalog's listing and its engine is closed at once;
    /// its storage is purged by [`GraphAdmin::purge_expired`] after the retention hold. Answers
    /// the finished operation. A retry with the same `idempotency_key` answers it again.
    ///
    /// # Errors
    ///
    /// `INVALID_ARGUMENT`, `NOT_FOUND` or `UNAVAILABLE`.
    pub async fn delete_graph(
        &self,
        req: pb::DeleteGraphRequest,
    ) -> Result<ops::Operation, ConnectError> {
        let key = (!req.idempotency_key.is_empty()).then(|| req.idempotency_key.clone());
        let meta = self
            .catalog
            .mark_deleting(&req.namespace, &req.name, key)
            .await
            .map_err(map_catalog)?;
        if let Some(open) = self.open_graph(&req.namespace, &req.name)
            && open.id().is_some_and(|id| id.to_string() == meta.id)
        {
            drop(open);
            // A holder still mid-statement keeps it open; the catalog no longer serves it, and
            // `open` replaces it if the name is created again.
            if let Err(err) = self.engine.close(&req.namespace, &req.name) {
                tracing::warn!(namespace = %req.namespace, name = %req.name, error = %err, "a deleted graph is still in use; it closes when released");
            }
        }
        let mut operation = ops::Operation {
            id: operation_id(),
            kind: "graph.delete".to_string(),
            namespace: req.namespace.clone(),
            state: ops::OperationState::Succeeded.into(),
            ..Default::default()
        };
        operation.target.insert("graph".to_string(), req.name);
        operation.target.insert("graph_id".to_string(), meta.id);
        Ok(operation)
    }

    /// `GetSchema`: labels, edge types, property keys, counts and indexes. Opens the graph.
    ///
    /// # Errors
    ///
    /// `NOT_FOUND`, or the engine's error.
    pub async fn get_schema(
        &self,
        req: pb::GetSchemaRequest,
    ) -> Result<pb::GraphSchema, ConnectError> {
        let graph = self.open(&req.namespace, &req.name).await?;
        let summary = graph.schema().map_err(map_engine)?;
        Ok(pb::GraphSchema {
            labels: summary
                .labels
                .into_iter()
                .map(|(label, count)| pb::LabelInfo {
                    label,
                    count,
                    ..Default::default()
                })
                .collect(),
            edge_types: summary
                .edge_types
                .into_iter()
                .map(|(r#type, count)| pb::EdgeTypeInfo {
                    r#type,
                    count,
                    ..Default::default()
                })
                .collect(),
            property_keys: summary.property_keys,
            indexes: summary
                .indexes
                .into_iter()
                .map(|(name, target, kind)| pb::IndexInfo {
                    name,
                    target,
                    kind,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    /// `Execute` on a catalog graph, opening it lazily.
    ///
    /// # Errors
    ///
    /// As [`data::execute`], and `NOT_FOUND` for a graph the catalog does not list.
    pub async fn execute(
        &self,
        req: pb::ExecuteRequest,
    ) -> Result<pb::ExecuteResponse, ConnectError> {
        self.open(&req.namespace, &req.graph).await?;
        data::execute(&self.engine, req)
    }

    /// `ExecuteBatch` on a catalog graph, opening it lazily.
    ///
    /// # Errors
    ///
    /// As [`data::execute_batch`].
    pub async fn execute_batch(
        &self,
        req: pb::ExecuteBatchRequest,
    ) -> Result<pb::ExecuteBatchResponse, ConnectError> {
        self.open(&req.namespace, &req.graph).await?;
        data::execute_batch(&self.engine, req)
    }

    /// `Explain` on a catalog graph.
    ///
    /// # Errors
    ///
    /// As [`data::explain`].
    pub async fn explain(&self, req: pb::ExplainRequest) -> Result<pb::Plan, ConnectError> {
        self.open(&req.namespace, &req.graph).await?;
        data::explain(&self.engine, req)
    }

    /// Purges every deleted graph whose retention hold (`hold`, or the admin's own when `None`)
    /// has passed: its engine is closed, its storage directory removed and its catalog record
    /// dropped. Answers how many were purged. Meant for a periodic sweep (Task 5 wires it).
    ///
    /// # Errors
    ///
    /// `UNAVAILABLE` when the catalog is.
    pub async fn purge_expired(&self, hold: Duration) -> Result<usize, ConnectError> {
        let now = self.catalog.now_ms();
        let before = now.saturating_sub(u64::try_from(hold.as_millis()).unwrap_or(u64::MAX));
        let mut purged = 0;
        for namespace in self.catalog.namespaces().await.map_err(map_catalog)? {
            for meta in self
                .catalog
                .deleting(&namespace, before)
                .await
                .map_err(map_catalog)?
            {
                if let Some(open) = self.open_graph(&namespace, &meta.name)
                    && open.id().is_some_and(|id| id.to_string() == meta.id)
                {
                    drop(open);
                    if self.engine.close(&namespace, &meta.name).is_err() {
                        // Still in use: purge on a later sweep.
                        continue;
                    }
                }
                if let (Some(data_dir), Ok(id)) = (self.engine.data_dir(), meta.graph_id()) {
                    let dir = data_dir.join("graphs").join(id.to_string());
                    if let Err(err) = std::fs::remove_dir_all(&dir)
                        && err.kind() != std::io::ErrorKind::NotFound
                    {
                        tracing::warn!(dir = %dir.display(), error = %err, "could not purge a deleted graph's storage");
                        continue;
                    }
                }
                if self
                    .catalog
                    .purge(&namespace, &meta.id)
                    .await
                    .map_err(map_catalog)?
                {
                    purged += 1;
                }
            }
        }
        Ok(purged)
    }

    /// Deletes catalog documents older than `grace` that no reader still needs (the catalog's
    /// [`GraphCatalog::sweep_documents`]); run with [`crate::catalog::DOCUMENT_GRACE`] beside
    /// [`GraphAdmin::purge_expired`].
    ///
    /// # Errors
    ///
    /// `UNAVAILABLE` when the catalog is.
    pub async fn sweep_documents(&self, grace: Duration) -> Result<usize, ConnectError> {
        self.catalog
            .sweep_documents(grace)
            .await
            .map_err(map_catalog)
    }

    /// The retention hold this admin purges with.
    #[must_use]
    pub fn retention_hold(&self) -> Duration {
        self.retention_hold
    }

    /// Closes every open graph, releasing its storage (for a shutdown or a test restart).
    pub async fn shutdown(&self) {
        for graph in self.engine.list(None).unwrap_or_default() {
            let (namespace, name) = (graph.namespace().to_string(), graph.name().to_string());
            drop(graph);
            if let Err(err) = self.engine.close(&namespace, &name) {
                tracing::warn!(%namespace, %name, error = %err, "closing a graph at shutdown failed");
            }
        }
    }
}
