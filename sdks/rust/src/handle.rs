//! The collection handle (SDKP §2.8) and the hand-written document and search
//! conveniences (SDKP §2.3, §2.4).
//!
//! ```rust,ignore
//! let notes = loams.collection("notes");
//! notes.upsert(vec![Document::new(1u64).source(json!({"title": "Refunds"}))], CallOptionsOverrides::new()).await?;
//! let hits = notes.search(notes.query().text(TextRetriever::fields("refund", ["body"])), CallOptionsOverrides::new()).await?;
//! ```
//!
//! A handle delegates every call to [`CollectionCalls`] with `collection`
//! (and `namespace`, when it was given one) filled in, and holds no state of
//! its own. `CollectionCalls` is the app modules' unary calls, implemented by
//! the client once those modules are generated (SDKP P0a) and by a stub in the
//! tests.

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::Stream;
use serde_json::{Map, Value, json};

use crate::batch::{BatchError, BatchOptions, BatchResult, write_batch};
use crate::builders::{DocId, Filter, Search, VectorOptions, decode};
use crate::call::CallOptionsOverrides;
use crate::error::{ErrorKind, LoamsError};
use connectrpc::ErrorCode as Code;
use loams_proto::loams::collection::v1::{
    CollectionInfo, CountDocumentsRequest, CountDocumentsResponse, Document as StoredDocument,
    DropCollectionRequest, DropCollectionResponse, GetCollectionRequest, GetDocumentsRequest,
    GetDocumentsResponse, Hit, ScrollDocumentsRequest, ScrollDocumentsResponse, SearchRequest,
    SearchResponse, WriteDocumentsRequest, WriteDocumentsResponse, WriteOp,
};

/// The app modules' unary calls a handle and the conveniences need.
/// The futures are not `Send`: a handle runs on the caller's task.
#[async_trait(?Send)]
pub trait CollectionCalls: Send + Sync {
    /// `documents.write`.
    async fn write_documents(
        &self,
        request: WriteDocumentsRequest,
        options: CallOptionsOverrides,
    ) -> Result<WriteDocumentsResponse, LoamsError>;
    /// `documents.get`.
    async fn get_documents(
        &self,
        request: GetDocumentsRequest,
        options: CallOptionsOverrides,
    ) -> Result<GetDocumentsResponse, LoamsError>;
    /// `documents.scroll`.
    async fn scroll_documents(
        &self,
        request: ScrollDocumentsRequest,
        options: CallOptionsOverrides,
    ) -> Result<ScrollDocumentsResponse, LoamsError>;
    /// `documents.count`.
    async fn count_documents(
        &self,
        request: CountDocumentsRequest,
        options: CallOptionsOverrides,
    ) -> Result<CountDocumentsResponse, LoamsError>;
    /// `search.query`.
    async fn search(
        &self,
        request: SearchRequest,
        options: CallOptionsOverrides,
    ) -> Result<SearchResponse, LoamsError>;
    /// `collections.get`.
    async fn get_collection(
        &self,
        request: GetCollectionRequest,
        options: CallOptionsOverrides,
    ) -> Result<CollectionInfo, LoamsError>;
    /// `collections.drop`.
    async fn drop_collection(
        &self,
        request: DropCollectionRequest,
        options: CallOptionsOverrides,
    ) -> Result<DropCollectionResponse, LoamsError>;
}

/// One document to write.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// The id.
    pub id: DocId,
    /// `_source`, a JSON object.
    pub source: Option<Value>,
    /// Dense vectors by name.
    pub vectors: BTreeMap<String, Vec<f32>>,
}

impl Document {
    /// A document with only an id.
    #[must_use]
    pub fn new(id: impl Into<DocId>) -> Self {
        Document {
            id: id.into(),
            source: None,
            vectors: BTreeMap::new(),
        }
    }

    /// With `_source`.
    #[must_use]
    pub fn source(mut self, source: Value) -> Self {
        self.source = Some(source);
        self
    }

    /// With one dense vector.
    #[must_use]
    pub fn vector(mut self, name: &str, values: Vec<f32>) -> Self {
        self.vectors.insert(name.to_owned(), values);
        self
    }

    fn json(&self) -> Value {
        let mut doc = Map::new();
        doc.insert("id".to_owned(), self.id.document_id());
        if let Some(source) = &self.source {
            doc.insert("source".to_owned(), source.clone());
        }
        if !self.vectors.is_empty() {
            doc.insert("vectors".to_owned(), json!(self.vectors));
        }
        Value::Object(doc)
    }
}

