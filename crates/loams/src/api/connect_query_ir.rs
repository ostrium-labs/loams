//! The search IR as `loams.collection.v1` messages and back, in both
//! directions, and nothing else.
//!
//! Design §44 §4 and the API1 plan's "handlers are thin" rule:
//! [`super::connect_query`] calls the same `CollectionService::search` the REST
//! route calls, and *this* module is the only place that turns a
//! `QueryService` request message into the native REST JSON
//! `loams_query::json::hybrid::parse_query_body` reads, and a `SearchResponse`
//! back into the generated message. One mapping, in each direction, is what
//! makes the two surfaces provably the same behaviour rather than two
//! implementations that happen to agree today.
//!
//! ## The direction that matters: the request
//!
//! A request message becomes the REST route's **own JSON**, in the IR's
//! `snake_case` spelling, and `parse_query_body` reads it. Every field is
//! mapped into the key the REST route uses, so a filter the RPC accepts and the
//! REST route rejects is refused by the REST route's parser with the REST
//! route's message, and a field the mapping drops cannot silently disappear —
//! it is either emitted under the key the IR reads or refused here.
//!
//! The `Query` and `SortKey` arms are in [`super::connect_query_filters`],
//! which is this module's other half and is called from it: the retrievers carry
//! a `Query`, so the two cannot be separated without a cycle.
//!
//! ## The three rules proto3 JSON forces, and where each lands
//!
//! - **A `null` is an absent field.** Every `MessageFieldView` is tested with
//!   [`MessageFieldView::as_option`], and an absent optional scalar stays
//!   absent, so `"nprobes": null` on the REST fixture and no `nprobes` at all
//!   on the RPC are the same request. `optional` proto3 fields arrive as
//!   `Option<T>`, and the mapping only emits a key the caller actually sent.
//! - **A oneof arm is `{"arm": {…}}`, and a unit arm is `{"arm": {}}`.** The
//!   mapping reads the arm off the view's oneof and writes the IR's
//!   externally-tagged spelling, which for a unit variant is a **bare string**:
//!   `match_all` → `"match_all"`, `dbsf` → `"dbsf"`. That is the IR's spelling,
//!   not the wire's, and it is what makes the REST parser the one that runs.
//! - **A bare id is a `DocumentId`.** `Query::Ids` maps each `DocumentId` back
//!   through [`json_of_document_id_view`] — the same function
//!   `connect_documents` reads `GetDocumentsRequest.ids` through — so a
//!   `u64::MAX` id reaches `pk::from_json` as `18446744073709551615` and not
//!   through a `double`.
//!
//! ## The §05 §4 aliases
//!
//! `from`, `retrieve` and `fuse` are read here and mapped onto the IR fields
//! they alias, **before** the body is handed over, and the body is then always in
//! the IR's spelling: `from` becomes `collection` (with §05 §4's `collections.`
//! prefix stripped), `retrieve` becomes `retrievers`, and `fuse` becomes
//! `fusion` through [`loams_query::json::hybrid::fuse_from`] — the REST route's
//! own parse of the same `{"method": …}` object, so `rrf`, `dbsf` and `weighted`
//! are one implementation rather than two. A `collection` beats a `from` and
//! `retrievers` beats a `retrieve`, as `query.proto` documents.
//!
//! Passing the aliases through instead would **not** work, and the reason is
//! worth stating because it is the reason this module exists in this shape:
//! `parse_query_body` reads a body holding a `from` or a `retrieve` key as the
//! hybrid form, whose key set is shorter than the IR's and whose text retriever
//! takes `field` + a string `query` — not the typed `Query` §05 §4's RPC shape
//! already carries. Emitting the IR spelling makes every request on this RPC take
//! the IR path, which is the path the REST route takes for an IR body and the
//! only one `SearchRequest` itself is read by.
//!
//! ## The two M3 stages are read first, and refused
//!
//! `rerank` and `expand` are declared in `query.proto` precisely so this
//! module can see them: proto3 JSON ignores a field the message does not
//! declare, and a silently dropped stage is the one failure a caller cannot
//! detect. They are checked before anything else, and the refusal reuses
//! [`loams_query::json::hybrid`]'s own sentence, so both surfaces say the same
//! words.
//!
//! ## The answer
//!
//! `SearchResponse` → message is the other direction and is mechanical: the IR
//! values have JSON forms already (`json_pk::to_json` for a key, the `Value`
//! and `SortValue` serde for the rest), and each becomes the message field the
//! proto declares. It lives here rather than in a fourth module because it is
//! the short half of the mapping and shares the `Value` helpers the request
//! direction uses. `hot_used` is the one that is a **translation** rather than a
//! copy: `HotKind`'s two spellings (`hnsw`, `splits`) are the REST route's and
//! travel verbatim, and the set is empty exactly when the IR's is — which is
//! what makes proto3 JSON omit the field and the REST route omit it too.

