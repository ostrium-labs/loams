//! `Query` and `SortKey` as `loams.collection.v1` messages and back: the filter
//! IR of `crates/loams-query/src/ir.rs`, in the IR's own `snake_case` JSON, which
//! is what `loams_query::json::hybrid::parse_query_body` reads.
//!
//! This is [`super::connect_query_ir`]'s other half rather than a module of its
//! own concern: a retriever's `filter` is a `Query` and a sparse retriever's
//! `idf_corpus` is one too, so the request mapping calls into here and the two
//! would be a cycle if this were independent. Split by *size*, not by direction —
//! and the size limit is the reason: the whole mapping is one file's worth of
//! work and two files under the 1000-line rule.
//!
//! ## The same three rules, from `query.proto`'s header
//!
//! - **A `null` is an absent field.** Every optional arm field is an `Option<T>`
//!   and a range bound is a `MessageFieldView`, tested with `as_option`, so a
//!   bound the caller did not send stays absent and the IR's unbounded side is
//!   unbounded.
//! - **A oneof arm is `{"arm": {…}}`, and a unit arm is `{"arm": {}}` on the
//!   wire** — which becomes the IR's **bare string**: `match_all` →
//!   `"match_all"`, `dbsf` → `"dbsf"`. That is what makes the REST parser, not
//!   this mapping, the thing that reads the result.
//! - **A bare id is a `DocumentId`.** `Query.ids` is a `repeated
//!   google.protobuf.Value` carrying the `DocumentId` shape, because a
//!   `repeated` field cannot be a oneof arm in protobuf at all and the bare-list
//!   spelling is the only one the IR has. [`id_json`] maps each element back to
//!   the REST route's bare key, and `pk::from_json` — the function
//!   `GetDocumentsRequest.ids` goes through — reads it.

use buffa::MessageFieldView;
use buffa::enumeration::EnumValue;
use connectrpc::ConnectError;
use loams_proto::google::protobuf::__buffa::view::ValueView;
use loams_proto::loams::collection::v1 as pb;
use loams_proto::loams::collection::v1::__buffa::view::oneof::fuzziness::Fuzziness as FuzzinessArm;
use loams_proto::loams::collection::v1::__buffa::view::oneof::query::Query as QueryArm;
use loams_proto::loams::collection::v1::__buffa::view::oneof::sort_key::Key as SortKeyArm;
use loams_proto::loams::collection::v1::__buffa::view::{
    BoolQueryView, BoostQueryView, ConstantScoreQueryView, FieldQueryView, FieldSortView,
    FuzzinessView, FuzzyQueryView, MatchPhraseQueryView, MatchQueryView, MultiMatchFieldView,
    MultiMatchQueryView, PrefixQueryView, PrimaryKeySortView, QueryStringQueryView, QueryView,
    RangeQueryView, ScoreSortView, SortKeyView, TermQueryView, TermsQueryView,
    ValuesCountQueryView, WildcardQueryView,
};
use serde_json::{Map, Value, json};

use super::connect_errors::invalid;

// ----- `Query` -----

/// A `Query` — a filter, a text retriever's query, or a sparse retriever's IDF
/// corpus — as the IR's own JSON.
pub(super) fn query_json(view: &QueryView<'_>) -> Result<Value, ConnectError> {
    // `ids` is a plain field rather than a oneof arm (see `query.proto`), so it
    // is read here. A request that sets both it and an arm is refused rather than
    // letting one silently win: two keys naming two different filters is a
    // request that cannot be answered.
    let has_ids = !view.ids.is_empty();
    let Some(arm) = view.query.as_ref() else {
        if has_ids {
            // The IR's `Query::Ids` and nothing else.
            return Ok(json!({
                "ids": view
                    .ids
                    .iter()
                    .map(super::connect_messages::json_of_value_view)
                    .map(id_json)
                    .collect::<Result<Vec<_>, _>>()?
            }));
        }
        // A `Query` with no arm is the `Document`-with-no-`id` case: proto3 JSON
        // has no `null` for a message field, so `{}` is how an absent arm
        // arrives. It is refused rather than read as "match everything" — a
        // filter that silently degrades to no filter is the one failure this
        // module cannot allow.
        return Err(invalid("filter", "unknown filter {}: name one query"));
    };
    if has_ids {
        return Err(invalid(
            "filter",
            "a filter names one query; ids and another arm are two",
        ));
    }
    let (name, body) = match arm {
        // The two unit arms are the IR's bare strings.
        QueryArm::MatchAll(_) => return Ok(json!("match_all")),
        QueryArm::MatchNone(_) => return Ok(json!("match_none")),
        QueryArm::Match(v) => ("match", Some(match_json(v)?)),
        QueryArm::MatchPhrase(v) => ("match_phrase", Some(match_phrase_json(v))),
        QueryArm::MultiMatch(v) => ("multi_match", Some(multi_match_json(v)?)),
        QueryArm::Term(v) => ("term", Some(term_json(v))),
        QueryArm::Terms(v) => ("terms", Some(terms_json(v))),
        QueryArm::Range(v) => ("range", Some(range_json(v))),
        QueryArm::Exists(v) => ("exists", Some(field_json(v))),
        QueryArm::IsNull(v) => ("is_null", Some(field_json(v))),
        QueryArm::IsEmpty(v) => ("is_empty", Some(field_json(v))),
        QueryArm::ValuesCount(v) => ("values_count", Some(values_count_json(v))),
        QueryArm::Prefix(v) => ("prefix", Some(prefix_json(v))),
        QueryArm::Wildcard(v) => ("wildcard", Some(wildcard_json(v))),
        QueryArm::Fuzzy(v) => ("fuzzy", Some(fuzzy_json(v)?)),
        QueryArm::QueryString(v) => ("query_string", Some(query_string_json(v))),
        QueryArm::Bool(v) => ("bool", Some(bool_json(v)?)),
        QueryArm::Boost(v) => ("boost", Some(boost_json(v)?)),
        QueryArm::ConstantScore(v) => ("constant_score", Some(constant_score_json(v)?)),
    };
    Ok(json!({ name: body }))
}

