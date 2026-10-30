//! `loams-transport`: Loams' cluster transport (design §83, plan CT1).
//!
//! This first slice (CT1 Task 3, part 1) is the identity layer that both
//! transports share:
//!
//! - [`SpiffeId`]: the workload identities of D2308,
//!   `spiffe://<cluster>/node/<id>`, `…/role/<role>` and `…/svc/<service>`.
//! - [`tls`]: TLS 1.3-only `rustls` configurations for an internal listener
//!   (client certificates required, D2303) and an internal client, on
//!   aws-lc-rs (D2306), with 0-RTT off (D2318) and the ALPN of the transport
//!   (`h3` or `h2`, D2304).
//! - [`peer`]: the verified peer's identities, read from its leaf
//!   certificate's URI SANs, which route authorization keys on (D2315).
//!
//! Part 2 adds what issues and holds those certificates:
//!
//! - [`Identity`]: this process's chain, key and trust bundle, its validity
//!   window and renewal time (two-thirds of the leaf's life, D2309), and the
//!   TLS configurations built from it. Loaded from PEM files for an external
//!   CA (D2314) or built from what the CA issued.
//! - [`ca`]: the deployment's CA (D2309, D2310): a root and an intermediate,
//!   and leaves signed for CSRs ([`ca::new_key_and_csr`]), so private keys
//!   never leave the process that uses them.
//! - [`token`]: single-use join tokens (D2311) and the deny list (D2312).
//!
//! # Stability
//!
//! The types re-exported at the crate root are the API other lanes build on
//! (the cluster listener in CT1 Task 6, the desktop app's transport in design
//! 85). Changes to them are additive; a breaking change needs a CT1 ruling.
//! The CA service's RPC (`loams.ca.internal.v1`), the route policy and the
//! HTTP/3 endpoint are later parts of CT1 Tasks 3–7. Nothing in the product
//! uses this crate yet.

pub mod ca;
pub mod identity;
pub mod peer;
pub mod spiffe;
pub mod tls;
pub mod token;

pub use ca::{Ca, CaError, CaMaterial, Grant, Issued, new_key_and_csr};
pub use identity::{Identity, IdentityError};
pub use peer::{PeerError, PeerIdentity, VerifiedPeer, peer_identity, verified_peer};
pub use spiffe::{Kind, SpiffeId, SpiffeIdError};
pub use tls::{Alpn, TlsError, client_config, server_config};
pub use token::{DenyList, JoinToken, MemoryTokens, TokenError, TokenRecord, TokenStore};
