//! The schema builder (SDKP §2.6): `CreateCollectionRequest.schema`, in the
//! REST route's schema JSON.
//!
//! ```rust
//! use loams::builders::Schema;
//! let schema = Schema::new().text("title").keyword("tags").i64("n").vector("embedding", 4);
//! assert_eq!(schema.to_json()["vectors"][0]["dim"], 4);
//! ```
//!
//! The output is the shape `kb_schema()` in
//! `crates/loams/tests/connect_collections.rs` sends: every field spelled out
//! (`name`, `source_path`, `kind`, `indexed`, `fast`), the vector lists, and
//! `dynamic` and `max_fields` always present.

use serde_json::{Value, json};

/// What every field takes.
#[derive(Debug, Clone, Default)]
pub struct FieldOptions {
    /// In the inverted index. Default `true`.
    pub indexed: Option<bool>,
    /// A columnar copy for sorts, ranges and aggregations. Default `false` for
    /// text, `true` for every other kind.
    pub fast: Option<bool>,
    /// Where the value comes from in `_source`. Default: the name.
    pub source_path: Option<String>,
}

/// A text field's options.
#[derive(Debug, Clone, Default)]
pub struct TextOptions {
    /// `standard` (the default), `english`, `simple`, `whitespace`, `keyword`.
    pub analyzer: Option<String>,
    /// Positions, which phrase queries need. Default `true`.
    pub positions: Option<bool>,
    /// The common options.
    pub field: FieldOptions,
}

/// A dense vector's distance function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Distance {
    /// Cosine, the default.
    #[default]
    Cosine,
    /// Dot product.
    Dot,
    /// Euclidean.
    Euclid,
    /// Manhattan.
    Manhattan,
}

impl Distance {
    fn as_str(self) -> &'static str {
        match self {
            Distance::Cosine => "cosine",
            Distance::Dot => "dot",
            Distance::Euclid => "euclid",
            Distance::Manhattan => "manhattan",
        }
    }
}

/// What a write does with unmapped `_source` paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DynamicMapping {
    /// They live only in `_source` (the builder's default).
    #[default]
    Ignore,
    /// Such a write is refused.
    Strict,
    /// The gateway adds fields for them.
    Map,
}

impl DynamicMapping {
    fn as_str(self) -> &'static str {
        match self {
            DynamicMapping::Ignore => "ignore",
            DynamicMapping::Strict => "strict",
            DynamicMapping::Map => "map",
        }
    }
}

/// The schema builder.
#[derive(Debug, Clone)]
pub struct Schema {
    fields: Vec<Value>,
    vectors: Vec<Value>,
    sparse: Vec<Value>,
    dynamic: DynamicMapping,
    max_fields: u32,
}

impl Default for Schema {
    fn default() -> Self {
        Schema {
            fields: Vec::new(),
            vectors: Vec::new(),
            sparse: Vec::new(),
            dynamic: DynamicMapping::Ignore,
            max_fields: 1000,
        }
    }
}

impl Schema {
    /// An empty schema.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn field(
        mut self,
        name: &str,
        kind: Value,
        fast_default: bool,
        options: &FieldOptions,
    ) -> Self {
        self.fields.push(json!({
            "name": name,
            "source_path": options.source_path.clone().unwrap_or_else(|| name.to_owned()),
            "kind": kind,
            "indexed": options.indexed.unwrap_or(true),
            "fast": options.fast.unwrap_or(fast_default),
        }));
        self
    }

    /// A full-text field with the standard analyzer and positions.
    #[must_use]
    pub fn text(self, name: &str) -> Self {
        self.text_with(name, &TextOptions::default())
    }

    /// A full-text field with options.
    #[must_use]
    pub fn text_with(self, name: &str, options: &TextOptions) -> Self {
        let kind = json!({"text": {
            "analyzer": options.analyzer.clone().unwrap_or_else(|| "standard".to_owned()),
            "positions": options.positions.unwrap_or(true),
        }});
        self.field(name, kind, false, &options.field)
    }

    /// An exact-match string field.
    #[must_use]
    pub fn keyword(self, name: &str) -> Self {
        self.keyword_with(name, &FieldOptions::default())
    }

    /// An exact-match string field with options.
    #[must_use]
    pub fn keyword_with(self, name: &str, options: &FieldOptions) -> Self {
        self.field(name, json!("keyword"), true, options)
    }

    /// A signed 64-bit integer field.
    #[must_use]
    pub fn i64(self, name: &str) -> Self {
        self.field(name, json!("i64"), true, &FieldOptions::default())
    }

    /// A 64-bit float field.
    #[must_use]
    pub fn f64(self, name: &str) -> Self {
        self.field(name, json!("f64"), true, &FieldOptions::default())
    }

    /// A boolean field.
    #[must_use]
    pub fn bool(self, name: &str) -> Self {
        self.field(name, json!("bool"), true, &FieldOptions::default())
    }

    /// An RFC 3339 timestamp field (the schema's `date` kind).
    #[must_use]
    pub fn timestamp(self, name: &str) -> Self {
        self.field(name, json!("date"), true, &FieldOptions::default())
    }

    /// A UUID field.
    #[must_use]
    pub fn uuid(self, name: &str) -> Self {
        self.field(name, json!("uuid"), true, &FieldOptions::default())
    }

    /// Any non-text kind with explicit options: `"i64"`, `"f64"`, `"bool"`,
    /// `"date"`, `"uuid"`, `"keyword"`.
    #[must_use]
    pub fn typed(self, name: &str, kind: &str, options: &FieldOptions) -> Self {
        self.field(name, json!(kind), true, options)
    }

    /// A named dense vector with cosine distance.
    #[must_use]
    pub fn vector(self, name: &str, dim: u32) -> Self {
        self.vector_with(name, dim, Distance::Cosine)
    }

    /// A named dense vector.
    #[must_use]
    pub fn vector_with(mut self, name: &str, dim: u32, distance: Distance) -> Self {
        self.vectors
            .push(json!({"name": name, "dim": dim, "distance": distance.as_str()}));
        self
    }

    /// A named sparse vector; `idf` reweights the query by IDF.
    #[must_use]
    pub fn sparse_vector(mut self, name: &str, idf: Option<bool>) -> Self {
        self.sparse.push(match idf {
            None => json!({ "name": name }),
            Some(true) => json!({"name": name, "modifier": "idf"}),
            Some(false) => json!({"name": name, "modifier": "none"}),
        });
        self
    }

    /// Unmapped `_source` paths. Default `ignore`.
    #[must_use]
    pub fn dynamic(mut self, mode: DynamicMapping) -> Self {
        self.dynamic = mode;
        self
    }

    /// The most fields, vectors and sparse vectors together. Default 1000.
    #[must_use]
    pub fn max_fields(mut self, n: u32) -> Self {
        self.max_fields = n;
        self
    }

    /// The schema `Struct`'s JSON.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "fields": self.fields,
            "vectors": self.vectors,
            "sparse_vectors": self.sparse,
            "dynamic": self.dynamic.as_str(),
            "max_fields": self.max_fields,
        })
    }

    /// The schema as the `google.protobuf.Struct` `CreateCollectionRequest`
    /// takes.
    ///
    /// # Errors
    ///
    /// Never in practice; the JSON is always an object.
    pub fn to_struct(
        &self,
    ) -> Result<buffa_types::google::protobuf::Struct, crate::error::LoamsError> {
        super::decode(self.to_json(), "schema")
    }
}
