//! loams-agentd-store — the daemon's local document store: [`DocsStore`] keeps SQLite
//! snapshots of the chat and workspace docs and the processed-command ledger
//! (mark-BEFORE-execute semantics). The doc IS the outbox: commands and user entries
//! flush immediately. The edge room clients that synced these docs are gone (D781).

mod store;

pub use store::{DocsStore, StoreError};
