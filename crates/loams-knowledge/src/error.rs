//! The knowledge cache's typed errors.

use loams_agentd_marketplace::verify::SignatureError;
use loams_walrus::WalrusError;

/// An error of publishing or reading the knowledge cache.
#[derive(Debug, thiserror::Error)]
pub enum KnowledgeError {
    /// The repository is not marked public: nothing was sent anywhere.
    /// Walrus blobs are public and cannot be made private later.
    #[error("refusing to publish {repo}: not a public repository ({reason})")]
    NotPublic {
        /// The repository.
        repo: String,
        /// Why it does not count as public.
        reason: String,
    },
    /// No signer is pinned for this repository, so nothing it serves can
    /// be trusted.
    #[error("no trusted signer is configured for repository {0}")]
    UnknownRepo(String),
    /// No manifest (or no `latest` pointer) in the bucket.
    #[error("no knowledge manifest for {repo} at {commit}")]
    NoManifest {
        /// The repository.
        repo: String,
        /// The commit, or `latest`.
        commit: String,
    },
    /// The manifest's signature is missing or does not verify for the
    /// pinned signer. Nothing in it was used.
    #[error("the knowledge manifest of {repo} at {commit} is not trusted: {error}")]
    Signature {
        /// The repository.
        repo: String,
        /// The commit.
        commit: String,
        /// Why the bundle was refused.
        error: SignatureError,
    },
    /// A manifest that does not parse or breaks its own rules.
    #[error("invalid knowledge manifest: {0}")]
    InvalidManifest(String),
    /// The path is not in the manifest.
    #[error("{path} is not in the knowledge cache of {repo} at {commit}")]
    NotInManifest {
        /// The repository.
        repo: String,
        /// The commit.
        commit: String,
        /// The path asked for.
        path: String,
    },
    /// Bytes whose SHA-256 is not the manifest's. They were dropped.
    #[error("{path}: expected sha256 {expected}, got {actual}")]
    Digest {
        /// The path read.
        path: String,
        /// The manifest's digest.
        expected: String,
        /// The digest of what arrived.
        actual: String,
    },
    /// The knowledge set is larger than one quilt may be.
    #[error("the knowledge set is {size} bytes, over the {max}-byte quilt limit")]
    TooLarge {
        /// The set's size.
        size: u64,
        /// The limit (`max_blob_bytes`).
        max: u64,
    },
    /// A bad argument or config.
    #[error("{0}")]
    Invalid(String),
    /// The repository could not be read.
    #[error("reading the repository: {0}")]
    Source(String),
    /// Signing failed.
    #[error("signing the manifest: {0}")]
    Sign(String),
    /// The Walrus fast path failed, including an integrity failure on
    /// every aggregator.
    #[error(transparent)]
    Walrus(#[from] WalrusError),
    /// The bucket failed.
    #[error("bucket: {0}")]
    Store(#[from] loams_store::StoreError),
}