/// What a patch changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PatchChange {
    /// Keys to merge into `_source`.
    pub set: Option<Value>,
    /// Top-level keys to remove.
    pub unset: Vec<String>,
    /// `merge_deep` (the default), `merge_top` or `replace`.
    pub mode: Option<String>,
}

/// The `WriteOp`s of the document conveniences.
pub mod ops {
    use super::{DocId, Document, LoamsError, PatchChange, Value, WriteOp, decode, json};

    /// An upsert.
    ///
    /// # Errors
    ///
    /// `invalid_argument` when `source` is not an object.
    pub fn upsert(doc: &Document) -> Result<WriteOp, LoamsError> {
        decode(json!({"upsert": doc.json()}), "upsert")
    }

    /// A delete.
    ///
    /// # Errors
    ///
    /// Never for a well-formed id.
    pub fn delete(id: &DocId) -> Result<WriteOp, LoamsError> {
        decode(json!({"delete": {"id": id.document_id()}}), "delete")
    }

    /// A patch.
    ///
    /// # Errors
    ///
    /// `invalid_argument` when `set` is not an object.
    pub fn patch(id: &DocId, change: &PatchChange) -> Result<WriteOp, LoamsError> {
        let mut patch = json!({ "id": id.document_id() });
        if let Some(mode) = &change.mode {
            patch["mode"] = json!(mode);
        }
        if let Some(set) = &change.set {
            patch["source"] = set.clone();
        }
        if !change.unset.is_empty() {
            patch["deleteKeys"] = json!(change.unset);
        }
        decode::<WriteOp>(
            Value::Object(Map::from_iter([("patch".to_owned(), patch)])),
            "patch",
        )
    }

    use serde_json::Map;
}

/// Where a call goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The collection.
    pub collection: String,
    /// The namespace, when it is not the client's default.
    pub namespace: Option<String>,
}

impl Target {
    fn json(&self) -> Map<String, Value> {
        let mut out = Map::new();
        if let Some(namespace) = &self.namespace {
            out.insert("namespace".to_owned(), json!(namespace));
        }
        out.insert("collection".to_owned(), json!(self.collection));
        out
    }

    fn write(&self, ops: Vec<WriteOp>) -> WriteDocumentsRequest {
        WriteDocumentsRequest {
            namespace: self.namespace.clone().unwrap_or_default(),
            collection: self.collection.clone(),
            ops,
            ..Default::default()
        }
    }
}

/// Writes documents in one call.
///
/// # Errors
///
/// The call's failure, or `invalid_argument` for a document that does not
/// encode.
pub async fn upsert<C: CollectionCalls + ?Sized>(
    calls: &C,
    target: &Target,
    docs: &[Document],
    options: CallOptionsOverrides,
) -> Result<WriteDocumentsResponse, LoamsError> {
    let ops = docs
        .iter()
        .map(ops::upsert)
        .collect::<Result<Vec<_>, _>>()?;
    calls.write_documents(target.write(ops), options).await
}

/// A bulk upsert, chunked (R13).
///
/// # Errors
///
/// The first failed chunk, as a [`BatchError`].
pub async fn upsert_batch<C: CollectionCalls + ?Sized>(
    calls: &C,
    target: &Target,
    docs: impl IntoIterator<Item = Document>,
    batch: BatchOptions,
    options: CallOptionsOverrides,
) -> Result<BatchResult, BatchError> {
    use buffa::Message as _;
    let mut encoded = Vec::new();
    for doc in docs {
        match ops::upsert(&doc) {
            Ok(op) => encoded.push(op),
            Err(source) => {
                return Err(BatchError {
                    chunks_applied: 0,
                    rows_applied: 0,
                    token: None,
                    source,
                });
            }
        }
    }
    write_batch(
        encoded,
        batch,
        |op: &WriteOp| op.encoded_len() as usize,
        |chunk, key| {
            let request = target.write(chunk);
            let options = options.clone().idempotency_key(key);
            async move {
                calls
                    .write_documents(request, options)
                    .await
                    .map(|answer| Some(answer.token).filter(|t| !t.is_empty()))
            }
        },
    )
    .await
}