use buffa::MessageField;
use buffa::enumeration::EnumValue;
use connectrpc::ConnectError;
use loams_proto::loams::collection::v1 as pb;
use loams_proto::loams::collection::v1::__buffa::view::oneof::fusion::Fusion as FusionArm;
use loams_proto::loams::collection::v1::__buffa::view::oneof::retriever::Retriever as RetrieverArm;
use loams_proto::loams::collection::v1::__buffa::view::{
    AnnParamsView, ConsistencyView, FusedQueryView, FusionView, GroupByView, HighlightView,
    RescoreQueryView, RetrieverView, RrfView, SearchRequestView, SparseQueryView, TextQueryView,
    VectorQueryView, WeightedSumView,
};
use loams_query::SortValue;
use loams_query::ir::TotalRelation as NativeTotalRelation;
use loams_query::json::hybrid::fuse_from;
use loams_query::json::hybrid::{EXPAND_UNAVAILABLE, RERANK_UNAVAILABLE};
use serde_json::{Map, Value, json};

use super::ApiError;
use super::connect_documents::projection;
use super::connect_errors::{invalid, refused};
use super::connect_query_filters as filters;

/// `SearchRequest` as the REST route's own JSON — the body
/// `POST /v1/namespaces/{ns}/query` takes — ready for
/// `parse_query_body`.
///
/// The namespace is not in the body: it is the REST route's path segment and
/// `SearchRequest`'s RPC field, and this mapping produces the body only.
pub(super) fn request_json(view: &SearchRequestView<'_>) -> Result<Value, ConnectError> {
    // The M3 stages first, before anything is read: a body carrying one is
    // refused whether or not the rest of it parses. See the module note.
    if view.rerank.as_option().is_some() {
        return Err(refused(ApiError::invalid(RERANK_UNAVAILABLE)));
    }
    if view.expand.as_option().is_some() {
        return Err(refused(ApiError::invalid(EXPAND_UNAVAILABLE)));
    }

    let mut body = Map::new();
    // Everything is emitted in the **IR's** spelling, never the §05 §4 one. That is
    // the whole reason §05 §4's aliases are read here rather than passed through:
    // the §05 §4 body names the collection `from` and the retrievers `retrieve`,
    // and `parse_query_body` reads a body holding either of those keys as the
    // hybrid form — which accepts a shorter key set than the IR and whose text
    // retriever wants `field` + a string `query` rather than the typed `Query`
    // §05 §4's RPC shape already carries. Folding the aliases into `collection`,
    // `retrievers` and `fusion` makes every request on this RPC take the IR path,
    // which is the path the REST route takes for an IR body and the one the whole
    // `SearchRequest` is read by.
    if let Some(collection) = collection_of(view) {
        body.insert("collection".to_string(), json!(collection));
    }
    if let Some(consistency) = view.consistency.as_option() {
        // §05 §4's `"consistency": "strong"`, the same message this package's
        // reads already take, mapped to the IR's spelling.
        body.insert(
            "consistency".to_string(),
            json!(consistency_json(consistency)?),
        );
    }

    // `retrievers` beats the §05 §4 alias `retrieve` (see `query.proto`), and
    // either way the key is the IR's.
    let retrievers = if !view.retrievers.is_empty() {
        &view.retrievers
    } else {
        &view.retrieve
    };
    let list = retrievers
        .iter()
        .map(retriever_json)
        .collect::<Result<Vec<_>, _>>()?;
    if !list.is_empty() {
        body.insert("retrievers".to_string(), Value::Array(list));
    }

    // `fusion` beats the §05 §4 alias `fuse`, and `fuse` is read by the REST
    // route's **own** `fuse` parser — so `rrf`, `dbsf` and `weighted` are one
    // implementation on both surfaces and an unknown method is refused with that
    // parser's own message.
    if let Some(fusion) = view.fusion.as_option() {
        body.insert("fusion".to_string(), fusion_json(fusion)?);
    } else if let Some(fuse) = view.fuse.as_option() {
        let fuse = super::connect_messages::json_of_view(fuse);
        let fusion = fuse_from(&fuse).map_err(|err| invalid("fuse", format!("fuse: {err}")))?;
        body.insert(
            "fusion".to_string(),
            serde_json::to_value(fusion)
                .map_err(|err| invalid("fuse", format!("fuse is not a fusion: {err}")))?,
        );
    }

    if let Some(filter) = view.filter.as_option() {
        body.insert("filter".to_string(), filters::query_json(filter)?);
    }
    let sort = view
        .sort
        .iter()
        .map(filters::sort_key_json)
        .collect::<Result<Vec<_>, _>>()?;
    if !sort.is_empty() {
        body.insert("sort".to_string(), Value::Array(sort));
    }
    if view.offset != 0 {
        body.insert("offset".to_string(), json!(view.offset));
    }
    if let Some(limit) = view.limit {
        body.insert("limit".to_string(), json!(limit));
    }
    let after = view
        .search_after
        .iter()
        .map(super::connect_messages::json_of_value_view)
        .collect::<Vec<_>>();
    if !after.is_empty() {
        body.insert("search_after".to_string(), Value::Array(after));
    }
    if let Some(threshold) = view.score_threshold {
        body.insert("score_threshold".to_string(), json!(threshold));
    }
    if let Some(select) = view.select.as_option() {
        // Read through the one projection parse the package shares
        // (`connect_documents::projection`), so `select` cannot be spelled two
        // ways. §05 §4's `["id", "_score", "body"]` is the *REST* route's
        // projection spelling and lands on the same `Projection` through the
        // same parser.
        let select = projection(super::connect_messages::json_of_view(select))?;
        body.insert(
            "select".to_string(),
            serde_json::to_value(select)
                .map_err(|err| invalid("select", format!("select is not a projection: {err}")))?,
        );
    }
    if let Some(aggregations) = view.aggregations.as_option() {
        body.insert(
            "aggregations".to_string(),
            super::connect_messages::json_of_value_view(aggregations),
        );
    }
    if let Some(highlight) = view.highlight.as_option() {
        body.insert("highlight".to_string(), highlight_json(highlight)?);
    }
    if let Some(group_by) = view.group_by.as_option() {
        body.insert("group_by".to_string(), group_by_json(group_by));
    }
    if let Some(track) = view.track_total_hits.as_option() {
        body.insert(
            "track_total_hits".to_string(),
            track_total_hits_json(super::connect_messages::json_of_value_view(track))?,
        );
    }
    Ok(Value::Object(body))
}

