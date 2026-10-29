//! Who is calling (D1507 as amended by D1600; plan DX1 X1.R5).

use std::collections::HashMap;

use sha2::{Digest, Sha256};
use tonic::Status;
use tonic::metadata::MetadataMap;

/// An authenticated caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    /// The Loams namespace the credential belongs to.
    pub loams_namespace_id: String,
    /// May register namespaces.
    pub admin: bool,
}

/// Turns a request's metadata into a principal.
pub trait Authenticator: Send + Sync + 'static {
    /// The caller, or `UNAUTHENTICATED`.
    fn authenticate(&self, metadata: &MetadataMap) -> Result<Principal, Status>;
}

/// `lk_` API keys, checked by their SHA-256 against a fixed table (the
/// shape a later `loams-authz` adapter answers).
#[derive(Debug, Clone, Default)]
pub struct StaticKeys {
    by_hash: HashMap<String, Principal>,
}

impl StaticKeys {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds the key whose SHA-256 (lowercase hex) is `sha256_hex`.
    pub fn insert(&mut self, sha256_hex: impl Into<String>, principal: Principal) {
        self.by_hash
            .insert(sha256_hex.into().to_ascii_lowercase(), principal);
    }

    /// The SHA-256 hex of a key, as the table stores it.
    pub fn hash(key: &str) -> String {
        hex::encode(Sha256::digest(key.as_bytes()))
    }
}

impl Authenticator for StaticKeys {
    fn authenticate(&self, metadata: &MetadataMap) -> Result<Principal, Status> {
        let header = metadata
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing authorization"))?;
        let key = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .ok_or_else(|| Status::unauthenticated("authorization is not a bearer token"))?;
        self.by_hash
            .get(&Self::hash(key.trim()))
            .cloned()
            .ok_or_else(|| Status::unauthenticated("unknown API key"))
    }
}

/// Everyone is the given principal: the loopback-only dev mode
/// (`insecure_dev`), never on a public listener.
#[derive(Debug, Clone)]
pub struct DevAllowAll(pub Principal);

impl Authenticator for DevAllowAll {
    fn authenticate(&self, _: &MetadataMap) -> Result<Principal, Status> {
        Ok(self.0.clone())
    }
}
