//! The app modules (SDKP §2.2–§2.5): `namespaces`, `collections`,
//! `documents`, `search`, `vector`, `graph`, `sql` and `streams`.
//!
//! One method per facade call of [`crate::facade::MODULES`] (the protos'
//! `loams.options.v1.facade` annotations), typed with the generated messages,
//! each running the same runtime as `instance` and `tables`: R11's default
//! namespace and R4's session token ([`crate::app::prepare_request`]), the
//! idempotency key of a keyed request (R3), the retry class and deadline from
//! the binding (R2, R12), and the response's tokens merged into the session.
//!
//! ```rust,ignore
//! let loams = Loams::from_env()?;
//! loams.namespaces().create(CreateNamespaceRequest { namespace: "app".into(), ..Default::default() }, CallOptionsOverrides::new()).await?;
//! let hits = loams.search().query(Search::on("notes").text("refund").build()?, CallOptionsOverrides::new()).await?;
//! ```

use connectrpc::client::ClientTransport;
use connectrpc::http_body::Body;
use futures::Stream;

use crate::app::{AppRequest, prepare_request, session_token_for};
use crate::call::CallOptionsOverrides;
use crate::client::{Runtime, send};
use crate::error::LoamsError;
use crate::facade::CallBinding;
use loams_proto::loams::collection::v1 as c;
use loams_proto::loams::graph::v1 as g;
use loams_proto::loams::operations::v1::Operation;
use loams_proto::loams::sql::v1 as s;
use loams_proto::loams::stream::v1 as st;

/// A request whose message declares `idempotency_key` (R3).
pub trait KeyedRequest {
    /// The request's key field.
    fn key_mut(&mut self) -> &mut String;
}

macro_rules! keyed {
    ($($t:path),* $(,)?) => {$(
        impl KeyedRequest for $t {
            fn key_mut(&mut self) -> &mut String {
                &mut self.idempotency_key
            }
        }
    )*};
}

keyed!(
    c::WriteDocumentsRequest,
    g::CreateGraphRequest,
    g::UpdateGraphRequest,
    g::DeleteGraphRequest,
    g::ExecuteRequest,
    g::ExecuteBatchRequest,
);

// Requests with no `namespace` of their own: R11 has nothing to fill.
impl AppRequest for c::ListNamespacesRequest {}
impl AppRequest for g::UpdateGraphRequest {}
impl AppRequest for g::ExecuteStreamRequest {}

fn binding(module: &str, call: &str) -> &'static CallBinding {
    crate::binding::binding_of(module, call).unwrap_or_else(|| {
        panic!("the facade has {module}.{call}: it is generated from the protos")
    })
}

/// R11 and R4 on a request, before its first attempt.
fn prepared<R: AppRequest>(
    runtime: &Runtime,
    binding: &'static CallBinding,
    mut request: R,
    options: &CallOptionsOverrides,
) -> Result<R, LoamsError> {
    let session = options
        .session
        .clone()
        .or_else(|| runtime.consistency.clone());
    let token = session_token_for(session.as_deref(), binding.package);
    prepare_request(
        &mut request,
        runtime.namespace.as_deref(),
        token.as_deref(),
        binding.rpc,
    )?;
    Ok(request)
}

/// R3: a key once per logical call, the caller's if it gave one.
fn keyed<R: KeyedRequest>(mut request: R, options: &CallOptionsOverrides) -> R {
    let key = crate::request::idempotency_key(
        Some(request.key_mut().as_str()),
        options.idempotency_key.as_deref(),
    );
    *request.key_mut() = key;
    request
}

