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
//! The CA service, join tokens, renewal and the route policy are later parts
//! of Task 3 and Task 4. Nothing in the product uses this crate yet.

pub mod peer;
pub mod spiffe;
pub mod tls;

pub use peer::{PeerIdentity, peer_identity};
pub use spiffe::{Kind, SpiffeId, SpiffeIdError};
pub use tls::{Alpn, TlsError, client_config, server_config};
