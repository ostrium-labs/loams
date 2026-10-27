//! The filter builder (SDKP §2.4): one constructor per arm of
//! `loams.collection.v1.Query`.
//!
//! ```rust
//! use loams::builders::Filter;
//! let filter = Filter::and([Filter::eq("tags", "policy"), Filter::range("n").gte(2).build()]);
//! assert_eq!(filter.to_json()["bool"]["must"][0]["term"]["field"], "tags");
//! ```
//!
//! A [`Filter`] is the arm's proto3 JSON; [`Filter::to_query`] decodes it into
//! the generated `Query`.

use serde_json::{Map, Value, json};

use crate::error::LoamsError;
use loams_proto::loams::collection::v1::Query;

/// One filter, as the proto3 JSON of a `loams.collection.v1.Query`.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter(Value);

/// A document id: `uint`, `string` or `uuid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocId {
    /// `0..=2^64-1`.
    Uint(u64),
    /// A string id.
    Str(String),
    /// A UUID, `8-4-4-4-12`.
    Uuid(String),
}

impl From<u64> for DocId {
    fn from(id: u64) -> Self {
        DocId::Uint(id)
    }
}

impl From<&str> for DocId {
    fn from(id: &str) -> Self {
        DocId::Str(id.to_owned())
    }
}

impl From<String> for DocId {
    fn from(id: String) -> Self {
        DocId::Str(id)
    }
}

impl DocId {
    /// The id as `Query.ids` spells it: `{"uint": "1"}`, a bare string, or
    /// `{"uuid": "…"}`.
    #[must_use]
    pub fn filter_value(&self) -> Value {
        match self {
            DocId::Uint(id) => json!({"uint": id.to_string()}),
            DocId::Str(id) => json!(id),
            DocId::Uuid(id) => json!({"uuid": id}),
        }
    }

    /// The id as `DocumentId`'s proto3 JSON: `{"uint": "1"}`, `{"string": "a"}`
    /// or `{"uuid": "…"}`.
    #[must_use]
    pub fn document_id(&self) -> Value {
        match self {
            DocId::Uint(id) => json!({"uint": id.to_string()}),
            DocId::Str(id) => json!({"string": id}),
            DocId::Uuid(id) => json!({"uuid": id}),
        }
    }
}

/// `{"auto": {}}` or an exact number of edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fuzziness {
    /// The engine picks from the term's length.
    Auto,
    /// 0 to 2 edits.
    Edits(u32),
}

impl Fuzziness {
    fn json(self) -> Value {
        match self {
            Fuzziness::Auto => json!({"auto": {}}),
            Fuzziness::Edits(n) => json!({"edits": n}),
        }
    }
}

/// A range under construction: `Filter::range("n").gte(2).lt(9).build()`.
#[derive(Debug, Clone)]
pub struct Range {
    field: String,
    bounds: Map<String, Value>,
}

impl Range {
    fn bound(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.bounds.insert(key.to_owned(), value.into());
        self
    }

    /// Strictly greater than.
    #[must_use]
    pub fn gt(self, value: impl Into<Value>) -> Self {
        self.bound("gt", value)
    }

    /// Greater than or equal.
    #[must_use]
    pub fn gte(self, value: impl Into<Value>) -> Self {
        self.bound("gte", value)
    }

    /// Strictly less than.
    #[must_use]
    pub fn lt(self, value: impl Into<Value>) -> Self {
        self.bound("lt", value)
    }

    /// Less than or equal.
    #[must_use]
    pub fn lte(self, value: impl Into<Value>) -> Self {
        self.bound("lte", value)
    }

    /// The filter.
    #[must_use]
    pub fn build(self) -> Filter {
        let mut range = Map::new();
        range.insert("field".to_owned(), json!(self.field));
        range.extend(self.bounds);
        Filter(json!({"range": range}))
    }
}

impl From<Range> for Filter {
    fn from(range: Range) -> Self {
        range.build()
    }
}

/// The options of [`Filter::match_text`].
#[derive(Debug, Clone, Default)]
pub struct MatchOptions {
    /// `"or"` (the default) or `"and"`.
    pub operator: Option<String>,
    /// How many of the terms must match.
    pub minimum_should_match: Option<String>,
    /// Edit distance.
    pub fuzziness: Option<Fuzziness>,
    /// The analyzer for the query text.
    pub analyzer: Option<String>,
}

