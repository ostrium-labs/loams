//! The Loams SDK for Rust (design §44 §9 row 4, SDK2 Task 3).
//!
//! One client object with namespaced modules over the unified Connect API,
//! generated from `proto/`:
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use loams::{ApiKey, CallOptionsOverrides, GetInstanceRequest, Loams, TransportOptions};
//!
//! # async fn run() -> Result<(), loams::LoamsError> {
//! let loams = Loams::connect(TransportOptions::new("http://127.0.0.1:8080"))?
//!     .with_auth(Arc::new(ApiKey::new(std::env::var("LOAMS_API_KEY").unwrap_or_default())?));
//!
//! // The first call any client makes: what this instance is, and no auth.
//! let info = loams.instance().get_instance(GetInstanceRequest::default(), CallOptionsOverrides::new()).await?;
//! println!("{} serves {:?}", info.name, info.api_versions);
//!
//! // Feature-detect before calling (R5), rather than reading the refusal.
//! loams.system().guard("live").await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## What is where
//!
//! | Concern | Module | Contract clause |
//! |---|---|---|
//! | The client object and its modules | [`client`] | §44 §7.1 |
//! | The generated binding table | [`facade`] | D606, §44 §7.3 |
//! | Credentials | [`token`] | R1, D608 |
//! | Retry classes and backoff | [`retry`] | R2, D610 |
//! | The call path | [`call`] | R1–R4 |
//! | Idempotency keys, consistency tokens | [`request`] | R3, R4 |
//! | Typed errors | [`error`] | R8, D611 |
//! | Pagination | [`pagination`] | R6 |
//! | Server streams and resume | [`streams`] | R7 |
//! | Feature detection and versions | [`system`] | R5, R9 |
//! | The transport | [`transport`] | D600, D128 |
//!
//! ## The stack (D128)
//!
//! **connect-rust** (`connectrpc`), the server's own stack, and the generated
//! clients of `loams-proto` and `loams-live-proto` — the same build the server
//! compiles them with. `tonic` is deliberately **not** a dependency: the SDK2
//! plan rules it out for this SDK, and a second gRPC stack would mean two
//! codegen pipelines and two error shapes for the same protos. See D742.
//!
//! ## What is not here yet, and why
//!
//! These are shortfalls of the *task's wording*, not of this implementation, and
//! each is recorded with the work it waits on:
//!
//! * **No typed hybrid-query builder.** No hybrid query RPC exists:
//!   `loams.collection.v1` and `loams.query.v1` arrive with API1 Tasks 2 and 4,
//!   and Q604 (one builder or thirteen) is unanswered. A builder written now
//!   would be a hand-written API with no proto behind it, which is exactly what
//!   §44 §7.3 says not to do.
//! * **No `bulk`, no `QueryArrow`.** The write RPCs land with the write paths
//!   (API1 Tasks 3 and 4), and the API has no bidi (D420), so there is nothing
//!   to stream a request over. [`arrow-flight`] is the planned transport
//!   (§44 §7.5) and is not a dependency today.
//! * **No per-call `list_all` alias.** The paging **iterator**
//!   ([`pagination::paginate`]) is here and is tested; a generated alias needs a
//!   generated signature to hang on, which arrives with `ListCollections`.
//! * **The facade table is hand-written.** SDK1 Task 3's Rust renderer has not
//!   landed; see [`facade`] and D744.
//!
//! [`arrow-flight`]: https://docs.rs/arrow-flight

#![deny(missing_docs)]
#![warn(missing_debug_implementations)]

pub mod call;
pub mod client;
pub mod error;
pub mod facade;
pub mod pagination;
pub mod reason;
pub mod request;
pub mod retry;
pub mod streams;
pub mod system;
pub mod token;
pub mod transport;
pub mod uuidv7;

pub use crate::call::{Attempt, CallOptionsOverrides, RetryPlan, call_with_retry};
pub use crate::client::{InstanceModule, LiveModule, Loams, TablesModule};
pub use crate::error::{ErrorInfoShape, ErrorKind, LoamsError};
pub use crate::facade::{
    CallBinding, ModuleBinding, PROTO_PACKAGES, PROTO_REV, Pagination, Streaming,
};
pub use crate::pagination::{PageRequest, PageResponse, PagedCall, paginate};
pub use crate::reason::{FEATURE_NOT_IN_VARIANT, Reason, TOKEN_EXPIRED, UnknownReason};
pub use crate::request::{
    Consistency, ConsistencySession, KeyedRequest, idempotency_key, with_idempotency_key,
};
pub use crate::retry::{DEFAULT_MAX_RETRIES, RetryClass};
pub use crate::streams::{Opened, WatchOptions};
pub use crate::system::{Catalogue, System, VersionReport};
pub use crate::token::{
    ApiKey, EnvToken, FormPoster, OidcExchange, Refreshing, StaticToken, TokenSource,
};
pub use crate::transport::{TransportOptions, WireProtocol, transport};

// Re-exported so a caller needs one crate, not three: the generated messages and
// the facade's call options are the two things a caller always touches.
pub use connectrpc::ErrorCode as Code;
pub use loams_live_proto::loams::live::v1 as live_v1;
pub use loams_proto::loams::instance::v1 as instance_v1;

/// The RPC `Loams::watch` resumes, named for the error a failure carries.
pub(crate) const WATCH_RPC: &str = "loams.live.v1.LiveService/Watch";
