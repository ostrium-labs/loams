// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type, which is the intended refinement (`connect.rs`
// does the same for `loams.instance.v1`).
#![allow(refining_impl_trait)]

//! `loams.collection.v1.NamespaceService` and `CollectionService` on the main
//! port (design §44 §4 and §5.1, ruling 4; API1 Task 2).
//!
//! Every RPC here is the Connect shape of a route in
//! [`crate::api::collections`], [`crate::api::hot`] or `create_namespace`, and
//! it calls **the same** service trait the route calls:
//! `MetaStore::create_namespace`,
//! `CollectionService::{create_collection, list_collections, get_collection,
//! drop_collection, add_fields, versions, scan_plan, update_aliases}`,
//! `MetaStore::set_collection_hot`, `hot::hot_status_value` and
//! `hot::warm_owner`. The REST routes stay until Task 9 (the plan's
//! "behaviour first, deletion last"), so the two surfaces must not drift: the
//! only thing this module adds is [`super::connect_messages`]'s mapping to the
//! generated messages, and the request-message names ruling 4 moved out of the
//! URL.
//!
//! ## What the request messages take instead of a path
//!
//! `namespace` and `collection` are fields, so one path serves every
//! collection and a path that carries a name is not a route at all. A
//! collection is named by its name or by an alias wherever a collection is
//! named, and `GetCollection`'s answer reports the collection's own name.
//!
//! ## Failures
//!
//! Every failure goes through [`refused`], which carries one
//! `loams.errors.v1.ErrorInfo` in the Connect error's details whose `reason`
//! is registered in `docs/api/reasons.md` (D611). `ApiError`'s own codes
//! become the reasons unchanged (plan ruling 1.6), so there is no second error
//! hierarchy to keep in step, and a missing collection's `kind` and `name` ride
//! in `metadata`: a caller tells a missing collection from a missing namespace
//! without parsing prose.

use std::collections::BTreeMap;
use std::sync::Arc;

use buffa::MessageField;
use buffa::RepeatedView;
use connectrpc::{ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult};
use loams_common::meta::{HotConfig, MetaError};
use loams_proto::loams::collection::v1 as pb;
use loams_proto::loams::collection::v1::{
    AddFieldsRequest, AddFieldsResponse, CollectionService, CollectionServiceExt,
    CreateCollectionRequest, CreateNamespaceRequest, CreateNamespaceResponse, DropCollectionRequest,
    DropCollectionResponse, GetCollectionRequest, ListCollectionsRequest, ListCollectionsResponse,
    ListVersionsRequest, ListVersionsResponse, NamespaceService, NamespaceServiceExt, ScanRequest,
    SetHotRequest, SetHotResponse, UpdateAliasesRequest, UpdateAliasesResponse,
    WarmCollectionRequest, WarmCollectionResponse,
};
use loams_query::json::schema;
use loams_query::{ScanAt, ServiceError, alias_actions_from_json};
use serde_json::{Map, Value, json};

use super::connect::refuse;
use super::connect_messages as msg;
use super::{ApiError, AppState, CONSISTENCY_TOKEN, hot};

/// Both services of `loams.collection.v1`, over the server's state.
#[derive(Debug)]
pub(super) struct Collections {
    state: AppState,
}

impl Collections {
    pub(super) fn new(state: AppState) -> Arc<Self> {
        Arc::new(Self { state })
    }

    /// The namespace a request names. An empty name is a malformed request,
    /// not a missing resource: `not_found` would put a name in `metadata` that
    /// no caller sent.
    fn namespace(request_namespace: &str) -> Result<&str, ConnectError> {
        if request_namespace.is_empty() {
            return Err(invalid("namespace", "a namespace is required"));
        }
        Ok(request_namespace)
    }

    /// The namespace and the collection (a name or an alias) a request names.
    fn collection_ref<'a>(
        request_namespace: &'a str,
        collection: &'a str,
    ) -> Result<(&'a str, &'a str), ConnectError> {
        let namespace = Self::namespace(request_namespace)?;
        if collection.is_empty() {
            return Err(invalid(
                "collection",
                "a collection name or alias is required",
            ));
        }
        Ok((namespace, collection))
    }
}

// ----- Failures -----

/// A malformed request: `invalid_argument` naming the field that is missing.
fn invalid(field: &str, message: &str) -> ConnectError {
    refuse(
        ErrorCode::InvalidArgument,
        "invalid_argument",
        message.to_owned(),
        &[("field", field)],
    )
}

