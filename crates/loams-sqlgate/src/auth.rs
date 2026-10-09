//! Client identity for the gate: `ResolveUser`, Argon2id verification and
//! the `caching_sha2_password` fast-auth cache (§47 §12; plan SQ1 Task 4).

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::codec::auth::{Password, double_sha256, verify_caching_sha2};

/// A database role (plan SQ1 shared contracts). Each maps to the internal
/// TiDB user the gate logs in as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// `READER`.
    Reader,
    /// `WRITER`.
    Writer,
    /// `DDL`.
    Ddl,
    /// `ADMIN`.
    Admin,
}

impl Role {
    /// The internal TiDB user of the branch keyspace (`ri_<role>`).
    pub fn internal_user(self) -> &'static str {
        match self {
            Role::Reader => "ri_reader",
            Role::Writer => "ri_writer",
            Role::Ddl => "ri_ddl",
            Role::Admin => "ri_admin",
        }
    }
}

/// What `ResolveUser(user)` returns.
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedUser {
    /// The branch (and keyspace) the user belongs to.
    pub branch: String,
    /// Its role.
    pub role: Role,
    /// The Argon2id hash of the user's password, PHC string.
    pub password_hash: String,
}

impl fmt::Debug for ResolvedUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedUser")
            .field("branch", &self.branch)
            .field("role", &self.role)
            .field("password_hash", &"[redacted]")
            .finish()
    }
}

/// `ResolveUser` (`loams.internal.v1`, Task 7+): user → branch, role, hash.
#[async_trait]
pub trait UserResolver: Send + Sync {
    /// The user, or `None` when unknown.
    async fn resolve(&self, user: &str) -> Option<ResolvedUser>;
}

/// A fixed user table (tests and the desktop until Task 12 wires
/// `ResolveUser`).
#[derive(Debug, Default)]
pub struct StaticUsers(HashMap<String, ResolvedUser>);

impl StaticUsers {
    /// From `(user, resolved)` pairs.
    pub fn new(users: Vec<(String, ResolvedUser)>) -> Self {
        Self(users.into_iter().collect())
    }
}

#[async_trait]
impl UserResolver for StaticUsers {
    async fn resolve(&self, user: &str) -> Option<ResolvedUser> {
        self.0.get(user).cloned()
    }
}

/// An Argon2id PHC hash of `password` with a random salt (the control
/// plane's job in Task 11; tests use it too).
pub fn hash_password(password: &[u8]) -> String {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).unwrap_or_else(|e| panic!("OS randomness unavailable: {e}"));
    let hash: PasswordHash = Argon2::default()
        .hash_password_with_salt(password, &salt)
        .unwrap_or_else(|e| panic!("argon2: {e}"));
    hash.to_string()
}

/// Checks `password` against a PHC hash, on the blocking pool (Argon2id is
/// deliberately slow). A malformed hash is a mismatch.
pub async fn verify_password(hash: String, password: Password) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|h| {
                Argon2::default()
                    .verify_password(password.expose(), &h)
                    .is_ok()
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// The hash checked for unknown users, so they take as long as known ones.
pub(crate) fn decoy_hash() -> &'static str {
    static DECOY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DECOY.get_or_init(|| hash_password(b"loams-decoy-password"))
}

/// A user and the SHA-256 of their stored hash.
type CacheKey = (String, [u8; 32]);

/// The `caching_sha2_password` fast-auth cache: after a full Argon2id
/// check, `SHA256(SHA256(password))` per user, keyed by the user and the
/// SHA-256 of the stored hash, so a password rotation (a new hash) misses.
/// Bounded; a full cache is cleared (every user then re-does full auth).
#[derive(Debug)]
pub struct FastAuthCache {
    max: usize,
    entries: Mutex<HashMap<CacheKey, [u8; 32]>>,
}

impl FastAuthCache {
    /// A cache of at most `max` users.
    pub fn new(max: usize) -> Self {
        Self {
            max,
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn key(user: &str, hash: &str) -> CacheKey {
        (user.to_owned(), Sha256::digest(hash.as_bytes()).into())
    }

    /// Whether `scramble` proves the cached password for `user` and `hash`.
    pub fn check(&self, user: &str, hash: &str, nonce: &[u8], scramble: &[u8]) -> bool {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries
            .get(&Self::key(user, hash))
            .is_some_and(|cached| verify_caching_sha2(cached, nonce, scramble))
    }

    /// Remembers a password that passed the full check.
    pub fn remember(&self, user: &str, hash: &str, password: &Password) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.len() >= self.max {
            entries.clear();
        }
        entries.insert(Self::key(user, hash), double_sha256(password.expose()));
    }
}