fn match_json(view: &MatchQueryView<'_>) -> Result<Value, ConnectError> {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    body.insert("text".to_string(), json!(view.text));
    // `operator` is a string in `query.proto`, carrying the IR's own `or` /
    // `and` (see that field's comment). An absent one is the IR's default, so
    // it is left out rather than written.
    if !view.operator.is_empty() {
        let operator = match view.operator {
            "or" | "and" => view.operator,
            other => {
                return Err(invalid(
                    "filter.match.operator",
                    format!("unknown match operator {other}: expected or or and"),
                ));
            }
        };
        body.insert("operator".to_string(), json!(operator));
    }
    if let Some(minimum) = view.minimum_should_match {
        body.insert("minimum_should_match".to_string(), json!(minimum));
    }
    if let Some(fuzziness) = view.fuzziness.as_option() {
        body.insert("fuzziness".to_string(), fuzziness_json(fuzziness)?);
    }
    if !view.analyzer.is_empty() {
        body.insert("analyzer".to_string(), json!(view.analyzer));
    }
    Ok(Value::Object(body))
}

fn match_phrase_json(view: &MatchPhraseQueryView<'_>) -> Value {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    body.insert("text".to_string(), json!(view.text));
    if view.slop != 0 {
        body.insert("slop".to_string(), json!(view.slop));
    }
    Value::Object(body)
}

fn multi_match_json(view: &MultiMatchQueryView<'_>) -> Result<Value, ConnectError> {
    let mut body = Map::new();
    // The IR's `fields` is `Vec<(String, f32)>`, which its serde spells as a
    // list of two-element lists.
    let fields = view
        .fields
        .iter()
        .map(|f: &MultiMatchFieldView<'_>| {
            let (name, boost) = (f.field, f.boost);
            json!([name, boost])
        })
        .collect::<Vec<_>>();
    if !fields.is_empty() {
        body.insert("fields".to_string(), Value::Array(fields));
    }
    body.insert("text".to_string(), json!(view.text));
    body.insert("kind".to_string(), json!(multi_match_kind(view.kind)));
    body.insert("operator".to_string(), json!(bool_operator(view.operator)));
    if let Some(tie) = view.tie_breaker {
        body.insert("tie_breaker".to_string(), json!(tie));
    }
    Ok(Value::Object(body))
}

fn term_json(view: &TermQueryView<'_>) -> Value {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    let value = view.value.as_option().map_or(Value::Null, |v| {
        super::connect_messages::json_of_value_view(v)
    });
    body.insert("value".to_string(), value);
    Value::Object(body)
}

fn terms_json(view: &TermsQueryView<'_>) -> Value {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    let values = view
        .values
        .iter()
        .map(super::connect_messages::json_of_value_view)
        .collect::<Vec<_>>();
    body.insert("values".to_string(), Value::Array(values));
    Value::Object(body)
}

fn range_json(view: &RangeQueryView<'_>) -> Value {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    // Each bound is `optional` on the wire, so an absent bound is the IR's
    // `None` and an unbounded side stays unbounded.
    let bound = |message: &MessageFieldView<ValueView<'_>>| {
        message
            .as_option()
            .map(super::connect_messages::json_of_value_view)
    };
    for (key, value) in [
        ("gt", bound(&view.gt)),
        ("gte", bound(&view.gte)),
        ("lt", bound(&view.lt)),
        ("lte", bound(&view.lte)),
    ] {
        if let Some(value) = value {
            body.insert(key.to_string(), value);
        }
    }
    Value::Object(body)
}

