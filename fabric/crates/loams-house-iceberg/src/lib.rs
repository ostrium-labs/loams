//! Lake-table reads for the Loams House Rust engine (design §75 §9; plan HE1
//! Task 26), and from HE2 the writes.
//!
//! A DataFusion `TableProvider` over a pinned Iceberg snapshot of a House lake
//! table: ClickHouse types restored from the House's encodings, pruning by
//! partition, statistics, page index and bloom filters, and position deletes
//! and deletion vectors applied.