fn bool_operator(operator: &str) -> &'static str {
    if operator == "and" {
        "BOOL_OPERATOR_AND"
    } else {
        "BOOL_OPERATOR_OR"
    }
}

fn list(filters: impl IntoIterator<Item = Filter>) -> Vec<Value> {
    filters.into_iter().map(|filter| filter.0).collect()
}

impl Filter {
    /// Every document.
    #[must_use]
    pub fn all() -> Filter {
        Filter(json!({"matchAll": {}}))
    }

    /// No document.
    #[must_use]
    pub fn none() -> Filter {
        Filter(json!({"matchNone": {}}))
    }

    /// The field equals `value` (a term query).
    #[must_use]
    pub fn eq(field: &str, value: impl Into<Value>) -> Filter {
        Filter(json!({"term": {"field": field, "value": value.into()}}))
    }

    /// The field equals any of `values` (a terms query).
    #[must_use]
    pub fn any_of_values<V: Into<Value>>(
        field: &str,
        values: impl IntoIterator<Item = V>,
    ) -> Filter {
        let values: Vec<Value> = values.into_iter().map(Into::into).collect();
        Filter(json!({"terms": {"field": field, "values": values}}))
    }

    /// `in`: the field equals any of `values`. (`in` is a keyword in Rust.)
    #[must_use]
    pub fn is_in<V: Into<Value>>(field: &str, values: impl IntoIterator<Item = V>) -> Filter {
        Filter::any_of_values(field, values)
    }

    /// A range on `field`; add bounds and call `build`.
    #[must_use]
    pub fn range(field: &str) -> Range {
        Range {
            field: field.to_owned(),
            bounds: Map::new(),
        }
    }

    /// The field is present.
    #[must_use]
    pub fn exists(field: &str) -> Filter {
        Filter(json!({"exists": {"field": field}}))
    }

    /// The field is `null` or absent.
    #[must_use]
    pub fn is_null(field: &str) -> Filter {
        Filter(json!({"isNull": {"field": field}}))
    }

    /// The field is empty.
    #[must_use]
    pub fn is_empty(field: &str) -> Filter {
        Filter(json!({"isEmpty": {"field": field}}))
    }

    /// The field starts with `value`.
    #[must_use]
    pub fn prefix(field: &str, value: &str) -> Filter {
        Filter(json!({"prefix": {"field": field, "value": value}}))
    }

    /// The field matches a `*`/`?` pattern.
    #[must_use]
    pub fn wildcard(field: &str, pattern: &str) -> Filter {
        Filter(json!({"wildcard": {"field": field, "pattern": pattern}}))
    }

    /// The field is within `fuzziness` edits of `value`.
    #[must_use]
    pub fn fuzzy(field: &str, value: &str, fuzziness: Option<Fuzziness>) -> Filter {
        let mut fuzzy = json!({"field": field, "value": value});
        if let Some(fuzziness) = fuzziness {
            fuzzy["fuzziness"] = fuzziness.json();
        }
        Filter(json!({ "fuzzy": fuzzy }))
    }

    /// A full-text match on one field.
    #[must_use]
    pub fn match_text(field: &str, text: &str) -> Filter {
        Filter::match_with(field, text, &MatchOptions::default())
    }

    /// A full-text match with options.
    #[must_use]
    pub fn match_with(field: &str, text: &str, options: &MatchOptions) -> Filter {
        let mut query = json!({"field": field, "text": text});
        if let Some(operator) = &options.operator {
            query["operator"] = json!(operator);
        }
        if let Some(msm) = &options.minimum_should_match {
            query["minimumShouldMatch"] = json!(msm);
        }
        if let Some(fuzziness) = options.fuzziness {
            query["fuzziness"] = fuzziness.json();
        }
        if let Some(analyzer) = &options.analyzer {
            query["analyzer"] = json!(analyzer);
        }
        Filter(json!({ "match": query }))
    }

