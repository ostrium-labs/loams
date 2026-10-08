//! The graph catalog (design §48 §8.2; GR1 Task 4): which graphs exist, with their settings, as
//! `GraphMeta` records with a CAS `version`.
//!
//! **Where it lives.** One document per namespace, `{ name → GraphMeta }`, written to the bucket
//! as an immutable object and committed by a compare-and-swap of the namespace's metastore
//! pointer `graph-catalog` to that object's key: the manifest idiom of design §03 §3.3. The plan
//! named per-graph metastore keys (`graphs/<ns>/<name>`). The metastore's generic object API is
//! its pointers, and a pointer's value is at most 1 KiB (`MAX_KEY_LEN`) and pointers can be
//! neither listed nor deleted, so per-graph pointers could not hold a LINKED graph's mapping or
//! answer `ListGraphs` (ruling R4.1). A pointer to a per-namespace document gives atomic
//! multi-graph updates, AIP-158 listing and deletion without a new metastore type.
//!
//! Every write reads the current document, applies one change and CASes the pointer from the
//! version it read; a concurrent writer makes the CAS fail and the write retries on the new
//! document. A lost acknowledgement is recognised by the pointer already naming this write's own
//! object (each write's object key is unique).

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use bytes::Bytes;
use loams_common::NamespaceId;
use loams_common::meta::{ApplyError, Consistency, MetaError, MetaStore, PointerCas};
use loams_store::Store;
use serde::{Deserialize, Serialize};

use crate::GraphId;
use crate::engine::validate_names;

/// The metastore pointer, per namespace, that names the current catalog document.
pub const CATALOG_POINTER: &str = "graph-catalog";

/// The default `ListGraphs` page (AIP-158), and the largest a caller may ask for.
pub const DEFAULT_PAGE_SIZE: usize = 50;
/// The largest `ListGraphs` page.
pub const MAX_PAGE_SIZE: usize = 1000;

/// How long a superseded catalog document is kept before the sweep may delete it (review I1).
pub const DOCUMENT_GRACE: std::time::Duration = std::time::Duration::from_secs(600);

/// How many times a write retries a CAS that lost to a concurrent writer.
const MAX_ATTEMPTS: usize = 32;

/// Whether a graph's data is its own or derived from collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GraphMode {
    /// Written with GQL.
    #[default]
    Owned,
    /// Fed by links (Task 17).
    Linked,
}

/// Per-statement limits (§48 §13.1); zero takes the server's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GraphLimits {
    /// Statement timeout.
    pub timeout_ms: u32,
    /// Unary row cap.
    pub max_rows: u32,
    /// Unary byte cap.
    pub max_result_bytes: u64,
    /// The graph's memory budget.
    pub memory_bytes: u64,
    /// The longest variable-length path.
    pub max_path_hops: u32,
}

/// Where a graph is in its catalog life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum CatalogState {
    /// Serving.
    Ready,
    /// Deleted at `since_ms`; its storage is purged after the retention hold.
    Deleting {
        /// When the delete was accepted, metastore clock milliseconds.
        since_ms: u64,
    },
}

/// One graph's catalog record (shared contract `GraphMeta`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphMeta {
    /// `gr_<ULID>`; names the graph's storage, `<data_dir>/graphs/<id>/`.
    pub id: String,
    /// The namespace.
    pub namespace: String,
    /// The name, unique in the namespace among graphs that are not being deleted.
    pub name: String,
    /// OWNED or LINKED.
    pub mode: GraphMode,
    /// The languages the graph accepts, as `QueryLanguage` wire names (`GQL`).
    pub languages: Vec<String>,
    /// Statement limits.
    pub limits: GraphLimits,
    /// Followers beside the owner.
    pub replicas: u32,
    /// Ready or deleting.
    #[serde(flatten)]
    pub state: CatalogState,
    /// Creation time, metastore clock milliseconds.
    pub created_at_ms: u64,
    /// The record's CAS version: 1 at creation, +1 per update.
    pub version: u64,
    /// The idempotency key of the `CreateGraph` that made it, if it had one.
    pub idempotency_key: Option<String>,
    /// The idempotency key of the `DeleteGraph` that deleted it, if it had one.
    #[serde(default)]
    pub delete_key: Option<String>,
}