/// An `ApiError` as a Connect error. Its code becomes the registry `reason`
/// unchanged (plan ruling 1.6) and its structured fields — `kind` and `name`
/// for a `NotFound`, `field`, `retry_after_ms` — become `metadata`, so a
/// caller branches on the reason and reads the specifics without parsing the
/// message.
fn refused(err: ApiError) -> ConnectError {
    let (code, reason) = classify(err.code());
    let metadata: Vec<(&str, &str)> = err
        .extra()
        .iter()
        .filter_map(|(key, value)| value.as_str().map(|value| (key.as_str(), value)))
        .collect();
    refuse(code, reason, err.message(), &metadata)
}

/// A service failure as a Connect error, through the same `ApiError` the REST
/// route builds, so the two surfaces classify a failure identically.
fn refused_service(err: ServiceError) -> ConnectError {
    refused(ApiError::from(err))
}

/// A metastore failure, the same way.
fn refused_meta(err: MetaError) -> ConnectError {
    refused(ApiError::from(err))
}

/// The Connect code and registry reason an `ApiError` code maps to.
///
/// Every code `api::errors` raises is a registry row except three, which fold
/// into the row they mean: a `schema_violation` is an `invalid_argument` (the
/// offending field rides in `metadata`), a REST `conflict` is an `aborted` (a
/// fenced lease or a version mismatch is a concurrent write that won), and a
/// REST `timeout` is a `deadline_exceeded`. A code with no mapping is a layer
/// that grew one behind the registry, so it answers `internal` and is logged
/// rather than inventing a reason.
fn classify(code: &str) -> (ErrorCode, &'static str) {
    match code {
        "invalid_argument" | "schema_violation" => (ErrorCode::InvalidArgument, "invalid_argument"),
        "not_found" => (ErrorCode::NotFound, "not_found"),
        "already_exists" => (ErrorCode::AlreadyExists, "already_exists"),
        "conflict" => (ErrorCode::Aborted, "aborted"),
        "resource_exhausted" => (ErrorCode::ResourceExhausted, "resource_exhausted"),
        "unavailable" => (ErrorCode::Unavailable, "unavailable"),
        "timeout" => (ErrorCode::DeadlineExceeded, "deadline_exceeded"),
        "permission_denied" => (ErrorCode::PermissionDenied, "permission_denied"),
        "unauthenticated" => (ErrorCode::Unauthenticated, "unauthenticated"),
        "internal" => (ErrorCode::Internal, "internal"),
        other => {
            tracing::error!(code = other, "an API error code has no Connect mapping");
            (ErrorCode::Internal, "internal")
        }
    }
}

// ----- AIP-158 paging -----

/// One page of a list this server answers from memory (design §44 §5.1's note
/// that `ListCollections` is paginated).
///
/// The token is the offset of the next page's first item, in decimal. It is
/// opaque to the caller by AIP-158's own terms, and it is exactly as stable as
/// the list it was taken from: a collection created or dropped between two
/// pages shifts what a later page holds, the same trade the REST route made by
/// answering the whole list at once. A caller that needs a stable walk reads
/// versions or documents instead.
struct Page {
    offset: usize,
    /// `usize::MAX` for "every item that fits", which is what an absent or
    /// non-positive `page_size` means.
    size: usize,
}

impl Page {
    fn new(page_size: i32, page_token: &str) -> Result<Self, ConnectError> {
        let offset = if page_token.is_empty() {
            0
        } else {
            page_token.parse().map_err(|_| {
                invalid(
                    "page_token",
                    "page_token must be a next_page_token this server answered",
                )
            })?
        };
        let size = usize::try_from(page_size).unwrap_or(0);
        Ok(Self {
            offset,
            size: if size == 0 { usize::MAX } else { size },
        })
    }

    /// The page, and the token of the next one (empty on the last page). An
    /// offset past the end (the list shrank between pages) is an empty page
    /// with no token, not an error.
    fn finish<T: Clone>(self, items: Vec<T>) -> (Vec<T>, String) {
        let end = self.offset.saturating_add(self.size).min(items.len());
        let page = items.get(self.offset..end).unwrap_or_default().to_vec();
        let next = if end < items.len() {
            end.to_string()
        } else {
            String::new()
        };
        (page, next)
    }
}

impl NamespaceService for Collections {
    async fn create_namespace(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, CreateNamespaceRequest>,
    ) -> ServiceResult<CreateNamespaceResponse> {
        let name = Collections::namespace(request.namespace)?;
        let id = self
            .state
            .meta
            .create_namespace(name)
            .await
            .map_err(refused_meta)?;
        Response::ok(CreateNamespaceResponse {
            namespace: name.to_owned(),
            namespace_id: id.0,
            ..Default::default()
        })
    }
}