/// The collection a request names: `collection`, or the §05 §4 `from` with its
/// `collections.` prefix stripped (the REST route's own rule).
///
/// `None` when neither names one, which `parse_query_body` refuses with its
/// "collection" message rather than this one guessing.
fn collection_of(view: &SearchRequestView<'_>) -> Option<String> {
    let named = |name: &str| {
        name.strip_prefix("collections.")
            .unwrap_or(name)
            .trim()
            .to_string()
    };
    if !view.collection.is_empty() {
        Some(named(view.collection))
    } else if !view.from.is_empty() {
        Some(named(view.from))
    } else {
        None
    }
}

// ----- `Consistency` -----

/// A `Consistency` message as the IR's `ReadConsistency` JSON, through the same
/// `ReadConsistency` the document RPCs build (`connect_documents::asked_of`).
fn consistency_json(consistency: &ConsistencyView<'_>) -> Result<Value, ConnectError> {
    // Through the same `ReadConsistency` `connect_documents` builds for
    // `GetDocumentsRequest.consistency` (Task 3 defined that message, in this
    // package), so one message cannot mean two things across the package.
    serde_json::to_value(super::connect_documents::asked_of(consistency)?)
        .map_err(|err| invalid("consistency", format!("consistency is not readable: {err}")))
}

// ----- `Retriever` -----

