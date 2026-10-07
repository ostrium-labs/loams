// The generated service traits return `impl Encodable<_>`; these impls name
// the concrete message type, which is the intended refinement (`connect.rs`
// does the same for `loams.instance.v1`).
#![allow(refining_impl_trait)]

//! `loams.collection.v1.DocumentService` on the main port (design §44 §4 and
//! §5.1, ruling 4; API1 Task 3).
//!
//! Every RPC here is the Connect shape of a route in
//! [`crate::api::collections`], and it calls **the same** service trait the
//! route calls: `CollectionService::{write, get_with_token, scroll_with_token,
//! count_with_token, delete_by_filter, patch_by_filter}`. The REST routes stay
//! until Task 9 (the plan's "behaviour first, deletion last"), so the two
//! surfaces must not drift. That is why the handlers here do no parsing of
//! their own: [`super::connect_messages`] turns each request message back into
//! the **native REST JSON** and the route's own `op_from_json`,
//! `patch_spec_from_json` and `json_pk::from_json` parse it, so a rejected op
//! is refused by the same code with the same `op i:` message and the same
//! `index` metadata on both surfaces. The only thing this module adds is the
//! request-message names ruling 4 moved out of the URL.
//!
//! ## What stays a header
//!
//! Two request headers survive the move to RPCs, and they are read by the same
//! functions that read them for REST:
//!
//! - `loams-backpressure: off` overrides the write budget (M1.3 Task 15 rule
//!   5). It is a transport-level switch with no message-level meaning, and an
//!   unrecognised value is `invalid_argument` — the same refusal the REST
//!   route makes.
//! - `loams-consistency-token` merges into a read's or a filter write's
//!   `consistency`, by [`read_consistency`], unchanged.
//!
//! Everything else a caller can set is in the request message, and everything
//! this server answers is in the body as well as in the header: the token in
//! `loams-consistency-token`, the backlog in `loams-unapplied-records` and
//! `loams-unapplied-bytes` (design §44 §7.4: a response half is headers by
//! rule, and a Connect caller reads them without parsing the body).
//!
//! ## The idempotency ledger, and what it is not
//!
//! `WriteDocuments` is the first RPC in the API that is not idempotent by
//! construction (plan ruling 2.4), so it carries `idempotency_key`. The ledger
//! that makes a retry a **replay** is [`super::connect_idempotency`], and it is
//! worth being exact about what it is:
//!
//! - **Per process.** It is memory of *this* node. A retry that lands on
//!   another node of a cluster is a fresh write there, which is the same gap
//!   every REST write has and the reason a shared ledger is a later task, not a
//!   silent assumption here.
//! - **The immediate retry.** A keyed write's answer is kept for
//!   [`super::connect_idempotency::WINDOW`] and for at most
//!   [`super::connect_idempotency::ENTRIES`] other keyed writes. Past either
//!   bound a repeat is a fresh write, which is the REST behaviour: an answer
//!   that is wrong is worse than one that is late.
//! - **Only a repeat of the *same* request.** The ledger fingerprints the ops
//!   and the `report_existence` flag, so one key under two different requests
//!   does not answer one request's write with another's token. Two requests
//!   that differ only in the order of an unordered document's source keys are
//!   the same request; everything else is not.
//!
//! A caller that must not risk a double write keeps its own key ledger, which is
//! what AP0's rule asks for anyway (D610).

use std::sync::Arc;