fn field_json(view: &FieldQueryView<'_>) -> Value {
    json!({ "field": view.field })
}

fn values_count_json(view: &ValuesCountQueryView<'_>) -> Value {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    for (key, value) in [
        ("gt", view.gt),
        ("gte", view.gte),
        ("lt", view.lt),
        ("lte", view.lte),
    ] {
        if let Some(value) = value {
            body.insert(key.to_string(), json!(value));
        }
    }
    Value::Object(body)
}

fn prefix_json(view: &PrefixQueryView<'_>) -> Value {
    json!({ "field": view.field, "value": view.value })
}

fn wildcard_json(view: &WildcardQueryView<'_>) -> Value {
    json!({ "field": view.field, "pattern": view.pattern })
}

fn fuzzy_json(view: &FuzzyQueryView<'_>) -> Result<Value, ConnectError> {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    body.insert("value".to_string(), json!(view.value));
    match view.fuzziness.as_option() {
        Some(fuzziness) => {
            body.insert("fuzziness".to_string(), fuzziness_json(fuzziness)?);
        }
        None => {
            return Err(invalid(
                "filter.fuzzy.fuzziness",
                "a fuzzy match needs how far it may edit",
            ));
        }
    }
    Ok(Value::Object(body))
}

/// One `Query.ids` element — a `google.protobuf.Value` carrying the
/// `DocumentId` shape — as the REST route's **bare** id.
///
/// The REST spelling of a key is a bare `1` or a bare `"k-str"`, which has no
/// oneof form on the wire, so this is where the two meet. The value is
/// materialised first rather than pattern-matched on the view, so what is read
/// here is exactly what `pk::from_json` is about to read.
///
/// The three arms are total and spelled as `DocumentId` spells them; anything
/// else is refused here, with the same message `pk::from_json` would produce for
/// the same JSON, so a bad id is one error on both surfaces rather than two.
fn id_json(value: Value) -> Result<Value, ConnectError> {
    let refused = || {
        invalid(
            "filter.ids",
            format!(
                "invalid id {value}: expected an unsigned integer, a string or {{\"uuid\": \"…\"}}"
            ),
        )
    };
    // One arm, exactly as `pk::from_json` requires of the REST spelling's object
    // form. Anything else — a bare number here, a two-key object, no object at
    // all — is that function's refusal, said here.
    let Some(arms) = value.as_object().filter(|arms| arms.len() == 1) else {
        return Err(refused());
    };
    let (arm, inner) = arms.iter().next().expect("one key");
    match (arm.as_str(), inner) {
        // A `uint` arrives as the decimal **string** proto3 JSON requires of a
        // 64-bit integer, which is what keeps `u64::MAX` exact. The REST bare
        // form is the number, so it is parsed here rather than handed on for
        // `pk::from_json` to refuse as an object.
        ("uint", Value::String(text)) => {
            text.parse::<u64>()
                .map(|number| json!(number))
                .map_err(|_| {
                    invalid(
                        "filter.ids",
                        format!("invalid id {value}: uint is not an unsigned 64-bit integer"),
                    )
                })
        }
        ("string", Value::String(text)) => Ok(json!(text)),
        ("uuid", Value::String(text)) => Ok(json!({ "uuid": text })),
        _ => Err(refused()),
    }
}

fn query_string_json(view: &QueryStringQueryView<'_>) -> Value {
    let mut body = Map::new();
    body.insert("query".to_string(), json!(view.query));
    let fields = view
        .default_fields
        .iter()
        .map(|name| json!(name))
        .collect::<Vec<_>>();
    if !fields.is_empty() {
        body.insert("default_fields".to_string(), Value::Array(fields));
    }
    body.insert(
        "default_operator".to_string(),
        json!(bool_operator(view.default_operator)),
    );
    Value::Object(body)
}

fn bool_json(view: &BoolQueryView<'_>) -> Result<Value, ConnectError> {
    let mut body = Map::new();
    for (key, arm) in [
        ("must", &view.must),
        ("should", &view.should),
        ("must_not", &view.must_not),
        ("filter", &view.filter),
    ] {
        if arm.is_empty() {
            continue;
        }
        let queries = arm.iter().map(query_json).collect::<Result<Vec<_>, _>>()?;
        body.insert(key.to_string(), Value::Array(queries));
    }
    if let Some(minimum) = view.minimum_should_match {
        body.insert("minimum_should_match".to_string(), json!(minimum));
    }
    Ok(Value::Object(body))
}

fn boost_json(view: &BoostQueryView<'_>) -> Result<Value, ConnectError> {
    let mut body = Map::new();
    match view.query.as_option() {
        Some(query) => {
            body.insert("query".to_string(), query_json(query)?);
        }
        None => {
            return Err(invalid("filter.boost.query", "a boost needs a query"));
        }
    }
    body.insert("boost".to_string(), json!(view.boost));
    Ok(Value::Object(body))
}

