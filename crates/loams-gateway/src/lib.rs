//! `loams-gateway`: one Rust gateway with a module per protocol (design
//! §57, plan GW1). Its first module is Postgres's front door: it reads the
//! SNI of a client of `<ns>.<region>.pg.<base-domain>` and carries the
//! connection to that namespace's PgDog.
//!
//! - **`SSLRequest` clients are spliced** (D1014): the gateway answers `S`,
//!   reads the ClientHello's SNI, sends its own `SSLRequest` to PgDog and
//!   forwards the client's TLS bytes unchanged, so TLS runs end to end and
//!   PgDog holds the certificate.
//! - **Direct-TLS clients are terminated** (`sslnegotiation=direct`): the
//!   gateway selects ALPN `postgresql`, which PgDog does not, opens a
//!   second, verified TLS session to PgDog, and presents PgDog's own
//!   certificate to the client, because SCRAM-SHA-256-PLUS binds to it.
//! - **Plaintext is refused** (D1015), and **dedicated ports** route clients
//!   without SNI (D1016).
//! - **Routes** come from a file `pg-control` renders, hot-reloaded (D1018).
//!
//! The [`browser`] module carries agents' CDP sessions to the deployment's
//! headless browser service with the engine's own token (D2118).
//!
//! The [`risingwave`] module routes managed RisingWave instances
//! (`<name>.<ns>.<region>.rw.<base-domain>`) behind Loams auth (design 81).
//!
//! PgDog is an unmodified separate process (AGPL-3.0); nothing of it is in
//! this crate (§46 §8.3).

pub mod admin;
pub mod browser;
pub mod config;
pub mod io;
pub mod listeners;
pub mod observe;
pub mod pg;
pub mod reload;
pub mod risingwave;
pub mod routes;
pub mod sniff;
#[cfg(feature = "spark")]
pub mod spark;
#[cfg(feature = "temporal")]
pub mod temporal;
pub mod tls;

pub use config::{GatewayConfig, TlsMode};
pub use listeners::{Gateway, GatewayError};
pub use routes::{PgNamespace, RouteError, RouteTable};