use buffa::MessageField;
use buffa::MessageFieldView;
use buffa::enumeration::EnumValue;
use connectrpc::{ConnectError, RequestContext, Response, ServiceRequest, ServiceResult};
use loams_collection::PrimaryKey;
use loams_proto::google::protobuf::__buffa::view::{StructView, ValueView};
use loams_proto::loams::collection::v1 as pb;
use loams_proto::loams::collection::v1::{
    ConsistencyView, CountDocumentsRequest, CountDocumentsResponse, DeleteByFilterRequest,
    DocumentIdView, DocumentService, DocumentServiceExt, FilterWriteCursorView,
    FilterWriteResponse, GetDocumentsRequest, GetDocumentsResponse, PatchByFilterRequest,
    PatchView, ScrollDocumentsRequest, ScrollDocumentsResponse, WriteDocumentView,
    WriteDocumentsRequest, WriteDocumentsRequestView, WriteDocumentsResponse, WriteOpView,
};
use loams_query::filter_write::{FilterWriteCursor, FilterWriteResult, over_limit};
use loams_query::json::pk as json_pk;
use loams_query::{
    Backlog, FilterWriteOptions, OpResult, Projection, Query, ReadConsistency, ServiceError,
    WriteOptions, rejected_op_index,
};
use serde_json::{Map, Value, json};

use super::collections::{
    DEFAULT_SCROLL_LIMIT, backpressure_of, filter_write_options, op_error, op_from_json,
    patch_spec_from_json,
};
use super::connect_errors::{invalid, refused, refused_service, refused_with};
use super::connect_idempotency::Ledger;
use super::connect_messages as msg;
use super::{
    ApiError, AppState, CONSISTENCY_TOKEN, UNAPPLIED_BYTES_HEADER, UNAPPLIED_RECORDS_HEADER,
    read_consistency,
};

/// `loams.collection.v1.DocumentService` over the server's state.
#[derive(Debug)]
pub(super) struct Documents {
    state: AppState,
    /// The answers of recently-keyed writes, so an immediate retry replays.
    ledger: Ledger,
}

/// The namespace and the collection (a name or an alias) a request names.
///
/// Identical to `connect_collections::Collections::collection_ref`, and for the
/// same reason: an empty name is a malformed request rather than a missing
/// resource, because `not_found` would put a name in `metadata` that no caller
/// sent.
fn collection_ref<'a>(
    namespace: &'a str,
    collection: &'a str,
) -> Result<(&'a str, &'a str), ConnectError> {
    if namespace.is_empty() {
        return Err(invalid("namespace", "a namespace is required"));
    }
    if collection.is_empty() {
        return Err(invalid(
            "collection",
            "a collection name or alias is required",
        ));
    }
    Ok((namespace, collection))
}

impl Documents {
    pub(super) fn new(state: AppState) -> Arc<Self> {
        Arc::new(Self {
            state,
            ledger: Ledger::default(),
        })
    }

    /// A read's consistency: its `consistency` message merged with the
    /// `loams-consistency-token` request header by the REST route's own rule.
    fn consistency(
        ctx: &RequestContext,
        consistency: &MessageFieldView<ConsistencyView<'_>>,
    ) -> Result<ReadConsistency, ConnectError> {
        let asked = match consistency.as_option() {
            None => None,
            Some(asked) => Some(asked_of(asked)?),
        };
        read_consistency(ctx.headers(), asked).map_err(refused)
    }

    /// The backlog headers of a write's answer, measured at admission.
    fn backlog_headers(backlog: &Backlog) -> axum::http::HeaderMap {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            UNAPPLIED_RECORDS_HEADER,
            axum::http::HeaderValue::from(backlog.records),
        );
        headers.insert(
            UNAPPLIED_BYTES_HEADER,
            axum::http::HeaderValue::from(backlog.bytes),
        );
        headers
    }

    /// A write's answer with its token and its backlog headers.
    fn write_answer(
        response: &WriteDocumentsResponse,
        backlog: &Backlog,
    ) -> Response<WriteDocumentsResponse> {
        Response::new(response.clone())
            .with_header(CONSISTENCY_TOKEN.as_str(), response.token.as_str())
            .with_header(UNAPPLIED_RECORDS_HEADER, backlog.records)
            .with_header(UNAPPLIED_BYTES_HEADER, backlog.bytes)
    }

    /// A read's answer with its token header.
    fn read_answer<B>(body: B, token: &str) -> Response<B> {
        Response::new(body).with_header(CONSISTENCY_TOKEN.as_str(), token)
    }
}

