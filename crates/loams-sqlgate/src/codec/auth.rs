//! Authentication exchanges: `caching_sha2_password` (fast and full),
//! `AuthSwitchRequest` and `AuthMoreData`.
//!
//! The gate authenticates every client with `caching_sha2_password`:
//! - **fast:** the client's scramble is checked against the cached
//!   `SHA256(SHA256(password))` ([`double_sha256`]);
//! - **full:** on a cache miss, over TLS only, the client sends its password
//!   in clear; the caller checks it against the stored Argon2id hash and
//!   refills the cache. The RSA public-key exchange is never offered.
//!
//! A client that starts with another plugin is switched. Verdicts are the
//! caller's ([`Action::CheckFast`], [`Action::CheckFull`]); this module has
//! no clock, store or randomness.

use std::fmt;

use sha2::{Digest, Sha256};

use super::handshake::Nonce;
use super::{DecodeError, Reader, invalid, utf8};

/// `caching_sha2_password`.
pub const CACHING_SHA2: &str = "caching_sha2_password";
/// `mysql_native_password`.
pub const NATIVE: &str = "mysql_native_password";
/// `mysql_clear_password` (TiDB's `tidb_auth_token` uses it, R2.12).
pub const CLEAR: &str = "mysql_clear_password";

/// The longest cleartext password (or token) accepted in full auth.
pub const MAX_CLEAR_PASSWORD: usize = 1024;

/// A password or token. Prints `[redacted]`; zeroed on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct Password(Vec<u8>);

impl Password {
    /// Wraps secret bytes.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// The secret bytes.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        self.0.fill(0);
        // Keeps the zeroing from being optimised away.
        std::hint::black_box(&self.0);
    }
}

/// `SHA256(SHA256(password))`: what the fast-auth cache holds.
pub fn double_sha256(password: &[u8]) -> [u8; 32] {
    Sha256::digest(Sha256::digest(password)).into()
}

/// The client's `caching_sha2_password` scramble:
/// `SHA256(pw) XOR SHA256(SHA256(SHA256(pw)) || nonce)`. Empty for an empty
/// password.
pub fn scramble_caching_sha2(password: &[u8], nonce: &[u8]) -> Vec<u8> {
    if password.is_empty() {
        return Vec::new();
    }
    let h1: [u8; 32] = Sha256::digest(password).into();
    let h2: [u8; 32] = Sha256::digest(h1).into();
    let h3: [u8; 32] = Sha256::new()
        .chain_update(h2)
        .chain_update(nonce)
        .finalize()
        .into();
    h1.iter().zip(h3).map(|(a, b)| a ^ b).collect()
}

/// Checks a scramble against the cached `SHA256(SHA256(pw))`, in constant
/// time for a 32-byte scramble.
pub fn verify_caching_sha2(cached: &[u8; 32], nonce: &[u8], scramble: &[u8]) -> bool {
    if scramble.len() != 32 {
        return false;
    }
    let h3: [u8; 32] = Sha256::new()
        .chain_update(cached)
        .chain_update(nonce)
        .finalize()
        .into();
    let mut h1 = [0u8; 32];
    for (i, b) in h1.iter_mut().enumerate() {
        *b = scramble[i] ^ h3[i];
    }
    let candidate: [u8; 32] = Sha256::digest(h1).into();
    candidate
        .iter()
        .zip(cached)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

/// The gate's upstream login (Task 4): the auth response for `plugin`,
/// as a [`Password`] (redacted, zeroed on drop).
/// - `caching_sha2_password` scrambles.
/// - `mysql_clear_password` sends the secret NUL-terminated, over TLS only
///   (the caller's duty). `tidb_auth_token` users (R2.12) get it this way:
///   TiDB answers the `HandshakeResponse41` with an `AuthSwitchRequest` to
///   `mysql_clear_password`, and the JWT travels in the auth-switch
///   response, a raw packet with no 255-byte limit. TiDB v8.5.8 does not
///   offer `CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA`, so a JWT never fits the
///   `HandshakeResponse41` itself.
pub fn client_auth_response(
    plugin: &str,
    password: &Password,
    nonce: &[u8],
) -> Result<Password, AuthError> {
    match plugin {
        CACHING_SHA2 => Ok(Password::new(scramble_caching_sha2(
            password.expose(),
            nonce,
        ))),
        CLEAR => {
            let mut v = Vec::with_capacity(password.expose().len() + 1);
            v.extend_from_slice(password.expose());
            v.push(0);
            Ok(Password::new(v))
        }
        _ => Err(AuthError::UnsupportedPlugin),
    }
}

/// `AuthSwitchRequest` (`0xFE`, plugin name, plugin data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSwitchRequest {
    /// The plugin to switch to.
    pub plugin: String,
    /// Its data: for the SHA-2 and native plugins, the nonce and a NUL.
    pub data: Vec<u8>,
}