fn retriever_json(view: &RetrieverView<'_>) -> Result<Value, ConnectError> {
    let Some(arm) = view.retriever.as_ref() else {
        // A retriever with no arm is `null`, which the REST route's
        // `retriever_from` refuses with "a retriever must be an object with
        // one key". The same sentence, from the same parser.
        return Ok(Value::Null);
    };
    let (arm, body) = match arm {
        RetrieverArm::Vector(v) => ("vector", vector_json(v)?),
        RetrieverArm::Text(v) => ("text", text_json(v)?),
        RetrieverArm::Fused(v) => ("fused", fused_json(v)?),
        RetrieverArm::Rescore(v) => ("rescore", rescore_json(v)?),
        RetrieverArm::Sparse(v) => ("sparse", sparse_json(v)?),
    };
    Ok(json!({ arm: body }))
}

fn vector_json(view: &VectorQueryView<'_>) -> Result<Value, ConnectError> {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    body.insert("query".to_string(), floats(view.query.iter()));
    body.insert("k".to_string(), json!(view.k));
    if let Some(params) = view.params.as_option() {
        body.insert("params".to_string(), ann_params_json(params));
    }
    if let Some(filter) = view.filter.as_option() {
        body.insert("filter".to_string(), filters::query_json(filter)?);
    }
    Ok(Value::Object(body))
}

fn text_json(view: &TextQueryView<'_>) -> Result<Value, ConnectError> {
    let mut body = Map::new();
    // A text retriever's `query` is the IR's own `Query`, which is what
    // §05 §4's `field` + string `query` *is*: `connect_query`'s caller has
    // already turned that shorthand into a `match`.
    match view.query.as_option() {
        Some(query) => {
            body.insert("query".to_string(), filters::query_json(query)?);
        }
        None => {
            return Err(invalid(
                "retrieve.text.query",
                "a text retriever needs a query",
            ));
        }
    }
    body.insert("k".to_string(), json!(view.k));
    Ok(Value::Object(body))
}

fn fused_json(view: &FusedQueryView<'_>) -> Result<Value, ConnectError> {
    let inputs = view
        .inputs
        .iter()
        .map(retriever_json)
        .collect::<Result<Vec<_>, _>>()?;
    let mut body = Map::new();
    body.insert("inputs".to_string(), Value::Array(inputs));
    if let Some(fusion) = view.fusion.as_option() {
        body.insert("fusion".to_string(), fusion_json(fusion)?);
    }
    body.insert("k".to_string(), json!(view.k));
    Ok(Value::Object(body))
}

fn rescore_json(view: &RescoreQueryView<'_>) -> Result<Value, ConnectError> {
    let mut body = Map::new();
    match view.input.as_option() {
        Some(input) => {
            body.insert("input".to_string(), retriever_json(input)?);
        }
        None => {
            return Err(invalid(
                "retrieve.rescore.input",
                "a rescore needs the retriever whose candidates it re-ranks",
            ));
        }
    }
    body.insert("field".to_string(), json!(view.field));
    body.insert("query".to_string(), floats(view.query.iter()));
    body.insert("k".to_string(), json!(view.k));
    Ok(Value::Object(body))
}

fn sparse_json(view: &SparseQueryView<'_>) -> Result<Value, ConnectError> {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    match view.query.as_option() {
        Some(query) => {
            body.insert(
                "query".to_string(),
                super::connect_messages::json_of_view(query),
            );
        }
        None => {
            return Err(invalid(
                "retrieve.sparse.query",
                "a sparse retriever needs a query vector",
            ));
        }
    }
    body.insert("k".to_string(), json!(view.k));
    if let Some(filter) = view.filter.as_option() {
        body.insert("filter".to_string(), filters::query_json(filter)?);
    }
    if let Some(params) = view.params.as_option() {
        let mut sparse = Map::new();
        if let Some(corpus) = params.idf_corpus.as_option() {
            sparse.insert("idf_corpus".to_string(), filters::query_json(corpus)?);
        }
        body.insert("params".to_string(), Value::Object(sparse));
    }
    Ok(Value::Object(body))
}

