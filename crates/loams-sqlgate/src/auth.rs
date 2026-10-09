//! Client identity for the gate: `ResolveUser`, Argon2id verification and
//! the `caching_sha2_password` fast-auth cache (§47 §12; plan SQ1 Task 4).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Semaphore;

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::codec::auth::{Password, double_sha256, verify_caching_sha2};

/// Argon2id parameters (re-exported for configuration).
pub use argon2::Params as Argon2Params;

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
    hash_password_with(&Params::default(), password)
}

/// [`hash_password`] with explicit Argon2id parameters.
pub fn hash_password_with(params: &Params, password: &[u8]) -> String {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).unwrap_or_else(|e| panic!("OS randomness unavailable: {e}"));
    let hash: PasswordHash = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone())
        .hash_password_with_salt(password, &salt)
        .unwrap_or_else(|e| panic!("argon2: {e}"));
    hash.to_string()
}

/// Checks `password` against a PHC hash (its own parameters), on the
/// calling thread. A malformed hash is a mismatch.
fn verify_blocking(hash: &str, password: &Password) -> bool {
    PasswordHash::new(hash)
        .map(|h| {
            Argon2::default()
                .verify_password(password.expose(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

/// Checks `password` against a PHC hash, on the blocking pool (Argon2id is
/// deliberately slow), with no bound. The gate uses [`Verifier`].
pub async fn verify_password(hash: String, password: Password) -> bool {
    tokio::task::spawn_blocking(move || verify_blocking(&hash, &password))
        .await
        .unwrap_or(false)
}

/// All Argon2id verifications are busy: the client gets 1040.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("password verification is busy")]
pub struct Busy;

/// Bounded Argon2id verification (Task 4 fix round 1, C1): at most
/// `concurrency` checks run at once, a check waits at most `wait` for its
/// turn, and a permit is taken *before* the blocking job is queued, so a
/// client that gives up (the handshake deadline) leaves no queued work.
/// Unknown users are checked against a decoy hash made with the configured
/// parameters, so they cost and take what known users do.
#[derive(Debug)]
pub struct Verifier {
    permits: Arc<Semaphore>,
    wait: Duration,
    decoy: String,
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl Verifier {
    /// `concurrency` checks at once (at least 1), waiting at most `wait`.
    pub fn new(params: &Params, concurrency: usize, wait: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(concurrency.max(1))),
            wait,
            decoy: hash_password_with(params, b"loams-decoy-password"),
            in_flight: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The decoy hash for unknown users.
    pub fn decoy(&self) -> &str {
        &self.decoy
    }

    /// The most checks that ran at once.
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    /// Checks `password` against `hash`, or [`Busy`] when no permit came
    /// within the wait.
    pub async fn verify(&self, hash: String, password: Password) -> Result<bool, Busy> {
        let permit = tokio::time::timeout(self.wait, self.permits.clone().acquire_owned())
            .await
            .map_err(|_| Busy)?
            .map_err(|_| Busy)?;
        let (in_flight, peak) = (self.in_flight.clone(), self.peak.clone());
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            let ok = verify_blocking(&hash, &password);
            in_flight.fetch_sub(1, Ordering::SeqCst);
            ok
        })
        .await
        .map_err(|_| Busy)
    }
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