impl AuthSwitchRequest {
    /// The payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 + self.plugin.len() + self.data.len());
        out.push(0xfe);
        out.extend_from_slice(self.plugin.as_bytes());
        out.push(0);
        out.extend_from_slice(&self.data);
        out
    }

    /// Decodes a payload (plugin name at most 64 bytes, data at most 1 KiB).
    pub fn decode(payload: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        if r.u8("auth switch")? != 0xfe {
            return Err(invalid("auth switch", "not 0xFE"));
        }
        let plugin = utf8(r.nul_terminated(64, "auth plugin")?, "auth plugin")?;
        let data = r.rest();
        if data.len() > 1024 {
            return Err(DecodeError::TooLong {
                what: "auth switch data",
                limit: 1024,
            });
        }
        Ok(Self {
            plugin,
            data: data.to_vec(),
        })
    }

    /// The 20-byte nonce, when the data is a nonce and a NUL.
    pub fn nonce(&self) -> Option<&[u8; 20]> {
        match self.data.as_slice() {
            [n @ .., 0] => n.try_into().ok(),
            _ => None,
        }
    }
}

/// `AuthMoreData` from a `caching_sha2_password` server (`0x01` + status).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMoreData {
    /// `0x03`: the scramble matched the cache; an OK follows.
    FastAuthSuccess,
    /// `0x04`: send the password (over TLS) or ask for the RSA key.
    PerformFullAuthentication,
}

impl AuthMoreData {
    /// The payload.
    pub fn encode(self) -> Vec<u8> {
        vec![
            0x01,
            if self == Self::FastAuthSuccess {
                0x03
            } else {
                0x04
            },
        ]
    }

    /// Decodes the two-byte payload.
    pub fn decode(payload: &[u8]) -> Result<Self, DecodeError> {
        match payload {
            [0x01, 0x03] => Ok(Self::FastAuthSuccess),
            [0x01, 0x04] => Ok(Self::PerformFullAuthentication),
            [0x01, _] => Err(invalid("auth more data", "unknown status")),
            _ => Err(invalid("auth more data", "not 0x01 + status")),
        }
    }
}

/// Why authentication stopped. The gate answers each with an ERR packet
/// (Task 4: 1045, or 3159 for [`AuthError::SecureTransportRequired`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// Full authentication needs TLS.
    #[error("full authentication requires a secure connection")]
    SecureTransportRequired,
    /// The client asked for the RSA public key; the gate never serves it.
    #[error("public key retrieval is not supported")]
    PublicKeyRefused,
    /// A scramble or password of the wrong shape.
    #[error("malformed authentication data")]
    Malformed,
    /// A packet when none was expected.
    #[error("unexpected authentication packet")]
    Unexpected,
    /// A plugin the gate does not speak.
    #[error("unsupported authentication plugin")]
    UnsupportedPlugin,
    /// Denied without a lookup (an empty password; the gate issues none).
    #[error("access denied")]
    AccessDenied,
}

impl AuthError {
    /// The MySQL error code the gate answers with: 3159 when TLS is
    /// required, 1045 otherwise.
    pub fn error_code(self) -> u16 {
        match self {
            AuthError::SecureTransportRequired => 3159,
            _ => 1045,
        }
    }
}