impl GraphMeta {
    /// The graph's id, parsed.
    ///
    /// # Errors
    ///
    /// [`CatalogError::Corrupt`] when the stored id is not a `gr_<ULID>`.
    pub fn graph_id(&self) -> Result<GraphId, CatalogError> {
        self.id
            .parse()
            .map_err(|_| CatalogError::Corrupt(format!("graph id {:?}", self.id)))
    }

    /// Whether the graph is being deleted.
    pub fn is_deleting(&self) -> bool {
        matches!(self.state, CatalogState::Deleting { .. })
    }
}

/// What a new graph is created with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewGraph {
    /// OWNED (LINKED is Task 17's).
    pub mode: GraphMode,
    /// Languages; empty means `["GQL"]`.
    pub languages: Vec<String>,
    /// Limits.
    pub limits: GraphLimits,
    /// Replicas.
    pub replicas: u32,
    /// `CreateGraph`'s idempotency key.
    pub idempotency_key: Option<String>,
}

/// A catalog failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    /// An invalid namespace or graph name, or page token.
    #[error("{0}")]
    Invalid(String),
    /// No such graph (or it is being deleted).
    #[error("graph {namespace}/{name} not found")]
    NotFound {
        /// The namespace.
        namespace: String,
        /// The name.
        name: String,
    },
    /// The name is taken by another graph.
    #[error("graph {namespace}/{name} already exists")]
    AlreadyExists {
        /// The namespace.
        namespace: String,
        /// The name.
        name: String,
    },
    /// `expected_version` did not match.
    #[error("graph {name} is at version {current}, not {expected}")]
    VersionMismatch {
        /// The name.
        name: String,
        /// The version the caller expected.
        expected: u64,
        /// The current version.
        current: u64,
    },
    /// The metastore or the bucket failed; retry.
    #[error("the graph catalog is unavailable: {0}")]
    Unavailable(String),
    /// A stored document did not read back.
    #[error("the graph catalog is corrupt: {0}")]
    Corrupt(String),
}

/// One namespace's catalog document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Document {
    graphs: BTreeMap<String, GraphMeta>,
}

/// A loaded document and where it came from.
struct Loaded {
    namespace: NamespaceId,
    /// The pointer's version, `None` when the namespace has no catalog yet.
    version: Option<u64>,
    doc: Document,
}

/// One page of `ListGraphs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// The graphs, by name.
    pub graphs: Vec<GraphMeta>,
    /// The token of the next page; `None` on the last.
    pub next_page_token: Option<String>,
}

/// The graph catalog over a metastore and a bucket.
#[derive(Clone)]
pub struct GraphCatalog {
    meta: Arc<dyn MetaStore>,
    store: Store,
    counters: Arc<Counters>,
}

/// What the catalog counts, for tests and (Task 27) metrics.
#[derive(Debug, Default)]
pub struct Counters {
    /// Reads that found their document swept and followed the pointer again.
    pub document_rereads: std::sync::atomic::AtomicU64,
}

impl std::fmt::Debug for GraphCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphCatalog").finish_non_exhaustive()
    }
}

fn unavailable(err: impl std::fmt::Display) -> CatalogError {
    CatalogError::Unavailable(err.to_string())
}

fn not_found(namespace: &str, name: &str) -> CatalogError {
    CatalogError::NotFound {
        namespace: namespace.to_string(),
        name: name.to_string(),
    }
}

fn check_names(namespace: &str, name: &str) -> Result<(), CatalogError> {
    validate_names(namespace, name).map_err(|err| CatalogError::Invalid(err.to_string()))
}

impl GraphCatalog {
    /// A catalog over `meta` and `store`.
    #[must_use]
    pub fn new(meta: Arc<dyn MetaStore>, store: Store) -> Self {
        Self {
            meta,
            store,
            counters: Arc::default(),
        }
    }

