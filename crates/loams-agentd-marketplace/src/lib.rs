//! The Loams Marketplace, the Seed Exchange, in agentd (design §73, plan MK1):
//! the listing schema, sources pinned to signing identities, signed index
//! verification, revocations and search. Install plans, installers and the
//! conformance runner follow in MK0 Tasks 5, 6 and 8.

pub mod index;
pub mod listing;
pub mod market;
pub mod revocations;
pub mod sources;
pub mod verify;

pub use market::{Fetcher, ListingView, MarketError, Marketplace, Query, SourceInfo, Summary};
pub use sources::Source;
pub use verify::{Identity, SignatureVerifier, SigstoreVerifier};