// ----- `WriteDocuments` -----

impl DocumentService for Documents {
    async fn write_documents(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, WriteDocumentsRequest>,
    ) -> ServiceResult<WriteDocumentsResponse> {
        let (namespace, collection) = collection_ref(request.namespace, request.collection)?;
        // Read before the ledger: an unrecognised `loams-backpressure` is the
        // caller's mistake whatever the key says, and a refused call is not a
        // replay of anything.
        let backpressure = backpressure_of(ctx.headers()).map_err(refused)?;
        let key = request.idempotency_key;
        let fingerprint = fingerprint(&request);

        // One key at a time, and only for a keyed write: the guard is what
        // makes "a retry must not write twice" true for a retry that arrives
        // while the first is still running, not only for one that arrives after
        // it. It is taken before the ledger is read, so the second caller sees
        // the first's answer rather than racing it.
        let gate = self.ledger.gate(namespace, collection, key);
        let _held = gate.lock().await;

        if !key.is_empty()
            && let Some(answer) = self.ledger.replay(namespace, collection, key, fingerprint)
        {
            return Ok(Documents::write_answer(&answer.response, &answer.backlog));
        }

        let ops = request
            .ops
            .iter()
            .enumerate()
            .map(|(i, op)| op_from_json(i, &op_json(op)).map_err(|err| refused(op_error(i, err))))
            .collect::<Result<Vec<_>, _>>()?;
        let options = WriteOptions {
            report_existence: request.report_existence,
            atomic: true,
            backpressure,
        };
        let result = match self
            .state
            .collections
            .write(namespace, collection, ops, options)
            .await
        {
            Ok(result) => result,
            Err(err @ ServiceError::ResourceExhausted { .. }) => {
                // The measurement the refusal used (cached for the refresh
                // interval), which is what the backlog headers report.
                let backlog = self
                    .state
                    .collections
                    .collection_backlog(namespace, collection)
                    .await
                    .unwrap_or_default();
                return Err(refused_with(
                    ApiError::from(err),
                    Documents::backlog_headers(&backlog),
                ));
            }
            Err(err) => {
                return Err(refused(match rejected_op_index(&err) {
                    Some(i) => op_error(i, err),
                    None => ApiError::from(err),
                }));
            }
        };
        let mut results = Vec::with_capacity(result.results.len());
        for (i, op) in result.results.iter().enumerate() {
            results.push(match op {
                OpResult::Rejected(err) => {
                    // The writer refused an op the validation passed: the schema
                    // changed in between. The request fails with that op's error
                    // and still reports the backlog, exactly as REST does.
                    return Err(refused_with(
                        op_error(i, err.clone()),
                        Documents::backlog_headers(&result.backlog),
                    ));
                }
                other => msg::op_result(other),
            });
        }
        let response = WriteDocumentsResponse {
            token: result.token.to_string(),
            results,
            positions: result
                .positions
                .iter()
                .map(|position| msg::op_position(position.as_ref()))
                .collect(),
            // Also the two backlog headers. proto3 JSON omits a zero, so a
            // write admitted onto an empty backlog answers neither key there
            // and both headers — which is where a non-Connect reader reads them.
            unapplied_records: Some(result.backlog.records),
            unapplied_bytes: Some(result.backlog.bytes),
            ..Default::default()
        };
        if !key.is_empty() {
            self.ledger.remember(
                namespace,
                collection,
                key,
                fingerprint,
                response.clone(),
                result.backlog,
            );
        }
        Ok(Documents::write_answer(&response, &result.backlog))
    }

