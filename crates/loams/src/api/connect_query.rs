// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type, which is the intended refinement (`connect.rs`
// and `connect_documents.rs` do the same for `loams.instance.v1` and
// `loams.collection.v1.DocumentService`).
#![allow(refining_impl_trait)]

//! `loams.collection.v1.QueryService` on the main port (design §44 §4 and §5.1,
//! ruling 1; API1 Task 4).
//!
//! `Search` is the Connect shape of `POST /v1/namespaces/{ns}/query`, and it
//! calls **the same** `CollectionService::search` that route calls. The REST
//! route stays until Task 9 (the plan's "behaviour first, deletion last"), so
//! the two surfaces must not drift — which is why this module parses nothing of
//! its own. [`super::connect_query_ir`] turns the request message into the
//! **native REST JSON** and `loams_query::json::hybrid::parse_query_body`, the
//! REST route's own parser, reads it, and turns the answer back into the
//! generated message; [`super::connect_query_filters`] holds the `Query` and
//! `SortKey` arms of that mapping, which the retrievers need. One parse, one
//! validate, one `search`.
//!
//! ## What this module does do, and why each thing is here
//!
//! - **The namespace and the collection**, by [`collection_ref`], exactly as
//!   `connect_documents` checks them: an empty name is a malformed request, not
//!   a missing resource.
//! - **The consistency**, through [`read_consistency`] with the request's own
//!   `Consistency` message. This is where the note at `api::router` matters: the
//!   layer that sets the hot scope does not read `loams-consistency-token` and
//!   does not touch the message, so a Connect call's consistency is still the
//!   one its body states, merged with the header by the same function the REST
//!   route uses.
//! - **The token**, in the `loams-consistency-token` response header as well as
//!   in `read_token`, because that is the rule for a response half (design §44
//!   §7.4) and Task 3's `Documents::read_answer` already spells it.
//!
//! ## The hot tier reaches this path, and how
//!
//! Ruling 11 puts every route inside `HotLayer`, and §05 §4 makes the hot tier a
//! query stage, so a `Search` that could not use a hot structure would be a
//! different query engine from the REST one. The layer is applied to the
//! **connect router specifically** — see [`super::connect_hot`] — rather than by
//! merging the connect routes into the one the REST routes live in, because the
//! layer's refusal has to be a Connect error and it cannot tell a Connect call
//! from a REST one by content type (`application/json` is both). Inside the
//! connect router there is no ambiguity: every request is an RPC, so the layer
//! answers every bad header in the Connect protocol's own shape.
//!
//! The consistency-in-message guarantee is untouched by that: [`HotLayer`] sets a
//! task-local scope and a response header and reads only `Loams-Hot`. It never
//! reads a consistency token, so applying it to an RPC path cannot override the
//! consistency the request message carries.
//!
//! ## The M3 stages
//!
//! A `Search` carrying `rerank` or `expand` is `invalid_argument` and says so.
//! Both fields are declared in `query.proto` precisely so this is possible:
//! proto3 JSON ignores an undeclared field, and a silently dropped stage is the
//! one failure a caller cannot detect. [`super::connect_query_ir::request_json`]
//! checks them before it reads anything else.

use std::sync::Arc;

use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use loams_proto::loams::collection::v1::{
    QueryService, QueryServiceExt, SearchRequest, SearchResponse,
};
use loams_query::json::hybrid::parse_query_body;

use super::connect_errors::{invalid, refused, refused_service};
use super::connect_query_ir as ir;
use super::{AppState, CONSISTENCY_TOKEN, read_consistency};

/// `loams.collection.v1.QueryService` over the server's state.
#[derive(Debug)]
struct Query {
    state: AppState,
}

/// The namespace and the collection (a name or an alias) a request names.
///
/// Identical to `connect_documents::collection_ref` and for the same reason: an
/// empty name is a malformed request rather than a missing resource, because
/// `not_found` would put a name in `metadata` that no caller sent.
///
/// It differs from it in one way, and the difference is `from`: §05 §4's body
/// names the collection `from` and sends no `collection` at all, so refusing
/// here without looking at `from` would refuse the design's own example.
/// `ir::request_json` reads `from` (stripping its optional `collections.`
/// prefix) into the body; this only answers "is a collection named at all".
fn collection_ref<'a>(
    namespace: &'a str,
    collection: &'a str,
    from: &'a str,
) -> Result<(&'a str, &'a str), ConnectError> {
    if namespace.is_empty() {
        return Err(invalid("namespace", "a namespace is required"));
    }
    // The §05 §4 alias: a body written against the hybrid form names the
    // collection `from` and sends no `collection` at all, so refusing here would
    // refuse the design's own example body. `from` may carry the
    // `collections.` prefix, which the REST route strips; `ir::request_json`
    // strips it too, and this only answers "is a collection named at all".
    let named = if collection.is_empty() {
        from
    } else {
        collection
    };
    if named.is_empty() {
        return Err(invalid(
            "collection",
            "a collection name or alias is required",
        ));
    }
    Ok((namespace, collection))
}

impl Query {
    fn new(state: AppState) -> Arc<Self> {
        Arc::new(Self { state })
    }
}

impl QueryService for Query {
    /// One search over one collection.
    ///
    /// The whole IR is the request, so this is three steps and no logic: turn
    /// the message into the REST route's body, run the REST route's own parser
    /// and the service call, and turn the answer back into the message. A field
    /// the mapping does not understand cannot be accepted-and-ignored here — it
    /// is either emitted under the key the IR reads or refused while mapping.
    async fn search(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, SearchRequest>,
    ) -> ServiceResult<SearchResponse> {
        let (namespace, _collection) =
            collection_ref(request.namespace, request.collection, request.from)?;
        // The collection itself reaches the IR through the body: `collection`, or
        // the §05 §4 alias `from`, both mapped in `request_json`. `collection_ref`
        // is what refuses an empty name before the service is called.
        let body = ir::request_json(request.view())?;
        // Date math (`now-30d`) is relative to the proposer's clock, on both
        // surfaces: the same call `api::query::search` makes.
        let mut search =
            parse_query_body(body, self.state.meta.now_ms()).map_err(refused_service)?;
        // The same rule the REST route applies after its own parse (rule 1).
        search.consistency =
            read_consistency(ctx.headers(), Some(search.consistency)).map_err(refused)?;
        let response = self
            .state
            .collections
            .search(namespace, search)
            .await
            .map_err(refused_service)?;
        let token = response.read_token.to_string();
        let body = ir::response_json(&response);
        Ok(Response::new(body).with_header(CONSISTENCY_TOKEN.as_str(), &token))
    }
}

/// `loams.collection.v1.QueryService`, registered on the router.
pub(super) fn register(router: connectrpc::Router, state: &AppState) -> connectrpc::Router {
    QueryServiceExt::register(Query::new(state.clone()), router)
}
