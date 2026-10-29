//! The marketplace in the engine (MK1 Task 4b): the slot the daemon puts its
//! [`MarketplaceHost`] in, the keyring vault for install secrets, and the
//! sync that turns installs into registrations.
//!
//! - An installed ACP harness becomes an [`AcpHarness::marketplace`] entry in
//!   the [`HarnessRegistry`], listed by `ListHarnesses` as
//!   [`HarnessId::Marketplace`] with its listing id (MK1 Task 0 ruling 7).
//! - An installed local MCP server is added to every run's
//!   `RunRequest::mcp_servers`, with its secrets read from the vault when the
//!   run starts.
//!
//! The sync runs once when the host is attached, after every install,
//! uninstall and update the engine serves, and whenever the host says its
//! registrations changed (a refresh that found a revocation).
//!
//! [`HarnessId::Marketplace`]: loams_agentd_proto::HarnessId::Marketplace

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use loams_agentd_harness::acp::AcpLaunch;
use loams_agentd_marketplace::service::{MarketplaceHost, McpServerRun, Vault};

use crate::registry::HarnessRegistry;

/// Where the engine finds the daemon's marketplace; empty until the daemon
/// attaches one (and in engines that run none, such as most tests).
#[derive(Clone, Default)]
pub struct MarketplaceSlot(Arc<OnceLock<MarketplaceHost>>);

impl std::fmt::Debug for MarketplaceSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("MarketplaceSlot")
            .field(&self.0.get().is_some())
            .finish()
    }
}

impl MarketplaceSlot {
    pub fn get(&self) -> Option<&MarketplaceHost> {
        self.0.get()
    }

    /// The installed local MCP servers, as a run passes them to its harness.
    pub fn mcp_servers(&self) -> Vec<loams_agentd_proto::McpServer> {
        self.get()
            .map(|host| {
                host.registrations()
                    .mcp_servers
                    .into_iter()
                    .map(to_mcp_server)
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn to_mcp_server(server: McpServerRun) -> loams_agentd_proto::McpServer {
    loams_agentd_proto::McpServer {
        name: server.name,
        command: server.command,
        args: server.args,
        env: server.env,
    }
}

/// Puts `host` in `slot` and keeps `registry` in step with its installs. A
/// second attach is ignored. Must run inside a Tokio runtime.
pub fn attach(slot: &MarketplaceSlot, registry: Arc<HarnessRegistry>, host: MarketplaceHost) {
    if slot.0.set(host.clone()).is_err() {
        tracing::warn!("a marketplace is already attached");
        return;
    }
    sync(&host, &registry);
    let mut changes = host.subscribe();
    tokio::spawn(async move {
        while changes.changed().await.is_ok() {
            sync(&host, &registry);
        }
    });
}

/// Registers exactly the installed ACP harnesses.
pub fn sync(host: &MarketplaceHost, registry: &HarnessRegistry) {
    let launches = host
        .registrations()
        .harnesses
        .into_iter()
        .map(|registration| AcpLaunch {
            cwd: Some(host.layout().data_dir(&registration.listing_id)),
            listing_id: registration.listing_id,
            display_name: registration.display_name,
            program: PathBuf::from(registration.program),
            args: registration.args,
            env: Default::default(),
        })
        .collect();
    registry.sync_marketplace(launches);
}

/// Install secrets in the OS keyring, service `dev.loams.agentd` (design §50
/// §12), one entry per secret.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringVault;

impl Vault for KeyringVault {
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(crate::change_request_create::KEYRING_SERVICE, key)
            .and_then(|entry| entry.set_password(value))
            .map_err(|error| format!("keyring_unavailable: {error}"))
    }

    fn get(&self, key: &str) -> Option<String> {
        keyring::Entry::new(crate::change_request_create::KEYRING_SERVICE, key)
            .and_then(|entry| entry.get_password())
            .ok()
    }

    fn delete(&self, key: &str) {
        if let Ok(entry) = keyring::Entry::new(crate::change_request_create::KEYRING_SERVICE, key) {
            let _ = entry.delete_credential();
        }
    }
}