    async fn get_documents(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, GetDocumentsRequest>,
    ) -> ServiceResult<GetDocumentsResponse> {
        let (namespace, collection) = collection_ref(request.namespace, request.collection)?;
        let consistency = Documents::consistency(&ctx, &request.consistency)?;
        let select = projection(&request.select)?;
        let ids = pks(&request.ids)?;
        let (documents, token) = self
            .state
            .collections
            .get_with_token(namespace, collection, &ids, &select, consistency)
            .await
            .map_err(refused_service)?;
        let token = token.to_string();
        let response = GetDocumentsResponse {
            documents: documents
                .iter()
                // A missing id is an entry with no fields, which proto3 JSON
                // spells `{}`: it sits at the requested id's position and says
                // the document is not there. REST answers `null` here, which
                // proto3 JSON has no spelling for inside a repeated message.
                .map(|stored| {
                    stored
                        .as_ref()
                        .map_or_else(pb::Document::default, msg::document)
                })
                .collect(),
            read_token: token.clone(),
            ..Default::default()
        };
        Ok(Documents::read_answer(response, &token))
    }

    async fn scroll_documents(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ScrollDocumentsRequest>,
    ) -> ServiceResult<ScrollDocumentsResponse> {
        let (namespace, collection) = collection_ref(request.namespace, request.collection)?;
        let consistency = Documents::consistency(&ctx, &request.consistency)?;
        let select = projection(&request.select)?;
        let filter = query(&request.filter, "filter")?;
        let after = pk_of(&request.after)?;
        // `limit` is `optional`, so an explicit `0` is a page of nothing rather
        // than the default — proto3 JSON can spell the difference and the REST
        // route's `Option<usize>` could.
        let limit = request
            .limit
            .map_or(DEFAULT_SCROLL_LIMIT, |limit| limit as usize);
        let ((documents, next), token) = self
            .state
            .collections
            .scroll_with_token(
                namespace,
                collection,
                filter,
                after,
                limit,
                &select,
                consistency,
            )
            .await
            .map_err(refused_service)?;
        let token = token.to_string();
        let response = ScrollDocumentsResponse {
            documents: documents.iter().map(msg::document).collect(),
            next: next.as_ref().map_or_else(MessageField::none, |pk| {
                MessageField::some(msg::document_id(pk))
            }),
            read_token: token.clone(),
            ..Default::default()
        };
        Ok(Documents::read_answer(response, &token))
    }

    async fn count_documents(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, CountDocumentsRequest>,
    ) -> ServiceResult<CountDocumentsResponse> {
        let (namespace, collection) = collection_ref(request.namespace, request.collection)?;
        let consistency = Documents::consistency(&ctx, &request.consistency)?;
        let filter = query(&request.filter, "filter")?;
        let (count, token) = self
            .state
            .collections
            .count_with_token(namespace, collection, filter, consistency)
            .await
            .map_err(refused_service)?;
        let token = token.to_string();
        let response = CountDocumentsResponse {
            // Always set, including at zero: "nothing matched" is a count.
            count: Some(count),
            read_token: token.clone(),
            ..Default::default()
        };
        Ok(Documents::read_answer(response, &token))
    }

    async fn delete_by_filter(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, DeleteByFilterRequest>,
    ) -> ServiceResult<FilterWriteResponse> {
        let (namespace, collection) = collection_ref(request.namespace, request.collection)?;
        let filter = query(&request.filter, "filter")?.ok_or_else(|| {
            invalid(
                "filter",
                "delete_by_filter needs a filter; `match_all` deletes everything",
            )
        })?;
        let cursor = cursor(&request.cursor)?;
        let options = filter_write(
            &ctx,
            &request.consistency,
            request.max_rows,
            request.allow_partial,
            cursor,
        )?;
        let result = self
            .state
            .collections
            .delete_by_filter(namespace, collection, filter, options)
            .await;
        filter_write_answer(&self.state, namespace, collection, result).await
    }

