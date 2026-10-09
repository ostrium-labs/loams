//! The House front's configuration (HS1 Task 3; Task 7 reads it from TOML and the
//! command line).

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::errors::{ChError, HouseError};

/// Where the HTTP interface listens by default (§49 Shared contracts).
pub const DEFAULT_LISTEN: &str = "127.0.0.1:8123";

/// The ClickHouse version the House reports, which the pinned chdb-core v26.9.0
/// answers to `SELECT version()` (HS1 R1.1). It is rendered into every error.
/// `versions.rs` will own it with the reference server's digest (FL2 Global
/// Constraint); until then the worker's `Ready` is checked against it.
pub const CLICKHOUSE_VERSION: &str = "26.9.2.1";

/// `X-ClickHouse-Server-Display-Name` (FL2 Task 2).
pub const DISPLAY_NAME: &str = "loams-house";

/// The House front's configuration.
#[derive(Clone, Debug)]
pub struct HouseConfig {
    /// The HTTP listener: loopback only until TLS and the verifier (D111, HS1 Task 20).
    pub listen: SocketAddr,
    /// The development users (FL2 Task 2's `UserMap`); Task 20 replaces them. None
    /// by default (Task 3 review M9): a House with no users serves no queries.
    pub users: Vec<UserMap>,
    /// Where `wait_end_of_query = 1` spools results past [`SPOOL_IN_MEMORY`]: a data
    /// directory on disk, never the RAM-backed `/tmp` by default (review I2).
    pub tmp_dir: PathBuf,
    /// The most one `wait_end_of_query = 1` response may spool: 1 GiB.
    pub wait_end_of_query_max_bytes: u64,
    /// The most all spools together may hold at once (review I2): 8 GiB.
    pub spool_budget_bytes: u64,
    /// How much output is held before the response head goes out, unless
    /// `buffer_size` says otherwise (clamped to [`BUFFER_SIZE_RANGE`]): ClickHouse's
    /// 1 MiB. A statement that fails inside it still gets a proper error status.
    pub default_buffer_size: usize,
    /// The most bytes of a POST body read as statement text: ClickHouse's
    /// `max_query_size` default, 256 KiB. Data after an `INSERT … FORMAT` line is
    /// streamed, not counted.
    pub max_query_size: usize,
    /// How long a client may go silent while it sends a request body (per frame):
    /// ClickHouse's `http_receive_timeout`, 30 s. It never bounds a statement.
    pub receive_timeout: Duration,
    /// How long a write to the client may stall: ClickHouse's `http_send_timeout`.
    pub send_timeout: Duration,
    /// Keep-alive between requests: advertised as `Keep-Alive: timeout=…` and
    /// enforced (with the time a request head may take) by hyper's header timer, so
    /// the two never differ (fix round 2, N4); zero turns keep-alive off.
    pub keep_alive: Duration,
    /// Connections served at once; more wait in the listen backlog (review I1).
    pub max_connections: usize,
    /// Request header fields at most; more answer `431` (review M5).
    pub max_headers: usize,
    /// The largest request head hyper buffers: 128 KiB (fix round 2, N6), room for
    /// hyper's 65 534-byte URI limit and the headers.
    pub max_head_bytes: usize,
    /// The version rendered into errors.
    pub version: String,
    /// How far a compressed request body may expand (review I3).
    pub body_limits: crate::compress::BodyLimits,
}

/// The bytes a `wait_end_of_query` spool keeps in memory before it moves to a file
/// (review I2: a constant, not the client's `buffer_size`).
pub const SPOOL_IN_MEMORY: usize = 1024 * 1024;

/// `buffer_size` is clamped to this (review I2).
pub const BUFFER_SIZE_RANGE: std::ops::RangeInclusive<usize> = 1..=16 * 1024 * 1024;