/// A dense retriever's parameters, in the IR's spelling. Every key the caller
/// did not send is left out, so an absent `nprobes` is the IR's `None` and not
/// a zero.
fn ann_params_json(view: &AnnParamsView<'_>) -> Value {
    let mut params = Map::new();
    if view.exact {
        params.insert("exact".to_string(), json!(true));
    }
    if let Some(nprobes) = view.nprobes {
        params.insert("nprobes".to_string(), json!(nprobes));
    }
    if let Some(refine) = view.refine_factor {
        params.insert("refine_factor".to_string(), json!(refine));
    }
    if let Some(ef) = view.ef {
        params.insert("ef".to_string(), json!(ef));
    }
    if let Some(oversampling) = view.oversampling {
        params.insert("oversampling".to_string(), json!(oversampling));
    }
    if !view.distance.is_empty() {
        params.insert("distance".to_string(), json!(view.distance));
    }
    Value::Object(params)
}

// ----- `Fusion` -----

fn fusion_json(view: &FusionView<'_>) -> Result<Value, ConnectError> {
    let Some(arm) = view.fusion.as_ref() else {
        return Err(invalid("fusion", "a fusion must name a method"));
    };
    let (arm, body) = match arm {
        FusionArm::Rrf(rrf) => ("rrf", rrf_json(rrf)),
        FusionArm::Dbsf(_) => {
            // A unit arm is the IR's bare string, which is what the REST route
            // reads for `Fusion::Dbsf`.
            return Ok(json!("dbsf"));
        }
        FusionArm::WeightedSum(weights) => ("weighted_sum", weighted_sum_json(weights)),
    };
    Ok(json!({ arm: body }))
}

fn rrf_json(view: &RrfView<'_>) -> Value {
    let mut body = Map::new();
    // `optional`, so an absent `k` is the IR's default of 60 rather than a
    // zero the engine would read as "no smoothing".
    if let Some(k) = view.k {
        body.insert("k".to_string(), json!(k));
    }
    Value::Object(body)
}

fn weighted_sum_json(view: &WeightedSumView<'_>) -> Value {
    let mut body = Map::new();
    body.insert("weights".to_string(), floats(view.weights.iter()));
    Value::Object(body)
}

// ----- `Highlight`, `GroupBy`, `TrackTotalHits` -----

fn highlight_json(view: &HighlightView<'_>) -> Result<Value, ConnectError> {
    let mut fields = Vec::with_capacity(view.fields.len());
    for field in view.fields.iter() {
        if field.field.is_empty() {
            return Err(invalid(
                "highlight.fields.field",
                "a highlighted field must be named",
            ));
        }
        let mut body = Map::new();
        body.insert("field".to_string(), json!(field.field));
        // Every tag and size is `optional` on the wire so an absent one is the
        // IR's default (`<em>`, `</em>`, 100, 5) rather than an empty string.
        // The three shapes are mapped separately rather than through one array
        // of pairs, because `&str` and `Option<u32>` have nothing in common to
        // put in one tuple.
        if let Some(pre) = field.pre_tag {
            body.insert("pre_tag".to_string(), json!(pre));
        }
        if let Some(post) = field.post_tag {
            body.insert("post_tag".to_string(), json!(post));
        }
        if let Some(size) = field.fragment_size {
            body.insert("fragment_size".to_string(), json!(size));
        }
        if let Some(fragments) = field.number_of_fragments {
            body.insert("number_of_fragments".to_string(), json!(fragments));
        }
        fields.push(Value::Object(body));
    }
    Ok(json!({ "fields": fields }))
}

fn group_by_json(view: &GroupByView<'_>) -> Value {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    if let Some(size) = view.group_size {
        body.insert("group_size".to_string(), json!(size));
    }
    if let Some(limit) = view.limit {
        body.insert("limit".to_string(), json!(limit));
    }
    Value::Object(body)
}

/// A `track_total_hits` `Value` as the IR's `TrackTotalHits`.
///
/// The field is a `Value` because the IR's spelling is a serde externally
/// tagged enum — `"none"`, `"exact"`, `{"up_to": 5}` — while this proto's
/// oneof spelling is `{"exact": {}}`. Both are accepted, and so is the
/// proto3-JSON object form `{"upTo": 5}`, so a body written against either
/// spelling means the same thing. Anything else is refused here rather than
/// being read as "count nothing".
fn track_total_hits_json(value: Value) -> Result<Value, ConnectError> {
    let refused = || {
        invalid(
            "track_total_hits",
            "track_total_hits must be \"none\", \"exact\" or {\"upTo\": n}",
        )
    };
    Ok(match &value {
        Value::String(name) => match name.as_str() {
            "none" => json!("none"),
            "exact" => json!("exact"),
            _ => return Err(refused()),
        },
        Value::Object(fields) => {
            let mut entries = fields.iter();
            let (key, value) = entries.next().ok_or_else(refused)?;
            if entries.next().is_some() {
                return Err(refused());
            }
            match key.as_str() {
                "none" => json!("none"),
                "exact" => json!("exact"),
                "upTo" | "up_to" => {
                    let limit = value
                        .as_u64()
                        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
                        .ok_or_else(refused)?;
                    json!({ "up_to": limit })
                }
                _ => return Err(refused()),
            }
        }
        _ => return Err(refused()),
    })
}