/// What the caller does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send this payload (next sequence id) and wait for the client.
    Send(Vec<u8>),
    /// Look the user up in the fast-auth cache, check with
    /// [`verify_caching_sha2`], and report with
    /// [`CachingSha2Server::fast_result`].
    CheckFast {
        /// The client's 32-byte scramble.
        scramble: Vec<u8>,
    },
    /// Check the password against the stored hash; send OK or ERR.
    CheckFull {
        /// The cleartext password.
        password: Password,
    },
    /// Send the error for this and close.
    Fail(AuthError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Start,
    AwaitSwitchResponse,
    AwaitFastResult,
    AwaitClearPassword,
    Done,
}

/// The server side of `caching_sha2_password`, for one connection.
#[derive(Debug)]
pub struct CachingSha2Server {
    nonce: Nonce,
    tls: bool,
    state: State,
}

impl CachingSha2Server {
    /// `nonce` is the one sent in the greeting; `tls` whether the
    /// connection is encrypted.
    pub fn new(nonce: Nonce, tls: bool) -> Self {
        Self {
            nonce,
            tls,
            state: State::Start,
        }
    }

    /// Starts from the client's `HandshakeResponse41` plugin and auth data.
    pub fn start(&mut self, plugin: Option<&str>, auth_response: &[u8]) -> Action {
        if self.state != State::Start {
            return Action::Fail(AuthError::Unexpected);
        }
        if plugin != Some(CACHING_SHA2) {
            self.state = State::AwaitSwitchResponse;
            let mut data = self.nonce.as_bytes().to_vec();
            data.push(0);
            return Action::Send(
                AuthSwitchRequest {
                    plugin: CACHING_SHA2.into(),
                    data,
                }
                .encode(),
            );
        }
        self.scramble(auth_response)
    }

    /// Handles the client's next packet.
    pub fn on_packet(&mut self, payload: &[u8]) -> Result<Action, AuthError> {
        match self.state {
            State::AwaitSwitchResponse => {
                self.state = State::Start;
                Ok(self.scramble(payload))
            }
            State::AwaitClearPassword => {
                if payload == [0x02] {
                    self.state = State::Done;
                    return Err(AuthError::PublicKeyRefused);
                }
                let password = match payload {
                    [p @ .., 0] if p.len() <= MAX_CLEAR_PASSWORD && !p.contains(&0) => p,
                    _ => {
                        self.state = State::Done;
                        return Err(AuthError::Malformed);
                    }
                };
                self.state = State::Done;
                Ok(Action::CheckFull {
                    password: Password::new(password.to_vec()),
                })
            }
            State::Start | State::AwaitFastResult | State::Done => Err(AuthError::Unexpected),
        }
    }

    /// Reports the fast-auth check: a hit succeeds, a miss asks for the
    /// password over TLS or fails without it.
    pub fn fast_result(&mut self, hit: bool) -> Action {
        if self.state != State::AwaitFastResult {
            return Action::Fail(AuthError::Unexpected);
        }
        if hit {
            self.state = State::Done;
            return Action::Send(AuthMoreData::FastAuthSuccess.encode());
        }
        if !self.tls {
            self.state = State::Done;
            return Action::Fail(AuthError::SecureTransportRequired);
        }
        self.state = State::AwaitClearPassword;
        Action::Send(AuthMoreData::PerformFullAuthentication.encode())
    }

    /// The exchange has reached a verdict (or a failure).
    pub fn is_done(&self) -> bool {
        self.state == State::Done
    }

    fn scramble(&mut self, data: &[u8]) -> Action {
        match data {
            // An empty password (no data, or a lone 0x00 as some clients
            // send it): the gate never issues one, so it is denied.
            [] | [0] => {
                self.state = State::Done;
                Action::Fail(AuthError::AccessDenied)
            }
            _ if data.len() == 32 => {
                self.state = State::AwaitFastResult;
                Action::CheckFast {
                    scramble: data.to_vec(),
                }
            }
            _ => {
                self.state = State::Done;
                Action::Fail(AuthError::Malformed)
            }
        }
    }
}
