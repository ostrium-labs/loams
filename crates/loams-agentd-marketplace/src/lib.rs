//! The Loams Marketplace, the Seed Exchange, in agentd (design §73, plan MK1):
//! the listing schema, sources pinned to signing identities, signed index
//! verification, revocations, search, consent plans and the agent
//! environment contract. Installers and the conformance runner follow in MK0
//! Tasks 6 and 8.

pub mod artifact;
pub mod conformance;
pub mod contract;
pub mod engine;
pub mod fetch;
pub mod identity;
pub mod index;
pub mod install;
pub mod listing;
pub mod market;
pub mod plan;
pub mod provenance;
pub mod revocations;
pub mod service;
pub mod sources;
#[cfg(feature = "testkit")]
pub mod testkit;
pub mod verify;

pub use artifact::{ArtifactFetch, ArtifactSources, FetchError, Origin};
pub use engine::{Engine, EngineError, EngineProbe, ResolvedEngine};
pub use fetch::HttpsFetcher;
pub use market::{Fetcher, ListingView, MarketError, Marketplace, Query, SourceInfo, Summary};
pub use service::{Deps, MarketplaceHost, MemoryVault, Registrations, ServiceError, Vault};
pub use sources::Source;
pub use verify::{Identity, SignatureVerifier, SigstoreVerifier};
