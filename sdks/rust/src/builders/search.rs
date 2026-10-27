//! The search builder (SDKP §2.4): a `loams.collection.v1.SearchRequest`, one
//! verb at a time.
//!
//! ```rust
//! use loams::builders::{Filter, Fusion, Search, TextRetriever, VectorOptions};
//! let request = Search::on("notes")
//!     .text(TextRetriever::fields("refund", ["body"]))
//!     .vector("embedding", &[0.1, 0.2, 0.3, 0.4], VectorOptions::k(10))
//!     .fuse(Fusion::rrf(Some(60)))
//!     .filter(Filter::eq("tags", "policy"))
//!     .limit(5)
//!     .build()
//!     .expect("a valid request");
//! assert_eq!(request.retrievers.len(), 2);
//! ```
//!
//! Each verb takes the builder by value and returns it; `Clone` it to share a
//! base query. [`Search::to_json`] is the proto3 JSON the builder corpus
//! compares; [`Search::build`] decodes it into the generated message and
//! refuses what the builder can tell is wrong (a weighted fusion with the wrong
//! number of weights) before any network call.

use serde_json::{Map, Value, json};

use crate::consistency::ConsistencyToken;
use crate::error::{ErrorKind, LoamsError};
use connectrpc::ErrorCode as Code;
use loams_proto::loams::collection::v1::SearchRequest;

use super::filter::Filter;

/// A fusion, as the proto3 JSON of `loams.collection.v1.Fusion`.
#[derive(Debug, Clone, PartialEq)]
pub struct Fusion(Value);

impl Fusion {
    /// Reciprocal-rank fusion; `None` is the engine's 60.
    #[must_use]
    pub fn rrf(k: Option<u32>) -> Fusion {
        Fusion(match k {
            Some(k) => json!({"rrf": {"k": k}}),
            None => json!({"rrf": {}}),
        })
    }

    /// Distance-based score fusion.
    #[must_use]
    pub fn dbsf() -> Fusion {
        Fusion(json!({"dbsf": {}}))
    }

    /// A weighted sum, one weight per retriever in the order they were added.
    #[must_use]
    pub fn weighted(weights: impl IntoIterator<Item = f64>) -> Fusion {
        let weights: Vec<f64> = weights.into_iter().collect();
        Fusion(json!({"weightedSum": {"weights": weights}}))
    }

    /// The fusion's proto3 JSON.
    #[must_use]
    pub fn to_json(&self) -> Value {
        self.0.clone()
    }
}

/// A sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Ascending.
    Asc,
    /// Descending.
    Desc,
}

impl Order {
    fn json(self) -> Value {
        json!(match self {
            Order::Asc => "SORT_ORDER_ASC",
            Order::Desc => "SORT_ORDER_DESC",
        })
    }
}

/// One sort key, as the proto3 JSON of `SortKey`.
#[derive(Debug, Clone, PartialEq)]
pub struct Sort(Value);

impl Sort {
    /// By score; descending unless told otherwise.
    #[must_use]
    pub fn score(order: Option<Order>) -> Sort {
        Sort(json!({"score": order.map_or_else(|| json!({}), |o| json!({"order": o.json()}))}))
    }

    /// By primary key; ascending unless told otherwise. `query_all` needs
    /// this as the last key.
    #[must_use]
    pub fn pk(order: Option<Order>) -> Sort {
        Sort(json!({"pk": order.map_or_else(|| json!({}), |o| json!({"order": o.json()}))}))
    }

    /// By a fast field; `missing_first` puts documents without it first.
    #[must_use]
    pub fn field(name: &str, order: Option<Order>, missing_first: Option<bool>) -> Sort {
        let mut key = json!({ "field": name });
        if let Some(order) = order {
            key["order"] = order.json();
        }
        if let Some(first) = missing_first {
            key["missing"] = json!(if first {
                "MISSING_ORDER_FIRST"
            } else {
                "MISSING_ORDER_LAST"
            });
        }
        Sort(json!({ "field": key }))
    }
}

/// A text retriever: a query string, or a filter used as the query.
#[derive(Debug, Clone)]
pub struct TextRetriever {
    query: Filter,
    k: Option<u32>,
}

impl TextRetriever {
    /// A query string over the collection's default fields.
    #[must_use]
    pub fn query_string(text: &str) -> Self {
        TextRetriever {
            query: Filter::query_string(text, Vec::<String>::new(), None),
            k: None,
        }
    }