/// One unary facade call: `$name` on `$module`, `$client.$method`.
macro_rules! unary {
    ($(#[$doc:meta])* $name:ident, $module:literal, $call:literal, $req:ty, $resp:ty, $method:ident) => {
        $(#[$doc])*
        ///
        /// # Errors
        ///
        /// The call's failure, as the typed [`LoamsError`].
        pub async fn $name(
            &self,
            request: $req,
            options: CallOptionsOverrides,
        ) -> Result<$resp, LoamsError> {
            let binding = binding($module, $call);
            let request = prepared(self.runtime, binding, request, &options)?;
            let client = &self.client;
            send(binding, request, false, options, self.runtime, |request, opts| {
                client.$method(request, opts)
            })
            .await
        }
    };
    ($(#[$doc:meta])* keyed $name:ident, $module:literal, $call:literal, $req:ty, $resp:ty, $method:ident) => {
        $(#[$doc])*
        ///
        /// Keyed (R3): the SDK mints one idempotency key per logical call and
        /// reuses it on every retry; `CallOptionsOverrides::idempotency_key`
        /// supplies your own.
        ///
        /// # Errors
        ///
        /// The call's failure, as the typed [`LoamsError`].
        pub async fn $name(
            &self,
            request: $req,
            options: CallOptionsOverrides,
        ) -> Result<$resp, LoamsError> {
            let binding = binding($module, $call);
            let request = keyed(prepared(self.runtime, binding, request, &options)?, &options);
            let client = &self.client;
            send(binding, request, true, options, self.runtime, |request, opts| {
                client.$method(request, opts)
            })
            .await
        }
    };
}

/// The module struct over one generated client.
macro_rules! module {
    ($(#[$doc:meta])* $name:ident, $client:ty) => {
        $(#[$doc])*
        pub struct $name<'a, T> {
            pub(crate) client: $client,
            pub(crate) runtime: &'a Runtime,
        }

        impl<T> std::fmt::Debug for $name<'_, T> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
}

module!(
    /// `loams.namespaces`: the containers every collection and stream live in.
    NamespacesModule,
    c::NamespaceServiceClient<T>
);
module!(
    /// `loams.collections`: collections, their schemas, aliases and versions.
    CollectionsModule,
    c::CollectionServiceClient<T>
);
module!(
    /// `loams.documents`: write, read, scroll, count, and the filter writes.
    DocumentsModule,
    c::DocumentServiceClient<T>
);
module!(
    /// `loams.search`: one search over one collection (build it with
    /// [`crate::builders::Search`]).
    SearchModule,
    c::QueryServiceClient<T>
);
module!(
    /// `loams.vector`: the kNN half of `search`, by its own name (Q613).
    VectorModule,
    c::QueryServiceClient<T>
);
module!(
    /// `loams.graph`: the graph catalog and GQL statements. Its package is
    /// `unstable`, so the module is experimental.
    GraphModule,
    GraphClients<T>
);

module!(
    /// `loams.sql`: one read-only SQL statement over a namespace's
    /// collections, with `$1…$n` parameters (SDKP §2.5).
    SqlModule,
    s::SqlServiceClient<T>
);

module!(
    /// `loams.streams`: a namespace's logs — produce and fetch records and
    /// CloudEvents (SDKP §2.5). `Loams::subscribe` is the R14 loop over `fetch`.
    StreamsModule,
    st::StreamServiceClient<T>
);

/// The two services behind `loams.graph`.
#[derive(Clone)]
pub struct GraphClients<T> {
    pub(crate) admin: g::GraphAdminServiceClient<T>,
    pub(crate) statements: g::GraphServiceClient<T>,
}

impl<T> std::fmt::Debug for GraphClients<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphClients").finish_non_exhaustive()
    }
}

impl<T> NamespacesModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    unary!(
        /// `NamespaceService/CreateNamespace`; `already_exists` on a repeat.
        create, "namespaces", "create", c::CreateNamespaceRequest, c::CreateNamespaceResponse, create_namespace_with_options
    );
    unary!(
        /// `NamespaceService/ListNamespaces`, one page.
        list, "namespaces", "list", c::ListNamespacesRequest, c::ListNamespacesResponse, list_namespaces_with_options
    );

    /// Every namespace, page after page (R6).
    pub fn list_all(
        &self,
        request: c::ListNamespacesRequest,
        options: CallOptionsOverrides,
    ) -> impl Stream<Item = Result<c::NamespaceInfo, LoamsError>> + '_ {
        pages(
            request,
            move |request| {
                let options = options.clone();
                async move {
                    let page = self.list(request, options).await?;
                    Ok((page.namespaces, page.next_page_token))
                }
            },
            |request, token| request.page_token = token,
        )
    }
}

impl<T> CollectionsModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    unary!(
        /// `CollectionService/CreateCollection`; `schema` from [`crate::builders::Schema::to_struct`].
        create, "collections", "create", c::CreateCollectionRequest, c::CollectionInfo, create_collection_with_options
    );
    unary!(
        /// `CollectionService/ListCollections`, one page.
        list, "collections", "list", c::ListCollectionsRequest, c::ListCollectionsResponse, list_collections_with_options
    );
    unary!(
        /// `CollectionService/GetCollection`, by name or alias.
        get, "collections", "get", c::GetCollectionRequest, c::CollectionInfo, get_collection_with_options
    );
    unary!(
        /// `CollectionService/DropCollection`.
        drop, "collections", "drop", c::DropCollectionRequest, c::DropCollectionResponse, drop_collection_with_options
    );
    unary!(
        /// `CollectionService/AddFields`, additive.
        add_fields, "collections", "add_fields", c::AddFieldsRequest, c::AddFieldsResponse, add_fields_with_options
    );
    unary!(
        /// `CollectionService/ListVersions`, one page.
        list_versions, "collections", "list_versions", c::ListVersionsRequest, c::ListVersionsResponse, list_versions_with_options
    );
    unary!(
        /// `CollectionService/Scan`: plans an external read and pins it.
        scan, "collections", "scan", c::ScanRequest, c::ScanPlan, scan_with_options
    );
    unary!(
        /// `CollectionService/UpdateAliases`, atomically.
        update_aliases, "collections", "update_aliases", c::UpdateAliasesRequest, c::UpdateAliasesResponse, update_aliases_with_options
    );
    unary!(
        /// `CollectionService/SetHot`.
        set_hot, "collections", "set_hot", c::SetHotRequest, c::SetHotResponse, set_hot_with_options
    );
    unary!(
        /// `CollectionService/WarmCollection`.
        warm, "collections", "warm", c::WarmCollectionRequest, c::WarmCollectionResponse, warm_collection_with_options
    );

    /// Every collection of the namespace, page after page (R6).
    pub fn list_all(
        &self,
        request: c::ListCollectionsRequest,
        options: CallOptionsOverrides,
    ) -> impl Stream<Item = Result<c::CollectionInfo, LoamsError>> + '_ {
        pages(
            request,
            move |request| {
                let options = options.clone();
                async move {
                    let page = self.list(request, options).await?;
                    Ok((page.collections, page.next_page_token))
                }
            },
            |request, token| request.page_token = token,
        )
    }
}