impl Default for HouseConfig {
    fn default() -> Self {
        Self {
            listen: DEFAULT_LISTEN
                .parse()
                .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 8123))),
            users: Vec::new(),
            tmp_dir: PathBuf::from("loams-house-data").join("spool"),
            wait_end_of_query_max_bytes: 1024 * 1024 * 1024,
            spool_budget_bytes: 8 * 1024 * 1024 * 1024,
            default_buffer_size: 1024 * 1024,
            max_query_size: 256 * 1024,
            receive_timeout: Duration::from_secs(30),
            send_timeout: Duration::from_secs(30),
            keep_alive: Duration::from_secs(10),
            max_connections: 1024,
            max_headers: 100,
            max_head_bytes: 128 * 1024,
            version: CLICKHOUSE_VERSION.to_string(),
            body_limits: crate::compress::BodyLimits::default(),
        }
    }
}

/// Refuses a listener that is not loopback, with FL2's message (D111). Plaintext
/// listeners stay on loopback until HS1 Task 20 brings TLS and the verifier.
pub fn check_listen(addr: SocketAddr) -> Result<(), HouseError> {
    if addr.ip().is_loopback() {
        return Ok(());
    }
    Err(HouseError::from(ChError::bad_arguments(format!(
        "house listen on {addr}: only loopback addresses are served until the unified auth \
         plan (D111)"
    ))))
}

/// A development user (FL2 Task 2): a name, the SHA-256 of its password, the
/// namespace its queries run in, and whether it may only read. Task 20 replaces
/// these with Loams identities.
#[derive(Clone, PartialEq, Eq)]
pub struct UserMap {
    /// The ClickHouse user name.
    pub user: String,
    /// Lower-case hex SHA-256 of the password.
    pub password_sha256: String,
    /// The namespace the user's queries are bound to.
    pub namespace: u64,
    /// Whether the user may only read (as GET does for everyone).
    pub readonly: bool,
}

impl UserMap {
    /// A user from a clear-text password, hashed here.
    pub fn dev(user: &str, password: &str, namespace: u64, readonly: bool) -> Self {
        Self {
            user: user.to_string(),
            password_sha256: sha256_hex(password),
            namespace,
            readonly,
        }
    }

    /// Whether `password` is this user's, compared in constant time over the digest.
    pub fn verifies(&self, password: &str) -> bool {
        let given = sha256_hex(password);
        let ours = self.password_sha256.to_ascii_lowercase();
        given.len() == ours.len()
            && given
                .bytes()
                .zip(ours.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    }
}

impl fmt::Debug for UserMap {
    /// The digest is a credential: never printed (HS1 Global Constraint).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserMap")
            .field("user", &self.user)
            .field("password_sha256", &"[redacted]")
            .field("namespace", &self.namespace)
            .field("readonly", &self.readonly)
            .finish()
    }
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_only() {
        for ok in ["127.0.0.1:8123", "127.0.0.7:0", "[::1]:8123"] {
            assert!(check_listen(ok.parse().expect("addr")).is_ok(), "{ok}");
        }
        for bad in ["0.0.0.0:8123", "[::]:8123", "10.0.0.1:8123"] {
            let err = check_listen(bad.parse().expect("addr")).expect_err(bad);
            assert_eq!(
                err.message(),
                format!(
                    "house listen on {bad}: only loopback addresses are served until the \
                     unified auth plan (D111)"
                )
            );
        }
    }

    #[test]
    fn defaults_have_no_users_and_no_tmpfs_spool() {
        let config = HouseConfig::default();
        assert!(config.users.is_empty(), "review M9");
        assert!(!config.tmp_dir.starts_with("/tmp"), "review I2");
    }

    #[test]
    fn config_debug_redacts_credentials() {
        let config = HouseConfig {
            users: vec![UserMap::dev("alice", "secret", 1, false)],
            ..HouseConfig::default()
        };
        let digest = config.users[0].password_sha256.clone();
        let shown = format!("{config:?}");
        assert!(
            !shown.contains(&digest) && !shown.contains("secret"),
            "{shown}"
        );
    }

    #[test]
    fn passwords_verify_and_never_print() {
        let user = UserMap::dev("alice", "secret", 2, false);
        assert!(user.verifies("secret"));
        assert!(!user.verifies("Secret"));
        assert!(!user.verifies(""));
        let shown = format!("{user:?}");
        assert!(!shown.contains(&user.password_sha256), "{shown}");
        assert!(shown.contains("[redacted]"));
    }
}
