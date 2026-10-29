//! `loams-transfer`: resumable chunked transfers and job-run streams for Loams
//! Desktop (design §85, plan DQ1).
//!
//! The shape comes from a single-worker seeding script (§85 §2): every write
//! has a deterministic key, so a retry is harmless; a checkpoint commits with
//! the data; one writer holds a lease; work moves in fixed-size pieces; and a
//! resume never deletes what is committed.
//!
//! This first slice (DQ1 Task 1) is the server-side state:
//!
//! - [`checkpoint`]: the [`Checkpoints`] trait (leases and fenced pointer
//!   compare-and-swap), with an adapter over the metastore and an in-memory
//!   implementation for tests.
//! - [`journal`]: the fenced journal, immutable segments in the bucket plus a
//!   pointer whose swap is the commit (D2425).
//! - [`transfer`]: chunked transfers (D2422–D2433).
//! - [`run`]: job runs, their logs and artifacts (D2439–D2442).
//!
//! The HTTP API, the network, the client pipelines and the sidecar are later
//! tasks.

pub mod checkpoint;
pub mod error;
pub mod journal;
pub mod keys;
pub mod run;
pub mod transfer;

pub use checkpoint::{CheckpointError, Checkpoints, MemCheckpoints, MetaCheckpoints};
pub use error::{Error, Result};
pub use run::{ClaimedRun, LogRead, RunEntry, RunOutcome, RunView, Runs};
pub use transfer::{
    ChunkPut, CreateTransfer, TransferEntry, TransferState, TransferView, Transfers,
    TransfersConfig,
};