// ----- The answer -----

/// A native `SearchResponse` as `loams.collection.v1.SearchResponse`.
pub(super) fn response_json(response: &loams_query::SearchResponse) -> pb::SearchResponse {
    pb::SearchResponse {
        hits: response.hits.iter().map(hit_message).collect(),
        total: response.total.map_or_else(MessageField::none, |total| {
            MessageField::some(pb::TotalHits {
                // `optional` and always set, for the reason
                // `TotalHits.value` gives: a count of zero is a count, and
                // proto3 JSON omitting it would read as "unanswered".
                value: Some(total.value),
                relation: EnumValue::Known(match total.relation {
                    NativeTotalRelation::Eq => pb::TotalRelation::TOTAL_RELATION_EQ,
                    NativeTotalRelation::Gte => pb::TotalRelation::TOTAL_RELATION_GTE,
                }),
                ..Default::default()
            })
        }),
        aggregations: response
            .aggregations
            .clone()
            .map_or_else(MessageField::none, |value| {
                MessageField::some(super::connect_messages::proto_value(&value))
            }),
        groups: response
            .groups
            .as_ref()
            .map(|groups| groups.iter().map(hit_group_message).collect())
            .unwrap_or_default(),
        read_token: response.read_token.to_string(),
        // The REST route's own two spellings, in `HotKind` order, and empty
        // exactly when the IR's set is — which is what makes proto3 JSON omit
        // the field, as the REST route omits it too.
        hot_used: response
            .hot_used
            .iter()
            .map(|kind| kind.name().to_string())
            .collect(),
        // Named by design (§05 §4) and computed by nothing yet, on either
        // surface. See `query.proto`.
        performance: MessageField::none(),
        ..Default::default()
    }
}

fn hit_message(hit: &loams_query::Hit) -> pb::Hit {
    pb::Hit {
        pk: MessageField::some(super::connect_messages::document_id(&hit.pk)),
        score: hit.score,
        sort_values: hit
            .sort_values
            .iter()
            .map(sort_value)
            .map(|value| super::connect_messages::proto_value(&value))
            .collect(),
        source: hit
            .source
            .as_ref()
            .map_or_else(MessageField::none, |source| {
                MessageField::some(
                    super::connect_messages::struct_of(&Value::Object(source.clone()))
                        .expect("a source is a Struct"),
                )
            }),
        // The dense vectors, the same `map<string, google.protobuf.Value>` a
        // `Document`'s are, so a hit answers them exactly as a get does.
        vectors: super::connect_messages::proto_named(&hit.vectors),
        sparse_vectors: super::connect_messages::proto_sparse(&hit.sparse_vectors),
        highlight: super::connect_messages::proto_named(&hit.highlight),
        fields: super::connect_messages::proto_named(&hit.fields),
        ..Default::default()
    }
}

fn hit_group_message(group: &loams_query::HitGroup) -> pb::HitGroup {
    pb::HitGroup {
        key: MessageField::some(super::connect_messages::proto_value(
            &serde_json::to_value(&group.key).unwrap_or(Value::Null),
        )),
        hits: group.hits.iter().map(hit_message).collect(),
        ..Default::default()
    }
}

/// A `SortValue` as the IR's own JSON, which `proto_value` then carries.
///
/// The IR's `SortValue` has a hand-written serde ([`loams_query::json::values`])
/// rather than a derive, so going through it is what makes a hit's `sortValues`
/// the REST route's spelling — a `uuid` as `{"uuid": "…"}` and an integer as an
/// integer — rather than a second rendering of the same value.
fn sort_value(value: &SortValue) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// A `repeated float` field as the IR's JSON list of numbers.
fn floats<'a>(values: impl Iterator<Item = &'a f32>) -> Value {
    Value::Array(values.map(|value| json!(value)).collect())
}