/// Every document, page by page, following `next` until it is unset.
pub fn scroll_all<'a, C: CollectionCalls + ?Sized>(
    calls: &'a C,
    target: &Target,
    where_: Option<&Filter>,
    limit: Option<u32>,
    options: CallOptionsOverrides,
) -> impl Stream<Item = Result<StoredDocument, LoamsError>> + 'a {
    let mut base = target.json();
    if let Some(filter) = where_ {
        base.insert("where".to_owned(), filter.to_json());
    }
    if let Some(limit) = limit {
        base.insert("limit".to_owned(), json!(limit));
    }
    let first: Result<ScrollDocumentsRequest, LoamsError> =
        decode(Value::Object(base), "scroll request");
    futures::stream::unfold(
        (Some(first), std::collections::VecDeque::new()),
        move |(mut next, mut buffer)| {
            let options = options.clone();
            async move {
                loop {
                    if let Some(doc) = buffer.pop_front() {
                        return Some((Ok(doc), (next, buffer)));
                    }
                    let request: ScrollDocumentsRequest = match next.take()? {
                        Ok(request) => request,
                        Err(error) => return Some((Err(error), (None, buffer))),
                    };
                    match calls
                        .scroll_documents(request.clone(), options.clone())
                        .await
                    {
                        Ok(page) => {
                            buffer.extend(page.documents);
                            // `next` unset (or an empty id) is the last page.
                            next = page
                                .next
                                .as_option()
                                .filter(|id| id.id.is_some())
                                .map(|id| {
                                    let mut more = request;
                                    more.after = buffa::MessageField::some(id.clone());
                                    Ok(more)
                                });
                        }
                        Err(error) => return Some((Err(error), (None, buffer))),
                    }
                }
            }
        },
    )
}

/// Every hit, page after page, following `search_after` from the last hit's
/// sort values until a page comes back short. The sort must end in a
/// primary-key tiebreaker ([`crate::builders::Sort::pk`]); without one this
/// fails with `invalid_argument` before any call.
pub fn query_all<'a, C: CollectionCalls + ?Sized>(
    calls: &'a C,
    builder: Search,
    options: CallOptionsOverrides,
) -> impl Stream<Item = Result<Hit, LoamsError>> + 'a {
    let refused = (!builder.has_tiebreak()).then(|| {
        LoamsError::new(
            ErrorKind::InvalidArgument,
            Code::InvalidArgument,
            "search.query_all needs a sort that ends in the primary key, such as .sort([Sort::score(None), Sort::pk(None)])",
        )
        .with_client_reason(crate::error::SEARCH_AFTER_NEEDS_TIEBREAK)
        .with_metadata("field", "sort")
    });
    let window = builder.window() as usize;
    futures::stream::unfold(
        (
            Some(builder.clone()),
            std::collections::VecDeque::<Hit>::new(),
            refused,
        ),
        move |(page, mut buffer, refused)| {
            let options = options.clone();
            let base = builder.clone();
            async move {
                if let Some(error) = refused {
                    return Some((Err(error), (None, buffer, None)));
                }
                if let Some(hit) = buffer.pop_front() {
                    return Some((Ok(hit), (page, buffer, None)));
                }
                let current = page?;
                let request = match current.build() {
                    Ok(request) => request,
                    Err(error) => return Some((Err(error), (None, buffer, None))),
                };
                match calls.search(request, options).await {
                    Ok(answer) => {
                        let hits = answer.hits;
                        let next = (hits.len() >= window)
                            .then(|| hits.last().map(sort_values_json))
                            .flatten()
                            .map(|values| base.clone().after(values));
                        buffer.extend(hits);
                        let first = buffer.pop_front()?;
                        Some((Ok(first), (next, buffer, None)))
                    }
                    Err(error) => Some((Err(error), (None, buffer, None))),
                }
            }
        },
    )
}

fn sort_values_json(hit: &Hit) -> Vec<Value> {
    hit.sort_values
        .iter()
        .map(|value| serde_json::to_value(value).unwrap_or(Value::Null))
        .collect()
}

/// A collection, by name, with the document and search calls bound to it.
#[derive(Debug, Clone)]
pub struct CollectionHandle<'a, C: ?Sized> {
    calls: &'a C,
    target: Target,
}

impl<'a, C: CollectionCalls + ?Sized> CollectionHandle<'a, C> {
    /// A handle on `name`, in the client's default namespace unless one is
    /// given.
    pub fn new(calls: &'a C, name: &str, namespace: Option<&str>) -> Self {
        CollectionHandle {
            calls,
            target: Target {
                collection: name.to_owned(),
                namespace: namespace.map(str::to_owned),
            },
        }
    }

