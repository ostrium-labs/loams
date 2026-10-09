//! loams-agentd-store — the daemon's local document store: [`DocsStore`] keeps SQLite
//! snapshots of the chat and workspace docs and the processed-command ledger
//! (mark-BEFORE-execute semantics). The doc IS the outbox: commands and user entries
//! flush immediately. The edge room clients that synced these docs are gone (D781).

// Lints the zeron fork never ran clippy against; plan DD1 rulings T1-12 and T1-13. ci.yml's
// workspace clippy already runs with -D warnings, so this list keeps it green until
// Tasks 2-4 delete or fix the code and drop it.
#![allow(clippy::question_mark, missing_debug_implementations)]

mod store;

pub use store::{DocsStore, StoreError};
