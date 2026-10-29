//! The daemon's marketplace (MK1 Task 4b): the host over the daemon
//! directory, with the HTTPS fetcher, Sigstore's public-good root, the npm
//! and node agentd finds, and the keyring vault. The engine serves
//! `MarketplaceService` from it; a task refreshes its sources at start and
//! every 6 hours.

use std::path::Path;
use std::sync::Arc;

use loams_agentd_marketplace::service::{Deps, NPM_REGISTRY, NoNpm, ProcessNpm, REFRESH_INTERVAL};
use loams_agentd_marketplace::{HttpsFetcher, MarketplaceHost, SigstoreVerifier};

/// Opens the marketplace under `dir` (`marketplace/`, `agents/`).
///
/// # Errors
///
/// When the fetcher or the trusted root cannot be built, or `sources.json`
/// cannot be read.
pub fn open(dir: &Path) -> anyhow::Result<MarketplaceHost> {
    #[cfg(feature = "e2e-marketplace")]
    if test_marketplace() {
        tracing::warn!(
            "{TEST_MARKETPLACE_ENV} is set: the marketplace serves the testkit's signed fake index (tests only)"
        );
        let deps = loams_agentd_marketplace::testkit::World::new(dir).deps();
        return MarketplaceHost::open(deps).map_err(anyhow::Error::new);
    }
    let (node, npm) = loams_agentd_harness::find_node_and_npm();
    let npm: Arc<dyn loams_agentd_marketplace::install::Npm> = match npm {
        Some(npm) => Arc::new(ProcessNpm {
            npm,
            node: node.clone(),
        }),
        None => Arc::new(NoNpm),
    };
    let deps = Deps {
        data_dir: dir.to_owned(),
        fetcher: Arc::new(HttpsFetcher::new().map_err(anyhow::Error::msg)?),
        verifier: Arc::new(SigstoreVerifier::production().map_err(anyhow::Error::new)?),
        npm,
        vault: Arc::new(loams_agentd_sessions::marketplace::KeyringVault),
        node,
        registry: NPM_REGISTRY.into(),
        // Installed agents' tokens (IA4 Task 3, D1618, D1629-D1633).
        identity: loams_agentd_marketplace::identity::AgentIdentity::from_env()
            .map_err(anyhow::Error::msg)?,
        machine_users: loams_agentd_marketplace::identity::ZitadelUsers::from_env()
            .map_err(anyhow::Error::msg)?
            .map(|u| Arc::new(u) as Arc<dyn loams_agentd_marketplace::identity::MachineUsers>),
    };
    MarketplaceHost::open(deps).map_err(anyhow::Error::new)
}

/// In a debug build with the `e2e-marketplace` feature, a non-empty value opens
/// the testkit's world instead of
/// the network (MK0 Task 9: Studio's e2e installs `testkit/echo-acp`): the
/// official source served by a fake fetcher, stand-in signatures, a fake npm
/// and a memory vault. A release build ignores it, as it does
/// `LOAMS_AGENTD_TEST_ISSUER`.
pub const TEST_MARKETPLACE_ENV: &str = "LOAMS_AGENTD_TEST_MARKETPLACE";

#[cfg(feature = "e2e-marketplace")]
fn test_marketplace() -> bool {
    cfg!(debug_assertions) && std::env::var_os(TEST_MARKETPLACE_ENV).is_some_and(|v| !v.is_empty())
}

/// Opens the marketplace, attaches it to `core` and starts its refresh.
/// A failure is logged: the daemon serves everything else without it.
pub fn start(dir: &Path, core: &loams_agentd_sessions::EngineCore) {
    match open(dir) {
        Ok(host) => {
            // The task ends with the runtime.
            drop(host.spawn_refresh(REFRESH_INTERVAL));
            core.attach_marketplace(host);
        }
        Err(error) => tracing::warn!(%error, "the marketplace is not running"),
    }
}