    /// The collection's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.target.collection
    }

    /// Writes (creates or replaces) documents.
    ///
    /// # Errors
    ///
    /// The call's failure.
    pub async fn upsert(
        &self,
        docs: &[Document],
        options: CallOptionsOverrides,
    ) -> Result<WriteDocumentsResponse, LoamsError> {
        upsert(self.calls, &self.target, docs, options).await
    }

    /// Reads documents by id.
    ///
    /// # Errors
    ///
    /// The call's failure.
    pub async fn get(
        &self,
        ids: &[DocId],
        options: CallOptionsOverrides,
    ) -> Result<GetDocumentsResponse, LoamsError> {
        let mut json = self.target.json();
        json.insert(
            "ids".to_owned(),
            Value::Array(ids.iter().map(DocId::document_id).collect()),
        );
        let request = decode(Value::Object(json), "get request")?;
        self.calls.get_documents(request, options).await
    }

    /// Removes documents by id.
    ///
    /// # Errors
    ///
    /// The call's failure.
    pub async fn delete(
        &self,
        ids: &[DocId],
        options: CallOptionsOverrides,
    ) -> Result<WriteDocumentsResponse, LoamsError> {
        let ops = ids.iter().map(ops::delete).collect::<Result<Vec<_>, _>>()?;
        self.calls
            .write_documents(self.target.write(ops), options)
            .await
    }

    /// Changes part of one document.
    ///
    /// # Errors
    ///
    /// The call's failure.
    pub async fn patch(
        &self,
        id: &DocId,
        change: &PatchChange,
        options: CallOptionsOverrides,
    ) -> Result<WriteDocumentsResponse, LoamsError> {
        let op = ops::patch(id, change)?;
        self.calls
            .write_documents(self.target.write(vec![op]), options)
            .await
    }

    /// A chunked bulk upsert (R13).
    ///
    /// # Errors
    ///
    /// The first failed chunk.
    pub async fn write_batch(
        &self,
        docs: impl IntoIterator<Item = Document>,
        batch: BatchOptions,
        options: CallOptionsOverrides,
    ) -> Result<BatchResult, BatchError> {
        upsert_batch(self.calls, &self.target, docs, batch, options).await
    }

    /// How many documents match `where_` (all of them without one).
    ///
    /// # Errors
    ///
    /// The call's failure.
    pub async fn count(
        &self,
        where_: Option<&Filter>,
        options: CallOptionsOverrides,
    ) -> Result<CountDocumentsResponse, LoamsError> {
        let mut json = self.target.json();
        if let Some(filter) = where_ {
            json.insert("where".to_owned(), filter.to_json());
        }
        let request = decode(Value::Object(json), "count request")?;
        self.calls.count_documents(request, options).await
    }

    /// Every document, page by page.
    pub fn scroll_all(
        &self,
        where_: Option<&Filter>,
        limit: Option<u32>,
        options: CallOptionsOverrides,
    ) -> impl Stream<Item = Result<StoredDocument, LoamsError>> + 'a {
        scroll_all(self.calls, &self.target, where_, limit, options)
    }

    /// A search builder pointed at this collection.
    #[must_use]
    pub fn query(&self) -> Search {
        let search = Search::on(&self.target.collection);
        match &self.target.namespace {
            Some(namespace) => search.namespace(namespace),
            None => search,
        }
    }

    /// Runs a search; the builder's collection is replaced by this one.
    ///
    /// # Errors
    ///
    /// The builder's refusal or the call's failure.
    pub async fn search(
        &self,
        builder: Search,
        options: CallOptionsOverrides,
    ) -> Result<SearchResponse, LoamsError> {
        let mut builder = builder.collection(&self.target.collection);
        if let Some(namespace) = &self.target.namespace {
            builder = builder.namespace(namespace);
        }
        self.calls.search(builder.build()?, options).await
    }

    /// A kNN search on one vector field.
    ///
    /// # Errors
    ///
    /// The call's failure.
    pub async fn vector_search(
        &self,
        field: &str,
        vector: &[f32],
        vector_options: VectorOptions,
        limit: Option<u32>,
        options: CallOptionsOverrides,
    ) -> Result<SearchResponse, LoamsError> {
        let mut builder = self.query().vector(field, vector, vector_options);
        if let Some(limit) = limit {
            builder = builder.limit(limit);
        }
        self.search(builder, options).await
    }

    /// The collection's schema and stats.
    ///
    /// # Errors
    ///
    /// The call's failure.
    pub async fn info(&self, options: CallOptionsOverrides) -> Result<CollectionInfo, LoamsError> {
        let request = GetCollectionRequest {
            namespace: self.target.namespace.clone().unwrap_or_default(),
            collection: self.target.collection.clone(),
            ..Default::default()
        };
        self.calls.get_collection(request, options).await
    }

    /// Drops the collection.
    ///
    /// # Errors
    ///
    /// The call's failure.
    pub async fn drop(
        &self,
        options: CallOptionsOverrides,
    ) -> Result<DropCollectionResponse, LoamsError> {
        let request = DropCollectionRequest {
            namespace: self.target.namespace.clone().unwrap_or_default(),
            collection: self.target.collection.clone(),
            ..Default::default()
        };
        self.calls.drop_collection(request, options).await
    }
}
