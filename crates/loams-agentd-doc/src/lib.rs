//! loams-agentd-doc — session & workspace Loro doc schemas and the typed mirror layer.
//!
//! Port of loams-desktop's `packages/session-doc`. The schema SHAPE (container names, part maps with
//! LoroText bodies, command entries) is kept identical to the TS implementation so the edge's
//! tail materializer and any TS peer remain compatible.
//!
//! Load-bearing invariant (measured in loams-desktop, `oplog-shape.test.ts`): message parts are a
//! LoroList of part maps whose text bodies live in **LoroText** — streaming appends RLE-merge at
//! ~1.03x oplog overhead, whereas rewriting whole part values costs ~125x.

// Lints the zeron fork never ran clippy against; plan DD1 rulings T1-12 and T1-13. ci.yml's
// workspace clippy already runs with -D warnings, so this list keeps it green until
// Tasks 2-4 delete or fix the code and drop it.
#![allow(clippy::large_enum_variant, clippy::explicit_counter_loop)]

pub mod commands;
pub mod constants;
pub mod parts;
pub mod queue;
pub mod rebuild;
pub mod registry;
pub mod schema;
pub mod transcript_delta;
pub mod workspace;

pub use commands::*;
pub use constants::*;
pub use parts::*;
pub use queue::*;
pub use rebuild::*;
pub use registry::*;
pub use schema::*;
pub use transcript_delta::*;
pub use workspace::*;
