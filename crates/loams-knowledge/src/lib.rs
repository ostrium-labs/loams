//! The read-only project knowledge cache (MS1 Task 7, D1813).
//!
//! A **public** repository's knowledge set at one commit (Q1331) is stored
//! on Walrus as one quilt, one patch per file. A manifest maps each path to
//! its quilt patch id, SHA-256 and size. It is signed with Sigstore
//! (keyless, `cosign sign-blob --bundle`) and kept in the bucket, which
//! stays the source of truth (D1).
//!
//! - [`Publisher`] selects the knowledge set, stores the quilt, signs the
//!   manifest and writes it to the bucket. It refuses anything not public
//!   with [`KnowledgeError::NotPublic`] before any request.
//! - [`KnowledgeReader`] is what agents read through (agentd's MCP shim):
//!   it verifies the manifest's signature against the repository's pinned
//!   signer, then every entry's SHA-256. It has no write path.

mod config;
mod error;
mod manifest;
mod publish;
mod read;
mod search;
mod select;
mod sign;
mod source;

pub use config::{KnowledgeConfig, ReaderConfig, SignerRule};
pub use error::KnowledgeError;
pub use manifest::{KnowledgeManifest, Layout, MANIFEST_SCHEMA, ManifestEntry, VerifiedManifest};
pub use publish::{PublishRequest, Published, Publisher, Visibility};
pub use read::{KnowledgeFile, KnowledgeReader, TrustPolicy};
pub use search::{SearchHit, SearchResults};
pub use select::{FILES_ENTRY, KnowledgeToml, MARKER_PATH, MAX_ENTRY_BYTES};
pub use sign::{CosignSigner, ManifestSigner};
pub use source::{GitCheckout, KnowledgeSource, MemorySource, TrackedFile};

/// The Sigstore pieces callers need, re-exported from the marketplace.
pub use loams_agentd_marketplace::verify::{
    GITHUB_ACTIONS_ISSUER, Identity, SignatureError, SignatureVerifier, SigstoreVerifier,
};
