//! `loams-transfer`: resumable chunked transfers and job-run streams for Loams
//! Desktop (design §85, plan DQ1).
//!
//! The shape comes from a single-worker seeding script (§85 §2): every write
//! has a deterministic key, so a retry is harmless; a checkpoint commits with
//! the data; one writer holds a lease; work moves in fixed-size pieces; and a
//! resume never deletes what is committed.
//!
//! The server-side state (DQ1 Task 1):
//!
//! - [`checkpoint`]: the [`Checkpoints`] trait (leases and fenced pointer
//!   compare-and-swap), with an adapter over the metastore and an in-memory
//!   implementation for tests.
//! - [`journal`]: the fenced journal, immutable segments in the bucket plus a
//!   pointer whose swap is the commit (D2425).
//! - [`transfer`]: chunked transfers (D2422–D2433).
//! - [`run`]: job runs, their logs and artifacts (D2439–D2442).
//!
//! The API and the network (DQ1 Task 2):
//!
//! - [`server`]: the axum router of §85 §6 and §11, with an [`Authorizer`](server::Authorizer).
//! - [`net`]: HTTP/3 on quinn and h3, HTTP/2+TLS, and the race between them.
//! - [`api`]: a typed client over either transport.
//! - [`standalone`]: `loams-transfer serve`, the API without the engine.
//!
//!
//! The client (DQ1 Task 3):
//!
//! - [`client`]: resumable upload and download pipelines with bounded
//!   memory, the local state, progress, and following a run's log.
//! - [`agent`]: the desktop sidecar, JSON lines on stdio.
//!
//! Job runs (DQ1 Task 4): [`runner`], which claims runs of configured jobs
//! and streams their logs and artifacts back.

pub mod agent;
pub mod api;
pub mod checkpoint;
pub mod client;
pub mod error;
pub mod journal;
pub mod keys;
pub mod net;
pub mod run;
pub mod runner;
pub mod server;
pub mod standalone;
pub mod transfer;

pub use checkpoint::{CheckpointError, Checkpoints, MemCheckpoints, MetaCheckpoints};
pub use error::{Error, Result};
pub use run::{ClaimedRun, LogRead, RunEntry, RunOutcome, RunView, Runs};
pub use transfer::{
    ChunkPut, CreateTransfer, TransferEntry, TransferState, TransferView, Transfers,
    TransfersConfig,
};
