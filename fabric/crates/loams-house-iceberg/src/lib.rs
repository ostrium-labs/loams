//! Lake-table reads for the Loams House Rust engine (design §75 §9; plan HE1
//! Task 26), and from HE2 the writes.
//!
//! [`provider`] gives a DataFusion `TableProvider` over a [`TablePin`], a
//! pinned Iceberg snapshot of a House lake table:
//!
//! * [`provider::LakeTable`] reads the snapshot with the fork's `iceberg`
//!   (metadata, manifests, file planning, deletes) and DataFusion's Parquet
//!   source (data), all through the caller's `object_store` ([`io`]);
//! * [`encodings`] restores each column's ClickHouse type from the House's
//!   Iceberg encodings, as `loams-house`'s read views do;
//! * [`prune`] turns the scan's filters into partition, file-statistics,
//!   page-index and bloom-filter pruning;
//! * [`deletes`] applies v2 position deletes and v3 deletion vectors;
//! * [`virtual_columns`] adds `__loams_seq`, `__loams_row` and row lineage;
//! * [`computed`] computes `DEFAULT`s of added columns, `ALIAS` columns and
//!   the delete `TTL` in a view DataFusion inlines.

pub mod computed;
pub mod deletes;
pub mod encodings;
pub mod io;
pub mod pin;
pub mod provider;
pub mod prune;
pub mod virtual_columns;

pub use computed::provider;
pub use pin::{ChColumn, ChTableInfo, ReadMode, TablePin};
pub use provider::{LakeTable, ScanReport};