    /// One field is a `match`, several a `multi_match`, none a
    /// `query_string`.
    #[must_use]
    pub fn fields<S: AsRef<str>>(text: &str, fields: impl IntoIterator<Item = S>) -> Self {
        let fields: Vec<String> = fields.into_iter().map(|f| f.as_ref().to_owned()).collect();
        let query = match fields.as_slice() {
            [] => Filter::query_string(text, Vec::<String>::new(), None),
            [one] => Filter::match_text(one, text),
            many => Filter::multi_match(many, text),
        };
        TextRetriever { query, k: None }
    }

    /// A filter (`match`, `multi_match`, `phrase`…) as the query.
    #[must_use]
    pub fn filter(query: Filter) -> Self {
        TextRetriever { query, k: None }
    }

    /// How many candidates this retriever contributes.
    #[must_use]
    pub fn k(mut self, k: u32) -> Self {
        self.k = Some(k);
        self
    }
}

impl From<&str> for TextRetriever {
    fn from(text: &str) -> Self {
        TextRetriever::query_string(text)
    }
}

/// A vector retriever's options.
#[derive(Debug, Clone, Default)]
pub struct VectorOptions {
    /// Candidates.
    pub k: Option<u32>,
    /// An exact (brute-force) search.
    pub exact: Option<bool>,
    /// IVF partitions to probe.
    pub nprobes: Option<u32>,
    /// Re-rank factor.
    pub refine_factor: Option<u32>,
    /// HNSW search breadth.
    pub ef: Option<u32>,
    /// Quantised oversampling.
    pub oversampling: Option<f64>,
    /// Overrides the schema's distance.
    pub distance: Option<String>,
    /// A pre-filter on this retriever's candidates.
    pub filter: Option<Filter>,
}

impl VectorOptions {
    /// Only `k`.
    #[must_use]
    pub fn k(k: u32) -> Self {
        VectorOptions {
            k: Some(k),
            ..Self::default()
        }
    }
}

/// What to return of each hit.
#[derive(Debug, Clone, Default)]
pub struct SelectOptions {
    /// `_source` paths to keep.
    pub include: Option<Vec<String>>,
    /// `_source` paths to drop.
    pub exclude: Option<Vec<String>>,
    /// No `_source` at all.
    pub no_source: bool,
    /// Vectors to return.
    pub vectors: Option<Vec<String>>,
    /// Fast fields to return.
    pub fields: Option<Vec<String>>,
}

impl SelectOptions {
    /// Keep only these `_source` paths.
    #[must_use]
    pub fn include<S: AsRef<str>>(paths: impl IntoIterator<Item = S>) -> Self {
        SelectOptions {
            include: Some(paths.into_iter().map(|p| p.as_ref().to_owned()).collect()),
            ..Self::default()
        }
    }
}

/// A highlighted field's options.
#[derive(Debug, Clone, Default)]
pub struct HighlightOptions {
    /// The opening tag.
    pub pre: Option<String>,
    /// The closing tag.
    pub post: Option<String>,
    /// Fragment length.
    pub fragment_size: Option<u32>,
    /// Fragments per field.
    pub fragments: Option<u32>,
}

/// A graph expansion from the top hits.
#[derive(Debug, Clone, Default)]
pub struct ExpandOptions {
    /// The source field holding the vertex key.
    pub from_field: Option<String>,
    /// The seed vertices' label.
    pub seed_label: Option<String>,
    /// How many top hits seed the walk.
    pub seeds: Option<u32>,
    /// Hops.
    pub hops: Option<u32>,
    /// `out`, `in` or `both`.
    pub direction: Option<String>,
    /// Edge types to follow.
    pub edge_types: Option<Vec<String>>,
    /// Neighbours per seed.
    pub limit_per_seed: Option<u32>,
    /// Neighbours overall.
    pub limit: Option<u32>,
}

/// How fresh a read must be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsistencyLevel {
    /// Every acknowledged write.
    Strong,
    /// Whatever the tail holds.
    Eventual,
    /// At least the writes this token names.
    AtLeast(ConsistencyToken),
}

impl ConsistencyLevel {
    /// The proto3 JSON of `loams.collection.v1.Consistency`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            ConsistencyLevel::Strong => json!({"freshness": "FRESHNESS_STRONG"}),
            ConsistencyLevel::Eventual => json!({"freshness": "FRESHNESS_EVENTUAL"}),
            ConsistencyLevel::AtLeast(token) => json!({"atLeast": token.to_string()}),
        }
    }
}

/// The search builder.
#[derive(Debug, Clone)]
pub struct Search {
    json: Map<String, Value>,
}

fn put_opt<T: serde::Serialize>(map: &mut Map<String, Value>, key: &str, value: Option<&T>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), json!(value));
    }
}