impl CollectionService for Collections {
    async fn create_collection(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, CreateCollectionRequest>,
    ) -> ServiceResult<pb::CollectionInfo> {
        let namespace = Collections::namespace(request.namespace)?;
        let Some(schema) = request.schema.as_option() else {
            return Err(invalid("schema", "create_collection needs a schema"));
        };
        let schema = schema::from_json(&msg::json_of_view(schema)).map_err(refused_service)?;
        let info = self
            .state
            .collections
            .create_collection(namespace, request.name, schema, request.partitions)
            .await
            .map_err(refused_service)?;
        Response::ok(msg::collection_info(
            &info,
            Some(msg::catalog_hot(&info.hot)),
        ))
    }

    async fn list_collections(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, ListCollectionsRequest>,
    ) -> ServiceResult<ListCollectionsResponse> {
        let namespace = Collections::namespace(request.namespace)?;
        let page = Page::new(request.page_size, request.page_token)?;
        let collections = self
            .state
            .collections
            .list_collections(namespace)
            .await
            .map_err(refused_service)?;
        let (collections, next_page_token) = page.finish(
            collections
                .iter()
                .map(|info| msg::collection_info(info, Some(msg::catalog_hot(&info.hot))))
                .collect(),
        );
        Response::ok(ListCollectionsResponse {
            collections,
            next_page_token,
            ..Default::default()
        })
    }

    async fn get_collection(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, GetCollectionRequest>,
    ) -> ServiceResult<pb::CollectionInfo> {
        let (namespace, collection) =
            Collections::collection_ref(request.namespace, request.collection)?;
        let info = self
            .state
            .collections
            .get_collection(namespace, collection)
            .await
            .map_err(refused_service)?;
        // B6: the owner's full hot status replaces the catalog summary, so a
        // caller reading a collection from any node reads the node that serves
        // its queries (M1.3 rule 4). A resolve that disagreed with the read
        // keeps the summary: the catalog value is the safe answer.
        let (ns_id, cid) = hot::resolve(&self.state, namespace, collection)
            .await
            .map_err(refused)?;
        let hot = if cid == info.id {
            msg::hot_status(&hot::hot_status_value(&self.state, ns_id, cid).await)
        } else {
            msg::catalog_hot(&info.hot)
        };
        Response::ok(msg::collection_info(&info, Some(hot)))
    }

    async fn drop_collection(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, DropCollectionRequest>,
    ) -> ServiceResult<DropCollectionResponse> {
        let (namespace, collection) =
            Collections::collection_ref(request.namespace, request.collection)?;
        let dropped = self
            .state
            .collections
            .drop_collection(namespace, collection)
            .await
            .map_err(refused_service)?;
        Response::ok(DropCollectionResponse {
            dropped,
            ..Default::default()
        })
    }

