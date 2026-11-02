//! The Loams Marketplace, the Seed Exchange, in agentd (design §73, plan MK1):
//! the listing schema, sources pinned to signing identities, signed index
//! verification, revocations, search, consent plans and the agent
//! environment contract, installers and the conformance runner.
//!
//! Features (OP2-B Task 1, D2605): `agentd` (default) is agentd's half and
//! implies `fetch`; `fetch` is the `Fetcher` trait and its HTTPS client;
//! `testkit` is a fake world for tests. With none, the crate is the listing
//! core: `listing` (schemas v1 and v2), `index`, `sources`, `revocations`
//! and `verify`.

#[cfg(feature = "agentd")]
pub mod artifact;
#[cfg(feature = "agentd")]
pub mod conformance;
#[cfg(feature = "agentd")]
pub mod contract;
#[cfg(feature = "agentd")]
pub mod engine;
#[cfg(feature = "fetch")]
pub mod fetch;
#[cfg(feature = "agentd")]
pub mod identity;
pub mod index;
#[cfg(feature = "agentd")]
pub mod install;
pub mod listing;
#[cfg(feature = "agentd")]
pub mod market;
#[cfg(feature = "agentd")]
pub mod plan;
#[cfg(feature = "agentd")]
pub mod provenance;
pub mod revocations;
#[cfg(feature = "agentd")]
pub mod service;
pub mod sources;
#[cfg(feature = "testkit")]
pub mod testkit;
pub mod verify;

#[cfg(feature = "agentd")]
pub use artifact::{ArtifactFetch, ArtifactSources, FetchError, Origin};
#[cfg(feature = "agentd")]
pub use engine::{Engine, EngineError, EngineProbe, ResolvedEngine};
#[cfg(feature = "fetch")]
pub use fetch::{Fetcher, HttpsFetcher};
#[cfg(feature = "agentd")]
pub use market::{ListingView, MarketError, Marketplace, Query, SourceInfo, Summary};
#[cfg(feature = "agentd")]
pub use service::{Deps, MarketplaceHost, MemoryVault, Registrations, ServiceError, Vault};
pub use sources::Source;
pub use verify::{Identity, SignatureVerifier, SigstoreVerifier};