impl Search {
    /// A search of `collection`, a name or an alias.
    #[must_use]
    pub fn on(collection: &str) -> Search {
        let mut json = Map::new();
        json.insert("collection".to_owned(), json!(collection));
        Search { json }
    }

    /// A builder over an existing request's proto3 JSON.
    #[must_use]
    pub fn from_json(json: Map<String, Value>) -> Search {
        Search { json }
    }

    fn set(mut self, key: &str, value: Value) -> Search {
        self.json.insert(key.to_owned(), value);
        self
    }

    fn push_retriever(mut self, retriever: Value) -> Search {
        let entry = self
            .json
            .entry("retrievers")
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(list) = entry {
            list.push(retriever);
        }
        self
    }

    /// The namespace, when it is not the client's default.
    #[must_use]
    pub fn namespace(self, namespace: &str) -> Search {
        self.set("namespace", json!(namespace))
    }

    /// Replaces the collection.
    #[must_use]
    pub fn collection(self, collection: &str) -> Search {
        self.set("collection", json!(collection))
    }

    /// A text retriever: a `&str` is a query string; see [`TextRetriever`].
    #[must_use]
    pub fn text(self, retriever: impl Into<TextRetriever>) -> Search {
        let retriever = retriever.into();
        let mut text = Map::new();
        text.insert("query".to_owned(), retriever.query.to_json());
        put_opt(&mut text, "k", retriever.k.as_ref());
        self.push_retriever(json!({ "text": text }))
    }

    /// A dense-vector kNN retriever.
    #[must_use]
    pub fn vector(self, field: &str, values: &[f32], options: VectorOptions) -> Search {
        let mut vector = Map::new();
        vector.insert("field".to_owned(), json!(field));
        vector.insert("query".to_owned(), json!(values));
        put_opt(&mut vector, "k", options.k.as_ref());
        let mut params = Map::new();
        put_opt(&mut params, "exact", options.exact.as_ref());
        put_opt(&mut params, "nprobes", options.nprobes.as_ref());
        put_opt(&mut params, "refineFactor", options.refine_factor.as_ref());
        put_opt(&mut params, "ef", options.ef.as_ref());
        put_opt(&mut params, "oversampling", options.oversampling.as_ref());
        put_opt(&mut params, "distance", options.distance.as_ref());
        if !params.is_empty() {
            vector.insert("params".to_owned(), Value::Object(params));
        }
        if let Some(filter) = options.filter {
            vector.insert("filter".to_owned(), filter.to_json());
        }
        self.push_retriever(json!({ "vector": vector }))
    }

    /// A sparse-vector retriever.
    #[must_use]
    pub fn sparse(self, field: &str, indices: &[u32], values: &[f32], k: Option<u32>) -> Search {
        let mut sparse = Map::new();
        sparse.insert("field".to_owned(), json!(field));
        sparse.insert(
            "query".to_owned(),
            json!({"indices": indices, "values": values}),
        );
        put_opt(&mut sparse, "k", k.as_ref());
        self.push_retriever(json!({ "sparse": sparse }))
    }

    /// How the ranked lists combine.
    #[must_use]
    pub fn fuse(self, fusion: Fusion) -> Search {
        self.set("fusion", fusion.0)
    }

    /// The filter every candidate must match.
    #[must_use]
    pub fn filter(self, filter: Filter) -> Search {
        self.set("filter", filter.to_json())
    }

    /// The sort, replacing any earlier one.
    #[must_use]
    pub fn sort(self, keys: impl IntoIterator<Item = Sort>) -> Search {
        let keys: Vec<Value> = keys.into_iter().map(|key| key.0).collect();
        self.set("sort", Value::Array(keys))
    }

    /// The window size; the engine's default is 10.
    #[must_use]
    pub fn limit(self, limit: u32) -> Search {
        self.set("limit", json!(limit))
    }

    /// Hits to skip before the window.
    #[must_use]
    pub fn offset(self, offset: u32) -> Search {
        self.set("offset", json!(offset))
    }

    /// The page after the hit whose sort values these are (as JSON).
    #[must_use]
    pub fn after(self, sort_values: impl IntoIterator<Item = Value>) -> Search {
        self.set(
            "searchAfter",
            Value::Array(sort_values.into_iter().collect()),
        )
    }

    /// Drops hits scoring below `score`.
    #[must_use]
    pub fn min_score(self, score: f64) -> Search {
        self.set("scoreThreshold", json!(score))
    }