    async fn patch_by_filter(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, PatchByFilterRequest>,
    ) -> ServiceResult<FilterWriteResponse> {
        let (namespace, collection) = collection_ref(request.namespace, request.collection)?;
        let filter = query(&request.filter, "filter")?.ok_or_else(|| {
            invalid(
                "filter",
                "patch_by_filter needs a filter; `match_all` patches everything",
            )
        })?;
        let Some(patch) = request.patch.as_option() else {
            return Err(invalid(
                "patch",
                "patch_by_filter needs the patch to apply to each match",
            ));
        };
        // The REST route's own parse, so the `id`/`upsert` refusal and the
        // three `mode` spellings are one implementation rather than two.
        let patch = patch_spec_from_json(&patch_json(patch)).map_err(refused_service)?;
        let cursor = cursor(&request.cursor)?;
        let options = filter_write(
            &ctx,
            &request.consistency,
            request.max_rows,
            request.allow_partial,
            cursor,
        )?;
        let result = self
            .state
            .collections
            .patch_by_filter(namespace, collection, filter, patch, options)
            .await;
        filter_write_answer(&self.state, namespace, collection, result).await
    }
}

// ----- Requests -----

/// A `WriteOp` as the REST route's op JSON, so `op_from_json` parses it.
///
/// The oneof spells the same three keys the REST route takes, and an op with no
/// arm set is JSON `null`, which `op_from_json` refuses with its own "an op
/// must be {"upsert": …}, …" message.
fn op_json(op: &WriteOpView<'_>) -> Value {
    use loams_proto::loams::collection::v1::__buffa::view::oneof::write_op::Op;
    match op.op.as_ref() {
        None => Value::Null,
        Some(Op::Upsert(document)) => json!({ "upsert": document_json(document) }),
        Some(Op::Delete(delete)) => {
            json!({ "delete": { "id": msg::json_of_document_id_view(&delete.id) } })
        }
        Some(Op::Patch(patch)) => json!({ "patch": patch_json(patch) }),
    }
}

/// A `WriteDocument` as the REST route's document JSON: the four keys
/// `doc_from_json` allows and nothing else.
///
/// An absent `id` is left out rather than sent as `null`, so the refusal is
/// `upsert.id is required` — the REST route's own message, from the REST
/// route's own check.
fn document_json(document: &WriteDocumentView<'_>) -> Value {
    let mut body = Map::new();
    if let Some(id) = document.id.as_option() {
        body.insert("id".to_string(), msg::json_of_document_id_view(id));
    }
    if let Some(source) = document.source.as_option() {
        body.insert("source".to_string(), msg::json_of_view(source));
    }
    body.insert(
        "vectors".to_string(),
        msg::json_of_values_view(&document.vectors),
    );
    body.insert(
        "sparse_vectors".to_string(),
        msg::json_of_structs_view(&document.sparse_vectors),
    );
    Value::Object(body)
}

/// A `Patch` as the REST route's patch JSON.
///
/// `vectors` and `sparse_vectors` are always present, so a patch's
/// `{"embedding": null}` arrives as the JSON `null` that
/// `patch_from_json` reads as "remove this vector" — which is the whole reason
/// the proto field is a `google.protobuf.Value`.
///
/// `upsert` is inserted **only when the caller sent it**, because its presence
/// is what `patch_spec_from_json` refuses on a filter write and what
/// `patch_from_json` reads on an op: a proto3 JSON `{"upsert": {}}` decodes to
/// a present-but-empty `WriteDocument`, which is exactly the request REST
/// refuses.
fn patch_json(patch: &PatchView<'_>) -> Value {
    let mut body = Map::new();
    if let Some(id) = patch.id.as_option() {
        body.insert("id".to_string(), msg::json_of_document_id_view(id));
    }
    if !patch.mode.is_empty() {
        body.insert("mode".to_string(), json!(patch.mode));
    }
    if let Some(source) = patch.source.as_option() {
        body.insert("source".to_string(), msg::json_of_view(source));
    }
    if !patch.delete_keys.is_empty() {
        body.insert(
            "delete_keys".to_string(),
            Value::Array(patch.delete_keys.iter().map(|key| json!(key)).collect()),
        );
    }
    body.insert(
        "vectors".to_string(),
        msg::json_of_values_view(&patch.vectors),
    );
    body.insert(
        "sparse_vectors".to_string(),
        msg::json_of_structs_view(&patch.sparse_vectors),
    );
    if let Some(upsert) = patch.upsert.as_option() {
        body.insert("upsert".to_string(), document_json(upsert));
    }
    Value::Object(body)
}