impl<T> DocumentsModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    unary!(
        /// `DocumentService/WriteDocuments`: raw ops. See
        /// [`crate::handle::CollectionHandle`] for upsert, delete, patch and
        /// `write_batch`.
        keyed write, "documents", "write", c::WriteDocumentsRequest, c::WriteDocumentsResponse, write_documents_with_options
    );
    unary!(
        /// `DocumentService/GetDocuments`, by id.
        get, "documents", "get", c::GetDocumentsRequest, c::GetDocumentsResponse, get_documents_with_options
    );
    unary!(
        /// `DocumentService/ScrollDocuments`, one page (`next` is the cursor).
        scroll, "documents", "scroll", c::ScrollDocumentsRequest, c::ScrollDocumentsResponse, scroll_documents_with_options
    );
    unary!(
        /// `DocumentService/CountDocuments`.
        count, "documents", "count", c::CountDocumentsRequest, c::CountDocumentsResponse, count_documents_with_options
    );
    unary!(
        /// `DocumentService/DeleteByFilter`. Not retried: resumable, not idempotent.
        delete_by_filter, "documents", "delete_by_filter", c::DeleteByFilterRequest, c::FilterWriteResponse, delete_by_filter_with_options
    );
    unary!(
        /// `DocumentService/PatchByFilter`. Not retried: resumable, not idempotent.
        patch_by_filter, "documents", "patch_by_filter", c::PatchByFilterRequest, c::FilterWriteResponse, patch_by_filter_with_options
    );
}

impl<T> SearchModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    unary!(
        /// `QueryService/Search`: text, vector, sparse or hybrid.
        query, "search", "query", c::SearchRequest, c::SearchResponse, search_with_options
    );
}

impl<T> VectorModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    unary!(
        /// `QueryService/Search`, as `vector.search`.
        search, "vector", "search", c::SearchRequest, c::SearchResponse, search_with_options
    );
}