    async fn add_fields(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, AddFieldsRequest>,
    ) -> ServiceResult<AddFieldsResponse> {
        let (namespace, collection) =
            Collections::collection_ref(request.namespace, request.collection)?;
        let fields = request
            .fields
            .iter()
            .map(|field| schema::field_from_json(&msg::json_of_view(field)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(refused_service)?;
        let vectors = request
            .vectors
            .iter()
            .map(|vector| schema::vector_from_json(&msg::json_of_view(vector)))
            .collect::<Result<Vec<_>, _>>()
            .map_err(refused_service)?;
        let annotations: BTreeMap<String, String> = request
            .annotations
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        let schema = self
            .state
            .collections
            .add_fields(namespace, collection, fields, vectors, annotations)
            .await
            .map_err(refused_service)?;
        Response::ok(AddFieldsResponse {
            schema: MessageField::some(msg::schema_struct(&schema)),
            ..Default::default()
        })
    }

    async fn list_versions(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, ListVersionsRequest>,
    ) -> ServiceResult<ListVersionsResponse> {
        let (namespace, collection) =
            Collections::collection_ref(request.namespace, request.collection)?;
        let page = Page::new(request.page_size, request.page_token)?;
        let versions = self
            .state
            .collections
            .versions(namespace, collection)
            .await
            .map_err(refused_service)?;
        let (versions, next_page_token) =
            page.finish(versions.iter().map(msg::manifest_version).collect());
        Response::ok(ListVersionsResponse {
            versions,
            next_page_token,
            ..Default::default()
        })
    }

    async fn scan(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, ScanRequest>,
    ) -> ServiceResult<pb::ScanPlan> {
        let (namespace, collection) =
            Collections::collection_ref(request.namespace, request.collection)?;
        let at = scan_at(request.at.as_option()).map_err(refused_service)?;
        let plan = self
            .state
            .collections
            .scan_plan(namespace, collection, at)
            .await
            .map_err(refused_service)?;
        // The pin's token is the answer a non-Connect reader has: it rides in
        // the same `loams-consistency-token` header the REST route sets and in
        // `ScanPlan.pin.token`, and it is a `ScanPoint.token` that plans the
        // same state again (M1.2 rule 6).
        Ok(Response::new(msg::scan_plan(&plan)).with_header(
            CONSISTENCY_TOKEN.as_str(),
            msg::token_of(&plan.pin.token),
        ))
    }

    async fn update_aliases(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, UpdateAliasesRequest>,
    ) -> ServiceResult<UpdateAliasesResponse> {
        let namespace = Collections::namespace(request.namespace)?;
        let actions =
            alias_actions_from_json(&alias_actions(&request.actions)).map_err(refused_service)?;
        self.state
            .collections
            .update_aliases(namespace, actions)
            .await
            .map_err(refused_service)?;
        Response::ok(UpdateAliasesResponse {
            ..Default::default()
        })
    }

    async fn set_hot(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, SetHotRequest>,
    ) -> ServiceResult<SetHotResponse> {
        let (namespace, collection) =
            Collections::collection_ref(request.namespace, request.collection)?;
        let Some(hot) = request.hot.as_option() else {
            return Err(invalid("hot", "set_hot needs the configuration to write"));
        };
        let config = HotConfig {
            vectors: hot.vectors,
            text: hot.text,
            fragments: hot.fragments,
        };
        let (ns_id, cid) = hot::resolve(&self.state, namespace, collection)
            .await
            .map_err(refused)?;
        self.state
            .meta
            .set_collection_hot(ns_id, cid, config)
            .await
            .map_err(refused_meta)?;
        // The status the local tier then builds towards, read from the owner,
        // which on a single node is this one (M1.3 rules 1 and 4).
        let status = hot::hot_status_value(&self.state, ns_id, cid).await;
        Response::ok(SetHotResponse {
            hot: MessageField::some(msg::hot_status(&status)),
            ..Default::default()
        })
    }

    async fn warm_collection(
        &self,
        _ctx: RequestContext,
        request: ServiceRequest<'_, WarmCollectionRequest>,
    ) -> ServiceResult<WarmCollectionResponse> {
        let (namespace, collection) =
            Collections::collection_ref(request.namespace, request.collection)?;
        let (ns_id, cid) = hot::resolve(&self.state, namespace, collection)
            .await
            .map_err(refused)?;
        hot::warm_owner(&self.state, ns_id, cid)
            .await
            .map_err(refused)?;
        Response::ok(WarmCollectionResponse {
            ..Default::default()
        })
    }
}

/// The scan point a request names.
///
/// A `ScanPoint` is a oneof, so the state is always named. An absent `at` (and
/// an empty one) is the live manifest, which is how the REST route's bare
/// `"current"` string is spelled here: a oneof has no bare-string form.
fn scan_at(point: Option<&pb::ScanPointView<'_>>) -> Result<ScanAt, ServiceError> {
    use pb::__buffa::view::oneof::scan_point::Point;
    match point.and_then(|point| point.point.as_ref()) {
        None => Ok(ScanAt::Current),
        Some(Point::ManifestVersion(version)) => Ok(ScanAt::ManifestVersion(*version)),
        Some(Point::Token(token)) => token.parse().map(ScanAt::Token).map_err(|err| {
            ServiceError::InvalidArgument(format!("invalid consistency token in at: {err}"))
        }),
    }
}

/// The alias actions of an `UpdateAliases` request, back to the JSON
/// `alias_actions_from_json` parses. A proto oneof spells a message variant as
/// its own key, so `{"create": {…}}` and `{"delete": {…}}` are unchanged from
/// the REST route; an action with no variant set names no change and is
/// dropped.
fn alias_actions(actions: &RepeatedView<'_, pb::AliasActionView<'_>>) -> Value {
    use pb::__buffa::view::oneof::alias_action::Action;
    Value::Array(
        actions
            .iter()
            .filter_map(|action| {
                let (key, body) = match action.action.as_ref()? {
                    Action::Create(create) => (
                        "create",
                        json!({ "alias": create.alias, "collection": create.collection }),
                    ),
                    Action::Delete(delete) => ("delete", json!({ "alias": delete.alias })),
                };
                Some(Value::Object(Map::from_iter([(key.to_owned(), body)])))
            })
            .collect(),
    )
}

/// Both services of `loams.collection.v1`, registered on the router.
pub(super) fn register(router: connectrpc::Router, state: &AppState) -> connectrpc::Router {
    let collections = Collections::new(state.clone());
    let router = NamespaceServiceExt::register(Arc::clone(&collections), router);
    CollectionServiceExt::register(collections, router)
}