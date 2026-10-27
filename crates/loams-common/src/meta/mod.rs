//! The metastore as every backend and every caller shares it: the semantic
//! [`MetaStore`] trait (D47) with its read models and requests, catalog
//! records, coordination primitives (leases, fences, pointers), the errors a
//! metastore operation can return, and the consistency levels a read may ask
//! for (M1.2a plan).
//!
//! The records and errors are moved verbatim from `loams-meta`, which keeps everything
//! openraft-specific (`Command`, `Reply`, `MetaState`, the codec, `MetaNode`,
//! ...) and re-exports every name here at its crate root, so
//! `loams_meta::X` keeps compiling for existing callers.

mod error;
mod store;
mod types;
pub mod upgrade;
mod views;

pub use error::{ApplyError, MetaError, MetaResult, StaleLag, log_stale_object};
pub use store::{ChangeWait, Consistency, MetaChanges, MetaStopped, MetaStore, Tracked};
pub use types::{
    AliasAction, AliasTargetAction, AliasTargets, COLLECTION_KIND, COLLECTION_POINTER_PREFIX,
    Collection, EntryKind, Fence, Freshness, GIT_STREAM, GRAPH_STREAM_PREFIX, HotConfig,
    IdempotencyEntry, IdempotencyKey, IdempotencyState, IndexEntry, Lease, LeaseGrant, Link,
    LinkId, MAX_ALIAS_TARGETS, MAX_COLLECTION_NAME_LEN, MAX_IDEMPOTENCY_KEYS,
    MAX_IDEMPOTENCY_TTL_MS, MAX_KEY_LEN, MAX_LEASE_TTL_MS, MAX_NAME_LEN, MAX_PARTITIONS,
    METER_STREAM, METER_STREAM_PARTITIONS, METER_STREAM_RETENTION_MS, Namespace, Pointer,
    Retention, SYSTEM_NAMESPACE, SYSTEM_STREAMS, Stream, TargetRef, WAL_COMMIT_WINDOW_MS, WalChunk,
    WalClass, collection_pk_prefix, collection_pointer_key, collection_prefix, graph_stream_name,
    implicit_name, is_graph_id, is_reserved_namespace, legacy_graph_stream_name, link_pointer_key,
    system_stream_partitions, system_stream_retention,
};
pub use upgrade::{PRUNED, SNAPSHOT_ROOT_PREFIX, pinned_by, snapshot_gc_floor, snapshot_root_time};
pub use views::{
    CollectionHead, CollectionRoots, IdempotencyClaim, IdempotencyCompletion, LinkHead, NameTarget,
    PartitionBounds, PartitionIndex, PointerCas, SegmentSwap, StreamState, WalCommit,
};