/// The projection a read's `select` asks for, or the REST route's default
/// (`everything`) when there is none.
fn projection(select: &MessageFieldView<StructView<'_>>) -> Result<Projection, ConnectError> {
    let Some(select) = select.as_option() else {
        return Ok(Projection::default());
    };
    serde_json::from_value(msg::json_of_view(select))
        .map_err(|err| invalid("select", format!("select is not a projection: {err}")))
}

/// The filter a read or a filter write asks for, through the REST route's own
/// parser: `filter` is the native query JSON until API1 Task 4 types it.
fn query(
    filter: &MessageFieldView<ValueView<'_>>,
    field: &str,
) -> Result<Option<Query>, ConnectError> {
    let Some(filter) = filter.as_option() else {
        return Ok(None);
    };
    serde_json::from_value(msg::json_of_value_view(filter))
        .map_err(|err| invalid(field, format!("{field} is not a query: {err}")))
}

/// The keys a get asks for, in order. Every id goes through the REST route's
/// own `from_json`, so an arm that is not a key it holds — a `uuid` that is not
/// a UUID — is the REST refusal with the REST message.
fn pks(ids: &buffa::RepeatedView<'_, DocumentIdView<'_>>) -> Result<Vec<PrimaryKey>, ConnectError> {
    ids.iter()
        .map(|id| json_pk::from_json(&msg::json_of_document_id_view(id)).map_err(refused_service))
        .collect()
}

/// The key a scroll continues after, or `None` for the first page.
fn pk_of(after: &MessageFieldView<DocumentIdView<'_>>) -> Result<Option<PrimaryKey>, ConnectError> {
    after
        .as_option()
        .map(|id| json_pk::from_json(&msg::json_of_document_id_view(id)).map_err(refused_service))
        .transpose()
}

/// The cursor that resumes a partial filter write, or `None` to start one.
fn cursor(
    cursor: &MessageFieldView<FilterWriteCursorView<'_>>,
) -> Result<Option<FilterWriteCursor>, ConnectError> {
    cursor
        .as_option()
        .map(msg::filter_write_cursor_of)
        .transpose()
        .map_err(refused_service)
}

/// A filter write's options: the same struct the REST route builds, from the
/// same two request headers.
fn filter_write(
    ctx: &RequestContext,
    consistency: &MessageFieldView<ConsistencyView<'_>>,
    max_rows: Option<u64>,
    allow_partial: bool,
    cursor: Option<FilterWriteCursor>,
) -> Result<FilterWriteOptions, ConnectError> {
    // The REST route passes `None` here because its body has no consistency
    // field; this one has a `Consistency` message, so it is passed through and
    // merged with the `loams-consistency-token` header by the same rule.
    let asked = match consistency.as_option() {
        None => None,
        Some(asked) => Some(asked_of(asked)?),
    };
    filter_write_options(ctx.headers(), asked, max_rows, allow_partial, cursor).map_err(refused)
}

