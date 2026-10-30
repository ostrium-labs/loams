//! `loams-ca`: the deployment's internal certificate authority (design §83
//! §6, plan CT1 Task 3).
//!
//! [`loams_transport`] has the certificate mechanics: the root and
//! intermediate ([`loams_transport::Ca`]), leaves built from CSRs, join
//! tokens and the deny list. This crate keeps them as one deployment's state
//! and serves them:
//!
//! - [`doc::CaDoc`]: the CA's state document. It holds the root certificate
//!   (never its key, which stays in the deployment's secret store), the
//!   intermediates with their keys sealed by age ([`seal::KeySealer`],
//!   D2205's pattern), the next serial, the join tokens (their secrets'
//!   digests only), the live issued serials and the deny list (§83 §6.3).
//! - [`store::CaDocStore`]: where the document lives, written with a version
//!   compare-and-swap. The server keeps it in the metastore and the bucket,
//!   as the principal directory and the meter's documents are kept
//!   (`crates/loams`, feature `transport`); tests keep it in memory.
//! - [`Authority`]: the operations, each one read-modify-CAS on the document:
//!   `init`, join tokens, revocation, intermediate rotation (§83 §6.6), and
//!   `Issue`, `Renew` and `TrustBundle` (§83 §6.3).
//! - [`connect`]: `loams.ca.internal.v1.CertificateService` over
//!   [`Authority`]. `Renew` reads the caller's [`loams_transport::VerifiedPeer`]
//!   from the request's extensions, which the mTLS listener puts there (CT1
//!   Task 6); without one it is `unauthenticated`.
//!
//! The service is the only writer of the document (§83 §6.3); the `loams ca`
//! CLI writes it through the same [`Authority`].

pub mod authority;
pub mod connect;
pub mod doc;
pub mod error;
pub mod seal;
pub mod store;

pub use authority::{Authority, Bundle, CertificateIssued, Revoke, RootKey, Status};
pub use error::CaServiceError;
pub use seal::KeySealer;
pub use store::{CaDocStore, MemoryDocStore};
