//! The Loams Marketplace, the Seed Exchange, in agentd (design §73, plan MK1):
//! the listing schema, sources pinned to signing identities, signed index
//! verification, revocations, search, consent plans and the agent
//! environment contract. Installers and the conformance runner follow in MK0
//! Tasks 6 and 8.

pub mod contract;
pub mod index;
pub mod install;
pub mod listing;
pub mod market;
pub mod plan;
pub mod provenance;
pub mod revocations;
pub mod sources;
pub mod verify;

pub use market::{Fetcher, ListingView, MarketError, Marketplace, Query, SourceInfo, Summary};
pub use sources::Source;
pub use verify::{Identity, SignatureVerifier, SigstoreVerifier};