/// The consistency a request asks for, as the native enum. A `pin` wins over
/// `at_least`, and `at_least` over `freshness`, because that is the order of
/// specificity a caller states them in.
fn asked_of(consistency: &ConsistencyView<'_>) -> Result<ReadConsistency, ConnectError> {
    if let Some(pin) = consistency.pin.as_option() {
        let token = pin.token.parse().map_err(|err| {
            invalid(
                "consistency.pin.token",
                format!("consistency.pin.token is not a consistency token: {err}"),
            )
        })?;
        return Ok(ReadConsistency::Pinned {
            manifest_version: pin.manifest_version,
            token,
        });
    }
    if !consistency.at_least.is_empty() {
        return consistency
            .at_least
            .parse()
            .map(ReadConsistency::AtLeast)
            .map_err(|err| {
                invalid(
                    "consistency.at_least",
                    format!("consistency.at_least is not a consistency token: {err}"),
                )
            });
    }
    Ok(match consistency.freshness {
        EnumValue::Known(pb::Freshness::FRESHNESS_EVENTUAL) => ReadConsistency::Eventual,
        // `FRESHNESS_UNSPECIFIED` is strong, and a value this build does not
        // know is read as the default rather than refused: an SDK generated
        // against a newer proto still gets the answer the server means.
        _ => ReadConsistency::Strong,
    })
}

// ----- Filter write answers -----

/// A filter write's answer: `FilterWriteResponse` plus the token header, a
/// refused batch carrying the backlog headers, and a call over its limit
/// carrying the numbers it refused on.
///
/// The three failure shapes are the REST route's `filter_write_answer`, and
/// this is its Connect half: a throttled write is a *refusal* here (a Connect
/// error with a code and a reason) where REST returns a `429` body it
/// constructs by hand.
async fn filter_write_answer(
    state: &AppState,
    namespace: &str,
    collection: &str,
    result: Result<FilterWriteResult, ServiceError>,
) -> ServiceResult<FilterWriteResponse> {
    match result {
        Ok(result) => {
            let token = result.token.to_string();
            Ok(Documents::read_answer(msg::filter_write(&result), &token))
        }
        Err(err @ ServiceError::ResourceExhausted { .. }) => {
            let backlog = state
                .collections
                .collection_backlog(namespace, collection)
                .await
                .unwrap_or_default();
            Err(refused_with(
                ApiError::from(err),
                Documents::backlog_headers(&backlog),
            ))
        }
        Err(err) => Err(refused(match over_limit(&err) {
            Some((matched, limit)) => ApiError::from(err)
                .with("matched", matched)
                .with("limit", limit),
            None => ApiError::from(err),
        })),
    }
}

// ----- The idempotency fingerprint -----

/// A hash of the parts of a write that make it *this* write: its ops and its
/// `report_existence` flag. Two requests with the same key and the same
/// fingerprint are the same request, and one with a different fingerprint is a
/// different write that must not replay.
///
/// The hash is `DefaultHasher`, which is deliberately not stable across Rust
/// releases: nothing persists it and nothing compares it across processes, and
/// a fingerprint that leaked into the wire would be a promise this server
/// cannot keep.
fn fingerprint(request: &WriteDocumentsRequestView<'_>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    request.report_existence.hash(&mut hasher);
    for op in request.ops.iter() {
        // The *canonical* JSON of the op: every object's keys sorted.
        canonical(&op_json(op)).to_string().hash(&mut hasher);
    }
    hasher.finish()
}

/// A JSON value with every object's keys in sorted order.
///
/// This workspace builds `serde_json` with `preserve_order` (qdrant-edge needs
/// it and Cargo unifies features), so `Value`'s own string form is the order
/// the decoder happened to produce — and a protobuf map field's order is not
/// something the wire promises. A fingerprint is taken over two byte-identical
/// requests and must not depend on that, so it is taken over this instead: the
/// same document written with its source keys in another order is the same
/// request, and two spellings of the same op are one write.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(fields) => {
            let mut entries: Vec<(&String, &Value)> = fields.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), canonical(value)))
                    .collect(),
            )
        }
        // An array is ordered on the wire, so its order is part of the request.
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// `loams.collection.v1.DocumentService`, registered on the router.
pub(super) fn register(router: connectrpc::Router, state: &AppState) -> connectrpc::Router {
    DocumentServiceExt::register(Documents::new(state.clone()), router)
}