fn constant_score_json(view: &ConstantScoreQueryView<'_>) -> Result<Value, ConnectError> {
    let mut body = Map::new();
    match view.query.as_option() {
        Some(query) => {
            body.insert("query".to_string(), query_json(query)?);
        }
        None => {
            return Err(invalid(
                "filter.constant_score.query",
                "a constant score needs a query",
            ));
        }
    }
    body.insert("score".to_string(), json!(view.score));
    Ok(Value::Object(body))
}

fn fuzziness_json(view: &FuzzinessView<'_>) -> Result<Value, ConnectError> {
    match view.fuzziness.as_ref() {
        Some(FuzzinessArm::Auto(_)) => Ok(json!("auto")),
        Some(FuzzinessArm::Edits(n)) => Ok(json!(n)),
        None => Err(invalid(
            "fuzziness",
            "a fuzziness must be \"auto\" or a number of edits",
        )),
    }
}

/// The IR's `BoolOperator` spelling. `BOOL_OPERATOR_UNSPECIFIED` is `or`,
/// which is that enum's default and what an absent field means.
fn bool_operator(value: EnumValue<pb::BoolOperator>) -> &'static str {
    match value {
        EnumValue::Known(pb::BoolOperator::BOOL_OPERATOR_AND) => "and",
        _ => "or",
    }
}

/// The IR's `MultiMatchKind` spelling; unspecified is `best_fields`, its
/// default.
fn multi_match_kind(value: EnumValue<pb::MultiMatchKind>) -> &'static str {
    match value {
        EnumValue::Known(pb::MultiMatchKind::MULTI_MATCH_KIND_MOST_FIELDS) => "most_fields",
        EnumValue::Known(pb::MultiMatchKind::MULTI_MATCH_KIND_CROSS_FIELDS) => "cross_fields",
        EnumValue::Known(pb::MultiMatchKind::MULTI_MATCH_KIND_PHRASE) => "phrase",
        EnumValue::Known(pb::MultiMatchKind::MULTI_MATCH_KIND_PHRASE_PREFIX) => "phrase_prefix",
        _ => "best_fields",
    }
}

// ----- `SortKey` -----

/// A `SortKey` as the IR's own JSON. The primary key ascending always breaks
/// ties, which is the engine's rule (Ruling 10) and not this message's.
pub(super) fn sort_key_json(view: &SortKeyView<'_>) -> Result<Value, ConnectError> {
    let Some(arm) = view.key.as_ref() else {
        return Err(invalid("sort", "a sort key must name what it sorts by"));
    };
    let (name, body) = match arm {
        // An absent `order` is each key's own default (desc for a score, asc for
        // a key and a field), so it is left out rather than written as the
        // proto3 default.
        SortKeyArm::Score(v) => ("score", score_sort_json(v)),
        SortKeyArm::Pk(v) => ("pk", primary_key_sort_json(v)),
        SortKeyArm::Field(v) => ("field", field_sort_json(v)),
    };
    Ok(json!({ name: body }))
}

fn score_sort_json(view: &ScoreSortView<'_>) -> Value {
    let mut body = Map::new();
    if let Some(order) = sort_order(view.order) {
        body.insert("order".to_string(), json!(order));
    }
    Value::Object(body)
}

fn primary_key_sort_json(view: &PrimaryKeySortView<'_>) -> Value {
    let mut body = Map::new();
    if let Some(order) = sort_order(view.order) {
        body.insert("order".to_string(), json!(order));
    }
    Value::Object(body)
}

fn field_sort_json(view: &FieldSortView<'_>) -> Value {
    let mut body = Map::new();
    body.insert("field".to_string(), json!(view.field));
    if let Some(order) = sort_order(view.order) {
        body.insert("order".to_string(), json!(order));
    }
    if let Some(missing) = missing_order(view.missing) {
        body.insert("missing".to_string(), json!(missing));
    }
    Value::Object(body)
}

fn sort_order(value: EnumValue<pb::SortOrder>) -> Option<&'static str> {
    match value {
        EnumValue::Known(pb::SortOrder::SORT_ORDER_ASC) => Some("asc"),
        EnumValue::Known(pb::SortOrder::SORT_ORDER_DESC) => Some("desc"),
        _ => None,
    }
}

fn missing_order(value: EnumValue<pb::MissingOrder>) -> Option<&'static str> {
    match value {
        EnumValue::Known(pb::MissingOrder::MISSING_ORDER_FIRST) => Some("first"),
        EnumValue::Known(pb::MissingOrder::MISSING_ORDER_LAST) => Some("last"),
        _ => None,
    }
}