    /// The words of `text`, in order, at most `slop` positions apart.
    #[must_use]
    pub fn phrase(field: &str, text: &str, slop: Option<u32>) -> Filter {
        let mut query = json!({"field": field, "text": text});
        if let Some(slop) = slop {
            query["slop"] = json!(slop);
        }
        Filter(json!({ "matchPhrase": query }))
    }

    /// A full-text match over several fields; `"title^2"` boosts one.
    #[must_use]
    pub fn multi_match<S: AsRef<str>>(fields: impl IntoIterator<Item = S>, text: &str) -> Filter {
        let fields: Vec<Value> = fields
            .into_iter()
            .map(|spec| {
                let spec = spec.as_ref();
                match spec.rsplit_once('^') {
                    Some((name, boost)) if !name.is_empty() => match boost.parse::<f64>() {
                        Ok(boost) => json!({"field": name, "boost": boost}),
                        Err(_) => json!({ "field": spec }),
                    },
                    _ => json!({ "field": spec }),
                }
            })
            .collect();
        Filter(json!({"multiMatch": {"fields": fields, "text": text}}))
    }

    /// The query-string syntax over `fields`, or the collection's defaults.
    #[must_use]
    pub fn query_string<S: AsRef<str>>(
        query: &str,
        fields: impl IntoIterator<Item = S>,
        operator: Option<&str>,
    ) -> Filter {
        let fields: Vec<String> = fields.into_iter().map(|f| f.as_ref().to_owned()).collect();
        let mut out = json!({ "query": query });
        if !fields.is_empty() {
            out["defaultFields"] = json!(fields);
        }
        if let Some(operator) = operator {
            out["defaultOperator"] = json!(bool_operator(operator));
        }
        Filter(json!({ "queryString": out }))
    }

    /// The documents with these ids, and no others.
    #[must_use]
    pub fn ids<I: Into<DocId>>(ids: impl IntoIterator<Item = I>) -> Filter {
        let ids: Vec<Value> = ids.into_iter().map(|id| id.into().filter_value()).collect();
        Filter(json!({ "ids": ids }))
    }

    /// Every filter matches (a bool `must`).
    #[must_use]
    pub fn and(filters: impl IntoIterator<Item = Filter>) -> Filter {
        Filter(json!({"bool": {"must": list(filters)}}))
    }

    /// At least one matches (a bool `should`).
    #[must_use]
    pub fn or(filters: impl IntoIterator<Item = Filter>) -> Filter {
        Filter(json!({"bool": {"should": list(filters)}}))
    }

    /// At least `minimum_should_match` match.
    #[must_use]
    pub fn any_of(filters: impl IntoIterator<Item = Filter>, minimum_should_match: &str) -> Filter {
        Filter(
            json!({"bool": {"should": list(filters), "minimumShouldMatch": minimum_should_match}}),
        )
    }

    /// None of them match (a bool `must_not`).
    #[must_use]
    pub fn not(filters: impl IntoIterator<Item = Filter>) -> Filter {
        Filter(json!({"bool": {"mustNot": list(filters)}}))
    }

    /// Every filter matches, without scoring (a bool `filter`). The name is
    /// the SDKP verb every SDK shares, so it stays despite naming the type.
    #[must_use]
    #[allow(clippy::self_named_constructors)]
    pub fn filter(filters: impl IntoIterator<Item = Filter>) -> Filter {
        Filter(json!({"bool": {"filter": list(filters)}}))
    }

    /// Multiplies the score by `boost`.
    #[must_use]
    pub fn boost(self, boost: f64) -> Filter {
        Filter(json!({"boost": {"query": self.0, "boost": boost}}))
    }

    /// Every match scores `score`.
    #[must_use]
    pub fn constant_score(self, score: f64) -> Filter {
        Filter(json!({"constantScore": {"query": self.0, "score": score}}))
    }

    /// The filter's proto3 JSON.
    #[must_use]
    pub fn to_json(&self) -> Value {
        self.0.clone()
    }

    /// A filter from proto3 JSON, as is.
    #[must_use]
    pub fn from_json(json: Value) -> Filter {
        Filter(json)
    }

    /// The generated `Query`.
    ///
    /// # Errors
    ///
    /// `invalid_argument` when the JSON is not a `Query`.
    pub fn to_query(&self) -> Result<Query, LoamsError> {
        super::decode(self.0.clone(), "filter")
    }
}