    /// The catalog's counters.
    #[must_use]
    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    /// The metastore clock, in milliseconds.
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        self.meta.now_ms()
    }

    /// Creates a graph, or answers the one a `CreateGraph` with the same idempotency key made.
    ///
    /// # Errors
    ///
    /// [`CatalogError::Invalid`], [`CatalogError::AlreadyExists`] when a graph with another (or
    /// no) idempotency key holds the name, or [`CatalogError::Unavailable`].
    pub async fn create(
        &self,
        namespace: &str,
        name: &str,
        new: NewGraph,
    ) -> Result<GraphMeta, CatalogError> {
        check_names(namespace, name)?;
        let ns = self.ensure_namespace(namespace).await?;
        let now = self.now_ms();
        let id = GraphId::new().to_string();
        self.write(ns, |doc| {
            if let Some(existing) = doc.graphs.get(name)
                && !existing.is_deleting()
            {
                if new.idempotency_key.is_some() && existing.idempotency_key == new.idempotency_key
                {
                    return Ok((None, existing.clone()));
                }
                return Err(CatalogError::AlreadyExists {
                    namespace: namespace.to_string(),
                    name: name.to_string(),
                });
            }
            let meta = GraphMeta {
                id: id.clone(),
                namespace: namespace.to_string(),
                name: name.to_string(),
                mode: new.mode,
                languages: if new.languages.is_empty() {
                    vec!["GQL".to_string()]
                } else {
                    new.languages.clone()
                },
                limits: new.limits,
                replicas: new.replicas,
                state: CatalogState::Ready,
                created_at_ms: now,
                version: 1,
                idempotency_key: new.idempotency_key.clone(),
                delete_key: None,
            };
            // A graph being deleted under this name keeps its record (under its id) until it
            // is purged, so the name is free for the new one.
            if let Some(old) = doc.graphs.remove(name) {
                doc.graphs.insert(deleting_key(&old.id), old);
            }
            doc.graphs.insert(name.to_string(), meta.clone());
            Ok((Some(()), meta))
        })
        .await
    }

    /// The graph `name`, if it exists and is not being deleted.
    ///
    /// # Errors
    ///
    /// [`CatalogError::Invalid`], [`CatalogError::NotFound`] or [`CatalogError::Unavailable`].
    pub async fn get_by_name(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<GraphMeta, CatalogError> {
        check_names(namespace, name)?;
        let Some(loaded) = self.load(namespace).await? else {
            return Err(not_found(namespace, name));
        };
        loaded
            .doc
            .graphs
            .get(name)
            .filter(|g| !g.is_deleting())
            .cloned()
            .ok_or_else(|| not_found(namespace, name))
    }

    /// The graph with id `id` in `namespace`, deleting ones included.
    ///
    /// # Errors
    ///
    /// [`CatalogError::NotFound`] or [`CatalogError::Unavailable`].
    pub async fn get(&self, namespace: &str, id: GraphId) -> Result<GraphMeta, CatalogError> {
        let id = id.to_string();
        let Some(loaded) = self.load(namespace).await? else {
            return Err(not_found(namespace, &id));
        };
        loaded
            .doc
            .graphs
            .values()
            .find(|g| g.id == id)
            .cloned()
            .ok_or_else(|| not_found(namespace, &id))
    }

    /// One page of the namespace's graphs, by name (AIP-158). The token names the last graph of
    /// the previous page, so inserts and deletes elsewhere never shift a page.
    ///
    /// # Errors
    ///
    /// [`CatalogError::Invalid`] for a bad namespace, size or token, or
    /// [`CatalogError::Unavailable`].
    pub async fn list(
        &self,
        namespace: &str,
        page_size: i32,
        page_token: &str,
    ) -> Result<Page, CatalogError> {
        check_names(namespace, "g")?;
        let size = match page_size {
            0 => DEFAULT_PAGE_SIZE,
            n if n < 0 => {
                return Err(CatalogError::Invalid(format!(
                    "page_size must not be negative, got {n}"
                )));
            }
            n => usize::try_from(n)
                .unwrap_or(MAX_PAGE_SIZE)
                .min(MAX_PAGE_SIZE),
        };
        let after = if page_token.is_empty() {
            None
        } else {
            Some(decode_token(namespace, page_token)?)
        };
        let Some(loaded) = self.load(namespace).await? else {
            return Ok(Page {
                graphs: Vec::new(),
                next_page_token: None,
            });
        };
        let mut live = loaded
            .doc
            .graphs
            .into_iter()
            .filter(|(key, g)| !g.is_deleting() && !key.starts_with(DELETING_PREFIX))
            .filter(|(key, _)| after.as_ref().is_none_or(|after| key > after))
            .map(|(_, g)| g);
        let graphs: Vec<GraphMeta> = live.by_ref().take(size).collect();
        let more = live.next().is_some();
        let next_page_token = match graphs.last() {
            Some(last) if more => Some(encode_token(namespace, &last.name)),
            _ => None,
        };
        Ok(Page {
            graphs,
            next_page_token,
        })
    }

    /// Applies `change` to the graph `name` at `expected_version` (any version when `None`),
    /// bumping its version.
    ///
    /// # Errors
    ///
    /// [`CatalogError::NotFound`], [`CatalogError::VersionMismatch`], or what `change` returns.
    pub async fn update(
        &self,
        namespace: &str,
        name: &str,
        expected_version: Option<u64>,
        change: impl Fn(&mut GraphMeta) -> Result<(), CatalogError> + Send + Sync,
    ) -> Result<GraphMeta, CatalogError> {
        check_names(namespace, name)?;
        let ns = self
            .namespace_id(namespace)
            .await?
            .ok_or_else(|| not_found(namespace, name))?;
        self.write(ns, |doc| {
            let graph = doc
                .graphs
                .get_mut(name)
                .filter(|g| !g.is_deleting())
                .ok_or_else(|| not_found(namespace, name))?;
            if let Some(expected) = expected_version
                && graph.version != expected
            {
                return Err(CatalogError::VersionMismatch {
                    name: name.to_string(),
                    expected,
                    current: graph.version,
                });
            }
            change(graph)?;
            graph.version += 1;
            Ok((Some(()), graph.clone()))
        })
        .await
    }

    /// Marks `name` as being deleted: it disappears from `get_by_name` and `list`, and its name
    /// is free; its record stays, under its id, until [`GraphCatalog::purge`]. A repeat with the
    /// same `delete_key` answers the record again.
    ///
    /// # Errors
    ///
    /// [`CatalogError::NotFound`] or [`CatalogError::Unavailable`].
    pub async fn mark_deleting(
        &self,
        namespace: &str,
        name: &str,
        delete_key: Option<String>,
    ) -> Result<GraphMeta, CatalogError> {
        check_names(namespace, name)?;
        let ns = self
            .namespace_id(namespace)
            .await?
            .ok_or_else(|| not_found(namespace, name))?;
        let now = self.now_ms();
        self.write(ns, |doc| {
            if let Some(graph) = doc.graphs.get(name).filter(|g| !g.is_deleting()) {
                let mut graph = graph.clone();
                doc.graphs.remove(name);
                graph.state = CatalogState::Deleting { since_ms: now };
                graph.delete_key = delete_key.clone();
                graph.version += 1;
                doc.graphs.insert(deleting_key(&graph.id), graph.clone());
                return Ok((Some(()), graph));
            }
            // A retried delete: the record is already under its id.
            if delete_key.is_some()
                && let Some(graph) = doc
                    .graphs
                    .values()
                    .find(|g| g.name == name && g.is_deleting() && g.delete_key == delete_key)
            {
                return Ok((None, graph.clone()));
            }
            Err(not_found(namespace, name))
        })
        .await
    }

    /// Graphs of `namespace` being deleted since before `before_ms`.
    ///
    /// # Errors
    ///
    /// [`CatalogError::Unavailable`].
    pub async fn deleting(
        &self,
        namespace: &str,
        before_ms: u64,
    ) -> Result<Vec<GraphMeta>, CatalogError> {
        let Some(loaded) = self.load(namespace).await? else {
            return Ok(Vec::new());
        };
        Ok(loaded
            .doc
            .graphs
            .into_values()
            .filter(
                |g| matches!(g.state, CatalogState::Deleting { since_ms } if since_ms <= before_ms),
            )
            .collect())
    }

    /// Every namespace with a catalog.
    ///
    /// # Errors
    ///
    /// [`CatalogError::Unavailable`].
    pub async fn namespaces(&self) -> Result<Vec<String>, CatalogError> {
        let all = self
            .meta
            .namespaces(Consistency::Linearizable)
            .await
            .map_err(unavailable)?;
        let mut out = Vec::new();
        for ns in all {
            if self
                .meta
                .pointer(Consistency::Linearizable, ns.id, CATALOG_POINTER)
                .await
                .map_err(unavailable)?
                .is_some()
            {
                out.push(ns.name);
            }
        }
        Ok(out)
    }

    /// Removes a deleting graph's record. Answers whether there was one.
    ///
    /// # Errors
    ///
    /// [`CatalogError::Unavailable`].
    pub async fn purge(&self, namespace: &str, id: &str) -> Result<bool, CatalogError> {
        let Some(ns) = self.namespace_id(namespace).await? else {
            return Ok(false);
        };
        self.write(ns, |doc| {
            let key = deleting_key(id);
            match doc.graphs.remove(&key) {
                Some(_) => Ok((Some(()), true)),
                None => Ok((None, false)),
            }
        })
        .await
    }

    // -----------------------------------------------------------------------------------------

    async fn namespace_id(&self, namespace: &str) -> Result<Option<NamespaceId>, CatalogError> {
        Ok(self
            .meta
            .namespace_by_name(Consistency::Linearizable, namespace)
            .await
            .map_err(unavailable)?
            .map(|ns| ns.id))
    }

    async fn ensure_namespace(&self, namespace: &str) -> Result<NamespaceId, CatalogError> {
        if let Some(id) = self.namespace_id(namespace).await? {
            return Ok(id);
        }
        match self.meta.create_namespace(namespace).await {
            Ok(id) | Err(MetaError::Rejected(ApplyError::NamespaceExists(id))) => Ok(id),
            Err(err) => Err(unavailable(err)),
        }
    }

    async fn load(&self, namespace: &str) -> Result<Option<Loaded>, CatalogError> {
        let Some(ns) = self.namespace_id(namespace).await? else {
            return Ok(None);
        };
        self.load_ns(ns).await.map(Some)
    }

    async fn load_ns(&self, namespace: NamespaceId) -> Result<Loaded, CatalogError> {
        let pointer = self
            .meta
            .pointer(Consistency::Linearizable, namespace, CATALOG_POINTER)
            .await
            .map_err(unavailable)?;
        let Some(pointer) = pointer else {
            return Ok(Loaded {
                namespace,
                version: None,
                doc: Document::default(),
            });
        };
        let mut pointer = pointer;
        for _ in 0..MAX_ATTEMPTS {
            match self.store.get(&pointer.value).await {
                Ok((bytes, _)) => {
                    let doc: Document = serde_json::from_slice(&bytes).map_err(|err| {
                        CatalogError::Corrupt(format!("{}: {err}", pointer.value))
                    })?;
                    return Ok(Loaded {
                        namespace,
                        version: Some(pointer.version),
                        doc,
                    });
                }
                // The document this pointer named was superseded and swept between the pointer
                // read and the GET (review I1): read the pointer again and follow it.
                Err(loams_store::StoreError::NotFound { .. }) => {
                    self.counters
                        .document_rereads
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    match self
                        .meta
                        .pointer(Consistency::Linearizable, namespace, CATALOG_POINTER)
                        .await
                        .map_err(unavailable)?
                    {
                        Some(next) if next.value != pointer.value => pointer = next,
                        // The pointer still names a document that is gone: nothing a retry fixes.
                        _ => {
                            return Err(CatalogError::Corrupt(format!(
                                "the current catalog document {} is missing",
                                pointer.value
                            )));
                        }
                    }
                }
                Err(err) => return Err(unavailable(err)),
            }
        }
        Err(CatalogError::Unavailable(
            "the catalog kept moving under this read; retry".to_string(),
        ))
    }

    /// Reads the document, applies `change` and commits it. `change` answers `(None, out)` to
    /// answer without writing.
    async fn write<T: Clone>(
        &self,
        namespace: NamespaceId,
        change: impl Fn(&mut Document) -> Result<(Option<()>, T), CatalogError>,
    ) -> Result<T, CatalogError> {
        for _ in 0..MAX_ATTEMPTS {
            let mut loaded = self.load_ns(namespace).await?;
            let (write, out) = change(&mut loaded.doc)?;
            if write.is_none() {
                return Ok(out);
            }
            let object = format!(
                "graphs/{}/catalog/{}.json",
                loaded.namespace,
                ulid::Ulid::generate()
            );
            let bytes = serde_json::to_vec(&loaded.doc)
                .map_err(|err| CatalogError::Corrupt(err.to_string()))?;
            self.store
                .put(&object, Bytes::from(bytes))
                .await
                .map_err(unavailable)?;
            let cas = self
                .meta
                .cas_pointer(PointerCas {
                    namespace: loaded.namespace,
                    key: CATALOG_POINTER.to_string(),
                    expected: loaded.version,
                    value: object.clone(),
                    fence: None,
                    fresh: None,
                })
                .await;
            match cas.result {
                Ok(_) => return Ok(out),
                // A lost acknowledgement of this very write: the pointer names its object.
                Err(MetaError::Rejected(ApplyError::VersionMismatch {
                    current: Some(current),
                })) if current.value == object => return Ok(out),
                // A concurrent writer won: redo the change on the new document. This attempt's
                // object is left for the sweep: nothing is ever deleted at once, because a CAS
                // reported as lost may have committed and been read (review I1).
                Err(MetaError::Rejected(ApplyError::VersionMismatch { .. })) => {}
                Err(err) => return Err(unavailable(err)),
            }
        }
        Err(CatalogError::Unavailable(
            "the catalog kept changing under this write; retry".to_string(),
        ))
    }

    /// Deletes catalog documents no reader can still need (review I1, M2): in every namespace,
    /// every object under `graphs/<namespace_id>/catalog/` except the pointer's current target
    /// and any object written within `grace`. That covers superseded documents and the orphans
    /// of CAS attempts that lost. Answers how many were deleted.
    ///
    /// A reader that read the pointer before a sweep and GETs a swept document re-reads the
    /// pointer ([`GraphCatalog::load_ns`]), so `grace` only bounds how often that happens.
    ///
    /// # Errors
    ///
    /// [`CatalogError::Unavailable`] when the namespaces cannot be listed; a failure in one
    /// namespace is logged and the sweep goes on.
    pub async fn sweep_documents(&self, grace: std::time::Duration) -> Result<usize, CatalogError> {
        let now_ms = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(u64::MAX);
        let cutoff = now_ms.saturating_sub(u64::try_from(grace.as_millis()).unwrap_or(u64::MAX));
        let namespaces = self
            .meta
            .namespaces(Consistency::Linearizable)
            .await
            .map_err(unavailable)?;
        let mut deleted = 0;
        for ns in namespaces {
            match self.sweep_namespace(ns.id, cutoff).await {
                Ok(n) => deleted += n,
                Err(err) => {
                    tracing::warn!(namespace = %ns.name, error = %err, "sweeping a graph catalog failed");
                }
            }
        }
        Ok(deleted)
    }

    async fn sweep_namespace(
        &self,
        namespace: NamespaceId,
        cutoff_ms: u64,
    ) -> Result<usize, CatalogError> {
        let objects = self
            .store
            .list(&format!("graphs/{namespace}/catalog/"))
            .await
            .map_err(unavailable)?;
        if objects.is_empty() {
            return Ok(0);
        }
        // Read the pointer after listing: a document written after this read is newer than the
        // cutoff anyway, and one the pointer moved to before it is kept.
        let current = self
            .meta
            .pointer(Consistency::Linearizable, namespace, CATALOG_POINTER)
            .await
            .map_err(unavailable)?
            .map(|p| p.value);
        let mut deleted = 0;
        for object in objects {
            if current.as_deref() == Some(object.path.as_str())
                || object.last_modified_ms > cutoff_ms
            {
                continue;
            }
            match self.store.delete(&object.path).await {
                Ok(()) | Err(loams_store::StoreError::NotFound { .. }) => deleted += 1,
                Err(err) => {
                    tracing::debug!(object = %object.path, error = %err, "could not sweep a graph catalog document");
                }
            }
        }
        Ok(deleted)
    }
}

/// The record key of a graph being deleted: under its id, so its name is free.
const DELETING_PREFIX: &str = "\u{0}deleting/";

fn deleting_key(id: &str) -> String {
    format!("{DELETING_PREFIX}{id}")
}

fn encode_token(namespace: &str, after: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("g1\n{namespace}\n{after}"))
}

fn decode_token(namespace: &str, token: &str) -> Result<String, CatalogError> {
    let invalid =
        || CatalogError::Invalid("page_token is not a token this list issued".to_string());
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| invalid())?;
    let text = String::from_utf8(bytes).map_err(|_| invalid())?;
    let mut parts = text.splitn(3, '\n');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("g1"), Some(ns), Some(after)) if ns == namespace => Ok(after.to_string()),
        _ => Err(invalid()),
    }
}
