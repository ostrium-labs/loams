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
    /// The development users (FL2 Task 2's `UserMap`); Task 20 replaces them.
    pub users: Vec<UserMap>,
    /// Where `wait_end_of_query = 1` spools results past `default_buffer_size`.
    pub tmp_dir: PathBuf,
    /// The most a `wait_end_of_query = 1` response may spool: 1 GiB (HS1 Task 3).
    pub wait_end_of_query_max_bytes: u64,
    /// How much output is held before the response starts, unless `buffer_size`
    /// says otherwise: ClickHouse's 1 MiB. A statement that fails inside it still
    /// gets a proper error status.
    pub default_buffer_size: usize,
    /// The most bytes of a POST body read as statement text: ClickHouse's
    /// `max_query_size` default, 256 KiB. Data after an `INSERT … FORMAT` line is
    /// streamed, not counted.
    pub max_query_size: usize,
    /// The least time between two `X-ClickHouse-Progress` headers, unless
    /// `http_headers_progress_interval_ms` says otherwise: 100 ms.
    pub progress_interval: Duration,
    /// How long an idle keep-alive connection is kept: 10 s, as ClickHouse.
    pub keep_alive: Duration,
    /// The version rendered into errors.
    pub version: String,
}

impl Default for HouseConfig {
    fn default() -> Self {
        Self {
            listen: DEFAULT_LISTEN
                .parse()
                .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 8123))),
            users: vec![UserMap::dev("default", "", 0, false)],
            tmp_dir: std::env::temp_dir(),
            wait_end_of_query_max_bytes: 1024 * 1024 * 1024,
            default_buffer_size: 1024 * 1024,
            max_query_size: 256 * 1024,
            progress_interval: Duration::from_millis(100),
            keep_alive: Duration::from_secs(10),
            version: CLICKHOUSE_VERSION.to_string(),
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