macro_rules! graph_call {
    (keyed $name:ident, $call:literal, $req:ty, $resp:ty, $client:ident, $method:ident) => {
        #[doc = concat!("`loams.graph.", $call, "` (keyed, R3).")]
        ///
        /// # Errors
        ///
        /// The call's failure; `feature_not_in_variant` on a build without
        /// Loams Graph.
        pub async fn $name(
            &self,
            request: $req,
            options: CallOptionsOverrides,
        ) -> Result<$resp, LoamsError> {
            let binding = binding("graph", $call);
            let request = keyed(
                prepared(self.runtime, binding, request, &options)?,
                &options,
            );
            let client = &self.client.$client;
            send(
                binding,
                request,
                true,
                options,
                self.runtime,
                |request, opts| client.$method(request, opts),
            )
            .await
        }
    };
    ($name:ident, $call:literal, $req:ty, $resp:ty, $client:ident, $method:ident) => {
        #[doc = concat!("`loams.graph.", $call, "`.")]
        ///
        /// # Errors
        ///
        /// The call's failure; `feature_not_in_variant` on a build without
        /// Loams Graph.
        pub async fn $name(
            &self,
            request: $req,
            options: CallOptionsOverrides,
        ) -> Result<$resp, LoamsError> {
            let binding = binding("graph", $call);
            let request = prepared(self.runtime, binding, request, &options)?;
            let client = &self.client.$client;
            send(
                binding,
                request,
                false,
                options,
                self.runtime,
                |request, opts| client.$method(request, opts),
            )
            .await
        }
    };
}

impl<T> GraphModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    graph_call!(keyed create, "create", g::CreateGraphRequest, g::Graph, admin, create_graph_with_options);
    graph_call!(
        get,
        "get",
        g::GetGraphRequest,
        g::Graph,
        admin,
        get_graph_with_options
    );
    graph_call!(
        list,
        "list",
        g::ListGraphsRequest,
        g::ListGraphsResponse,
        admin,
        list_graphs_with_options
    );
    graph_call!(keyed update, "update", g::UpdateGraphRequest, g::Graph, admin, update_graph_with_options);
    graph_call!(keyed delete, "delete", g::DeleteGraphRequest, Operation, admin, delete_graph_with_options);
    graph_call!(
        schema,
        "schema",
        g::GetSchemaRequest,
        g::GraphSchema,
        admin,
        get_schema_with_options
    );
    graph_call!(keyed query, "query", g::ExecuteRequest, g::ExecuteResponse, statements, execute_with_options);
    graph_call!(keyed query_batch, "query_batch", g::ExecuteBatchRequest, g::ExecuteBatchResponse, statements, execute_batch_with_options);
    graph_call!(
        explain,
        "explain",
        g::ExplainRequest,
        g::Plan,
        statements,
        explain_with_options
    );
}

/// Every item of a paged call: `fetch(request)` answers `(items,
/// next_page_token)`, and `set_token` puts a token on the next request.
fn pages<'a, R, I, F, Fut>(
    first: R,
    fetch: F,
    set_token: fn(&mut R, String),
) -> impl Stream<Item = Result<I, LoamsError>> + 'a
where
    R: Clone + 'a,
    I: 'a,
    F: Fn(R) -> Fut + 'a,
    Fut: std::future::Future<Output = Result<(Vec<I>, String), LoamsError>> + 'a,
{
    futures::stream::unfold(
        (Some(first), std::collections::VecDeque::new(), fetch),
        move |(mut next, mut buffer, fetch)| async move {
            loop {
                if let Some(item) = buffer.pop_front() {
                    return Some((Ok(item), (next, buffer, fetch)));
                }
                let request = next.take()?;
                match fetch(request.clone()).await {
                    Ok((items, token)) => {
                        buffer.extend(items);
                        if !token.is_empty() {
                            let mut more = request;
                            set_token(&mut more, token);
                            next = Some(more);
                        }
                    }
                    Err(error) => return Some((Err(error), (None, buffer, fetch))),
                }
            }
        },
    )
}