    /// What to return of each hit.
    #[must_use]
    pub fn select(self, options: SelectOptions) -> Search {
        let mut select = Map::new();
        if options.no_source {
            select.insert("source".to_owned(), json!("none"));
        } else if options.include.is_some() || options.exclude.is_some() {
            let mut source = Map::new();
            put_opt(&mut source, "include", options.include.as_ref());
            put_opt(&mut source, "exclude", options.exclude.as_ref());
            select.insert("source".to_owned(), Value::Object(source));
        }
        put_opt(&mut select, "vectors", options.vectors.as_ref());
        put_opt(&mut select, "fields", options.fields.as_ref());
        self.set("select", Value::Object(select))
    }

    /// Highlights `field`; call once per field.
    #[must_use]
    pub fn highlight(mut self, field: &str, options: HighlightOptions) -> Search {
        let mut entry = Map::new();
        entry.insert("field".to_owned(), json!(field));
        put_opt(&mut entry, "preTag", options.pre.as_ref());
        put_opt(&mut entry, "postTag", options.post.as_ref());
        put_opt(&mut entry, "fragmentSize", options.fragment_size.as_ref());
        put_opt(&mut entry, "numberOfFragments", options.fragments.as_ref());
        let highlight = self
            .json
            .entry("highlight")
            .or_insert_with(|| json!({ "fields": [] }));
        if let Some(Value::Array(fields)) = highlight.get_mut("fields") {
            fields.push(Value::Object(entry));
        }
        self
    }

    /// Groups the hits by `field`.
    #[must_use]
    pub fn group_by(self, field: &str, size: Option<u32>, limit: Option<u32>) -> Search {
        let mut group = Map::new();
        group.insert("field".to_owned(), json!(field));
        put_opt(&mut group, "groupSize", size.as_ref());
        put_opt(&mut group, "limit", limit.as_ref());
        self.set("groupBy", Value::Object(group))
    }

    /// Walks `graph` from the top hits.
    #[must_use]
    pub fn expand(self, graph: &str, options: ExpandOptions) -> Search {
        let mut spec = Map::new();
        spec.insert("graph".to_owned(), json!(graph));
        put_opt(&mut spec, "fromField", options.from_field.as_ref());
        put_opt(&mut spec, "seedLabel", options.seed_label.as_ref());
        put_opt(&mut spec, "seeds", options.seeds.as_ref());
        put_opt(&mut spec, "hops", options.hops.as_ref());
        if let Some(direction) = &options.direction {
            spec.insert(
                "direction".to_owned(),
                json!(format!("EXPAND_DIRECTION_{}", direction.to_uppercase())),
            );
        }
        put_opt(&mut spec, "edgeTypes", options.edge_types.as_ref());
        put_opt(&mut spec, "limitPerSeed", options.limit_per_seed.as_ref());
        put_opt(&mut spec, "limit", options.limit.as_ref());
        self.set("expandSpec", Value::Object(spec))
    }

    /// How fresh the read must be.
    #[must_use]
    pub fn consistency(self, level: ConsistencyLevel) -> Search {
        self.set("consistency", level.to_json())
    }

    /// The window size the builder asks for, or the engine's 10.
    #[must_use]
    pub fn window(&self) -> u32 {
        self.json
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(10)
    }

    /// Whether the sort ends in a primary-key tiebreaker.
    #[must_use]
    pub fn has_tiebreak(&self) -> bool {
        self.json
            .get("sort")
            .and_then(Value::as_array)
            .and_then(|keys| keys.last())
            .is_some_and(|last| last.get("pk").is_some())
    }

    /// The request's proto3 JSON.
    #[must_use]
    pub fn to_json(&self) -> Value {
        Value::Object(self.json.clone())
    }

    /// The generated message, checked.
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a weighted fusion whose weights do not match the
    /// retrievers one to one, or JSON that does not decode.
    pub fn build(&self) -> Result<SearchRequest, LoamsError> {
        let retrievers = self
            .json
            .get("retrievers")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        if let Some(weights) = self
            .json
            .get("fusion")
            .and_then(|f| f.get("weightedSum"))
            .and_then(|w| w.get("weights"))
            .and_then(Value::as_array)
            && weights.len() != retrievers
        {
            let mut error = LoamsError::new(
                ErrorKind::InvalidArgument,
                Code::InvalidArgument,
                format!(
                    "a weighted fusion needs one weight per retriever: {} weights for {retrievers} retrievers",
                    weights.len()
                ),
            );
            error = error.with_metadata("field", "fusion.weighted_sum.weights");
            return Err(error);
        }
        super::decode(self.to_json(), "search request")
    }
}
