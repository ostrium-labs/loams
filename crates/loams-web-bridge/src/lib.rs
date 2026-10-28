//! The web toolbox's browser providers (design §37 §18.14; decisions D565,
//! D627, D2110).
//!
//! One tool contract — a snapshot with uids, `find`, `click`, `fill`,
//! `wait_for`, a screenshot, a content extraction — and one engine behind it,
//! Obscura, wherever the browser runs:
//!
//! * `local` ([`ProviderKind::Local`]): agentd or Desktop starts
//!   `obscura serve` on loopback on the user's machine
//!   ([`obscura::launch::LocalEngine`]);
//! * `service` ([`ProviderKind::Service`]): the same engine image runs as the
//!   `browser` service of a self-hosted or cloud deployment (deploy/browser),
//!   reached on the private network ([`obscura::ServiceEndpoint`]).
//!
//! Both are the one [`ObscuraProvider`], so the network policy, the credential
//! boundary, the budgets and recording are the same code with the same
//! defaults (D2112). [`from_config`] builds whichever the configuration
//! (`LOAMS_BROWSER_PROVIDER=local|service`) selects. [`local::LocalProvider`]
//! stays as the seam for a host that drives its own page (and for the
//! deterministic driver in [`testing`]).
//!
//! **Cloudflare Browser Run was removed on 2026-10-27 (D2110).** The owner's
//! direction is one engine, Obscura, locally and in the cloud;
//! `docs/remote-browser-provider.md` records what the removed provider was.

pub mod artifact;
pub mod cdp;
pub mod config;
pub mod egress;
pub mod error;
pub mod local;
pub mod obscura;
pub mod page;
pub mod provider;
pub mod record;
pub mod redact;
pub mod registry;
pub mod secret;
pub mod tool;
pub mod webmcp;

#[cfg(any(test, feature = "test-util"))]
pub mod testing;

pub use config::{
    BudgetConfig, CredentialPolicy, Engine, LocalConfig, PINNED_OBSCURA_VERSION, ProviderKind,
    RecordingConfig, ServiceConfig, WebBridgeConfig,
};
pub use egress::{EgressError, EgressPolicy, HostPattern};
pub use error::{BridgeError, ConfigError};
pub use local::{DriverAction, DriverPage, LocalProvider, PageDriver};
pub use obscura::launch::LocalEngine;
pub use obscura::{EngineEndpoint, ObscuraProvider, Policies, ServiceEndpoint};
pub use page::PageState;
pub use provider::{BrowserProvider, Capabilities, OpenRequest, PageRef};
pub use registry::ProviderRegistry;
pub use secret::{EnvSecretResolver, MapSecretResolver, SecretRef, SecretResolver, SecretValue};
pub use tool::{
    ActionOutcome, AxNode, Extracted, FillValue, FindQuery, Navigation, Screenshot, Snapshot,
    SnapshotMode, SnapshotNode, SnapshotRequest, Uid, WaitCondition, WaitOutcome, WaitRequest,
};
pub use webmcp::{
    Availability, CallWebmcpToolRequest, ListWebmcpToolsRequest, WebmcpAbsent, WebmcpCallOutcome,
    WebmcpListing, WebmcpRequest, WebmcpTool, WebmcpToolName,
};

use std::sync::Arc;

/// Build the provider the configuration selects, over the real WebSocket
/// transport.
pub fn from_config(
    config: &WebBridgeConfig,
    secrets: Arc<dyn SecretResolver>,
) -> Result<Arc<dyn BrowserProvider>, BridgeError> {
    from_config_with(config, secrets, Arc::new(cdp::ws::WebSocketTransport))
}

/// Build the provider the configuration selects, over `transport` (a test
/// passes a scripted one).
pub fn from_config_with(
    config: &WebBridgeConfig,
    secrets: Arc<dyn SecretResolver>,
    transport: Arc<dyn cdp::CdpTransport>,
) -> Result<Arc<dyn BrowserProvider>, BridgeError> {
    config.validate()?;
    let policies = policies(config)?;
    let endpoint: Arc<dyn EngineEndpoint> = match config.provider {
        ProviderKind::Local => {
            Arc::new(LocalEngine::new(&config.local, config.allow_private_hosts))
        }
        ProviderKind::Service => {
            Arc::new(ServiceEndpoint::new(&config.service, Arc::clone(&secrets))?)
        }
    };
    Ok(Arc::new(ObscuraProvider::new(
        endpoint, policies, secrets, transport,
    )))
}

/// The policies a configuration describes, the same for every provider.
pub fn policies(config: &WebBridgeConfig) -> Result<Policies, BridgeError> {
    Ok(Policies {
        egress: config.egress()?,
        credentials: config.credentials.clone(),
        budgets: config.budgets.clone(),
        recording: config.recording.clone(),
        artifacts: config.artifacts_dir(),
    })
}

/// Build the local provider over a host's driver.
pub fn local_provider(
    config: &WebBridgeConfig,
    driver: Arc<dyn PageDriver>,
    secrets: Arc<dyn SecretResolver>,
) -> Result<Box<dyn BrowserProvider>, BridgeError> {
    Ok(Box::new(LocalProvider::new(
        driver,
        config.egress()?,
        config.credentials.clone(),
        secrets,
        config.artifacts_dir(),
    )))
}