impl<T> SqlModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    unary!(
        /// `SqlService/Query`: one read-only statement, its rows as JSON-shaped
        /// values in `columns` order.
        query, "sql", "query", s::QueryRequest, s::QueryResponse, query_with_options
    );
    unary!(
        /// `SqlService/QueryArrow`: the same result as one Arrow IPC stream
        /// (`arrow_ipc`), which `arrow-ipc`'s `StreamReader` reads; the SDK
        /// does not depend on Arrow.
        query_arrow, "sql", "query_arrow", s::QueryArrowRequest, s::QueryArrowResponse, query_arrow_with_options
    );

    /// One statement with its `$1…$n` values (JSON scalars, which the server
    /// casts to each placeholder's inferred type), answered as records
    /// ([`sql_records`]).
    ///
    /// # Errors
    ///
    /// `invalid_argument` before any call for a value that is not a
    /// `google.protobuf.Value`; otherwise the call's failure.
    pub async fn records(
        &self,
        statement: &str,
        params: Vec<serde_json::Value>,
        options: CallOptionsOverrides,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, LoamsError> {
        let params = params
            .into_iter()
            .enumerate()
            .map(|(at, value)| {
                serde_json::from_value(value).map_err(|error| {
                    LoamsError::new(
                        crate::error::ErrorKind::InvalidArgument,
                        connectrpc::ErrorCode::InvalidArgument,
                        format!("sql parameter ${}: {error}", at + 1),
                    )
                    .with_metadata("field", "params")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let request = s::QueryRequest {
            sql: statement.to_owned(),
            params,
            ..Default::default()
        };
        Ok(sql_records(&self.query(request, options).await?))
    }
}

impl<T> StreamsModule<'_, T>
where
    T: ClientTransport + Send + Sync + 'static,
    T::ResponseBody: Unpin,
    <T::ResponseBody as Body>::Error: std::fmt::Display,
{
    unary!(
        /// `StreamService/CreateStream`, retried: idempotent by name.
        create, "streams", "create", st::CreateStreamRequest, st::CreateStreamResponse, create_stream_with_options
    );
    unary!(
        /// `StreamService/DescribeStream`: one stream and its partitions' offsets.
        get, "streams", "get", st::DescribeStreamRequest, st::DescribeStreamResponse, describe_stream_with_options
    );
    unary!(
        /// `StreamService/ListStreams`, one page.
        list, "streams", "list", st::ListStreamsRequest, st::ListStreamsResponse, list_streams_with_options
    );
    unary!(
        /// `StreamService/Produce`, never retried by the SDK: set `producer_id`
        /// and `producer_seq` so a retry of yours cannot append twice.
        produce, "streams", "produce", st::ProduceRequest, st::ProduceResponse, produce_with_options
    );
    unary!(
        /// `StreamService/Fetch`: one partition's records from an offset.
        fetch, "streams", "fetch", st::FetchRequest, st::FetchResponse, fetch_with_options
    );
    unary!(
        /// `StreamService/ProduceCloudEvents`, never retried by the SDK.
        produce_events, "streams", "produce_events", st::ProduceCloudEventsRequest, st::ProduceCloudEventsResponse, produce_cloud_events_with_options
    );
    unary!(
        /// `StreamService/FetchCloudEvents`: records as a CloudEvents JSON batch.
        fetch_events, "streams", "fetch_events", st::FetchCloudEventsRequest, st::FetchCloudEventsResponse, fetch_cloud_events_with_options
    );

    /// Every stream, page after page (R6).
    pub fn list_all(
        &self,
        request: st::ListStreamsRequest,
        options: CallOptionsOverrides,
    ) -> impl Stream<Item = Result<st::StreamInfo, LoamsError>> + '_ {
        pages(
            request,
            move |request| {
                let options = options.clone();
                async move {
                    let page = self.list(request, options).await?;
                    Ok((page.streams, page.next_page_token))
                }
            },
            |request, token| request.page_token = token,
        )
    }
}

impl crate::subscribe::OffsetRecord for st::FetchedRecord {
    fn offset(&self) -> u64 {
        self.offset
    }
}

/// A `sql.query` answer's rows as one JSON object per row, keyed by column
/// name (SDKP §2.5). A 64-bit integer past 2^53 is the decimal string the
/// server sent, so it stays exact; a column name that repeats keeps its last
/// value.
#[must_use]
pub fn sql_records(answer: &s::QueryResponse) -> Vec<serde_json::Map<String, serde_json::Value>> {
    answer
        .rows
        .iter()
        .map(|row| {
            let values = match serde_json::to_value(row) {
                Ok(serde_json::Value::Array(values)) => values,
                _ => Vec::new(),
            };
            answer
                .columns
                .iter()
                .enumerate()
                .map(|(at, column)| {
                    (
                        column.name.clone(),
                        values.get(at).cloned().unwrap_or(serde_json::Value::Null),
                    )
                })
                .collect()
        })
        .collect()
}
