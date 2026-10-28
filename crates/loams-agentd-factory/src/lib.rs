//! loams-agentd-factory — the software factory's app connections in the daemon
//! (SF1 amendment F1-F6; design §50 §13, D795).
//!
//! The daemon holds the apps' credentials (the OS keyring, §50 §12) and runs the
//! read-only op allowlist (D660) for the desktop, the browser console's Studio and
//! the agents' `factory_query` tool. The ops are a Rust port of `factory/ops.ts` and
//! of the adapter calls they make; `tests/parity.rs` replays the TypeScript ops'
//! recorded upstream against this port.

pub mod apps;
pub mod client;
pub mod error;
pub mod host;
pub mod json;
pub mod ops;
pub mod params;
pub mod redact;
pub mod store;
pub mod upstream;

pub use apps::AppId;
pub use error::{OpError, OpErrorKind};
pub use host::FactoryHost;
pub use store::{ConfigStore, KeyringSecrets, MemorySecrets, SecretStore};
pub use upstream::{HttpUpstream, Upstream, UpstreamRequest, UpstreamResponse};

use std::path::Path;
use std::sync::Arc;

/// The daemon's factory: the keyring when it answers (configuration in
/// `<dir>/factory.json`), else memory only.
///
/// # Errors
///
/// The HTTP client could not be built.
pub fn open(dir: &Path) -> Result<FactoryHost, String> {
    let upstream: Arc<dyn Upstream> = Arc::new(HttpUpstream::new()?);
    Ok(match KeyringSecrets::probe() {
        Some(keyring) => FactoryHost::new(
            ConfigStore::open(Some(dir.join("factory.json"))),
            Arc::new(keyring),
            upstream,
        ),
        None => {
            tracing::warn!("no keyring: the factory's credentials last until the daemon stops");
            FactoryHost::new(
                ConfigStore::open(None),
                Arc::new(MemorySecrets::default()),
                upstream,
            )
        }
    })
}
