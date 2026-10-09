//! The connection phase: the server's `HandshakeV10`, the client's
//! `SSLRequest` and `HandshakeResponse41`, and capability negotiation.
//!
//! The gate speaks to clients as a server and to TiDB as a client, so each
//! message has both an encoder and a bounded decoder.

use std::fmt;
use std::ops::BitOr;

use super::{DecodeError, Reader, invalid, put_lenenc, put_lenenc_bytes, utf8};

/// Capability flags (`CLIENT_*`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Capabilities(pub u32);

impl Capabilities {
    /// `CLIENT_LONG_PASSWORD`.
    pub const LONG_PASSWORD: Self = Self(1);
    /// `CLIENT_FOUND_ROWS`.
    pub const FOUND_ROWS: Self = Self(1 << 1);
    /// `CLIENT_LONG_FLAG`.
    pub const LONG_FLAG: Self = Self(1 << 2);
    /// `CLIENT_CONNECT_WITH_DB`.
    pub const CONNECT_WITH_DB: Self = Self(1 << 3);
    /// `CLIENT_NO_SCHEMA`.
    pub const NO_SCHEMA: Self = Self(1 << 4);
    /// `CLIENT_COMPRESS`.
    pub const COMPRESS: Self = Self(1 << 5);
    /// `CLIENT_ODBC`.
    pub const ODBC: Self = Self(1 << 6);
    /// `CLIENT_LOCAL_FILES`.
    pub const LOCAL_FILES: Self = Self(1 << 7);
    /// `CLIENT_IGNORE_SPACE`.
    pub const IGNORE_SPACE: Self = Self(1 << 8);
    /// `CLIENT_PROTOCOL_41`.
    pub const PROTOCOL_41: Self = Self(1 << 9);
    /// `CLIENT_INTERACTIVE`.
    pub const INTERACTIVE: Self = Self(1 << 10);
    /// `CLIENT_SSL`.
    pub const SSL: Self = Self(1 << 11);
    /// `CLIENT_IGNORE_SIGPIPE`.
    pub const IGNORE_SIGPIPE: Self = Self(1 << 12);
    /// `CLIENT_TRANSACTIONS`.
    pub const TRANSACTIONS: Self = Self(1 << 13);
    /// `CLIENT_RESERVED`.
    pub const RESERVED: Self = Self(1 << 14);
    /// `CLIENT_SECURE_CONNECTION` (`CLIENT_RESERVED2`).
    pub const SECURE_CONNECTION: Self = Self(1 << 15);
    /// `CLIENT_MULTI_STATEMENTS`.
    pub const MULTI_STATEMENTS: Self = Self(1 << 16);
    /// `CLIENT_MULTI_RESULTS`.
    pub const MULTI_RESULTS: Self = Self(1 << 17);
    /// `CLIENT_PS_MULTI_RESULTS`.
    pub const PS_MULTI_RESULTS: Self = Self(1 << 18);
    /// `CLIENT_PLUGIN_AUTH`.
    pub const PLUGIN_AUTH: Self = Self(1 << 19);
    /// `CLIENT_CONNECT_ATTRS`.
    pub const CONNECT_ATTRS: Self = Self(1 << 20);
    /// `CLIENT_PLUGIN_AUTH_LENENC_CLIENT_DATA`.
    pub const PLUGIN_AUTH_LENENC_CLIENT_DATA: Self = Self(1 << 21);
    /// `CLIENT_CAN_HANDLE_EXPIRED_PASSWORDS`.
    pub const CAN_HANDLE_EXPIRED_PASSWORDS: Self = Self(1 << 22);
    /// `CLIENT_SESSION_TRACK`.
    pub const SESSION_TRACK: Self = Self(1 << 23);
    /// `CLIENT_DEPRECATE_EOF`.
    pub const DEPRECATE_EOF: Self = Self(1 << 24);
    /// `CLIENT_OPTIONAL_RESULTSET_METADATA`.
    pub const OPTIONAL_RESULTSET_METADATA: Self = Self(1 << 25);
    /// `CLIENT_ZSTD_COMPRESSION_ALGORITHM`.
    pub const ZSTD_COMPRESSION: Self = Self(1 << 26);
    /// `CLIENT_QUERY_ATTRIBUTES`.
    pub const QUERY_ATTRIBUTES: Self = Self(1 << 27);
    /// `MULTI_FACTOR_AUTHENTICATION`.
    pub const MULTI_FACTOR_AUTH: Self = Self(1 << 28);
    /// `CLIENT_CAPABILITY_EXTENSION`.
    pub const CAPABILITY_EXTENSION: Self = Self(1 << 29);
    /// `CLIENT_SSL_VERIFY_SERVER_CERT`.
    pub const SSL_VERIFY_SERVER_CERT: Self = Self(1 << 30);
    /// `CLIENT_REMEMBER_OPTIONS`.
    pub const REMEMBER_OPTIONS: Self = Self(1 << 31);

    /// Every flag of `other` is set.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Flags set in both.
    #[must_use]
    pub fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// `self` without the flags of `other`.
    #[must_use]
    pub fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// No flag outside `other`.
    pub fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }
}

impl BitOr for Capabilities {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl fmt::Debug for Capabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Capabilities({:#010x})", self.0)
    }
}

/// What the gate implements. Not offered, whatever TiDB supports:
/// `LOAD DATA LOCAL` (`CLIENT_LOCAL_FILES`: it lets the server read client
/// files), compression (the gate relays uncompressed frames), multi-factor auth,
/// optional result-set metadata and query attributes (they change packet
/// layouts the gate does not track), and the client-only and
/// extension flags.
pub const GATE_SUPPORTED: Capabilities = Capabilities(
    Capabilities::LONG_PASSWORD.0
        | Capabilities::FOUND_ROWS.0
        | Capabilities::LONG_FLAG.0
        | Capabilities::CONNECT_WITH_DB.0
        | Capabilities::NO_SCHEMA.0
        | Capabilities::ODBC.0
        | Capabilities::IGNORE_SPACE.0
        | Capabilities::PROTOCOL_41.0
        | Capabilities::INTERACTIVE.0
        | Capabilities::SSL.0
        | Capabilities::IGNORE_SIGPIPE.0
        | Capabilities::TRANSACTIONS.0
        | Capabilities::RESERVED.0
        | Capabilities::SECURE_CONNECTION.0
        | Capabilities::MULTI_STATEMENTS.0
        | Capabilities::MULTI_RESULTS.0
        | Capabilities::PS_MULTI_RESULTS.0
        | Capabilities::PLUGIN_AUTH.0
        | Capabilities::CONNECT_ATTRS.0
        | Capabilities::PLUGIN_AUTH_LENENC_CLIENT_DATA.0
        | Capabilities::CAN_HANDLE_EXPIRED_PASSWORDS.0
        | Capabilities::SESSION_TRACK.0
        | Capabilities::DEPRECATE_EOF.0,
);

/// Flags that change how TiDB frames results or counts rows: they must be
/// the same on the client ↔ gate and gate ↔ TiDB legs for a byte relay.
pub const RELAY_SENSITIVE: Capabilities = Capabilities(
    Capabilities::FOUND_ROWS.0
        | Capabilities::LONG_FLAG.0
        | Capabilities::NO_SCHEMA.0
        | Capabilities::IGNORE_SPACE.0
        | Capabilities::INTERACTIVE.0
        | Capabilities::TRANSACTIONS.0
        | Capabilities::MULTI_STATEMENTS.0
        | Capabilities::MULTI_RESULTS.0
        | Capabilities::PS_MULTI_RESULTS.0
        | Capabilities::SESSION_TRACK.0
        | Capabilities::DEPRECATE_EOF.0,
);

/// Flags every client must agree to.
const REQUIRED: Capabilities = Capabilities(
    Capabilities::PROTOCOL_41.0 | Capabilities::SECURE_CONNECTION.0 | Capabilities::PLUGIN_AUTH.0,
);

/// The capabilities of the pinned TiDB v8.5.8, as a static profile: the
/// gate greets a client before it knows the branch (wake on connect), so it
/// never waits for a live TiDB greeting. The value is the v8.5.8 greeting
/// captured in `tests/fixtures/clients` (`0x051ba6af`) plus `CLIENT_SSL`,
/// which TiDB adds when TLS is configured (always, for Loams pools). A TiDB
/// upgrade re-captures it (`ssl_is_offered_on_the_gates_terms`).
pub const TIDB_V8_5_8: Capabilities = Capabilities(0x051b_a6af | Capabilities::SSL.0);

/// What the gate offers clients: what the TiDB `profile` (normally
/// [`TIDB_V8_5_8`]) supports and the gate implements, plus `CLIENT_SSL` on
/// the gate's own terms (it terminates client TLS itself).
pub fn advertise(profile: Capabilities) -> Capabilities {
    profile.intersect(GATE_SUPPORTED) | Capabilities::SSL
}

/// A client the gate cannot serve.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("client lacks required capabilities {missing:?}")]
pub struct NegotiationError {
    /// The required flags the client (or the offer) lacks.
    pub missing: Capabilities,
}

/// The capabilities in force: the client's, limited to what was offered.
/// Refuses a client without 4.1 protocol, secure connection or plugin auth.
pub fn negotiate(
    client: Capabilities,
    offered: Capabilities,
) -> Result<Capabilities, NegotiationError> {
    let agreed = client.intersect(offered);
    if !agreed.contains(REQUIRED) {
        return Err(NegotiationError {
            missing: REQUIRED.without(agreed),
        });
    }
    Ok(agreed)
}

/// The gate's own flags on its connection to TiDB: 4.1, plugin auth (with
/// length-encoded data where TiDB has it), TLS, attributes and the database.
pub const GATE_OWN_UPSTREAM: Capabilities = Capabilities(
    REQUIRED.0
        | Capabilities::SSL.0
        | Capabilities::LONG_PASSWORD.0
        | Capabilities::CONNECT_WITH_DB.0
        | Capabilities::PLUGIN_AUTH_LENENC_CLIENT_DATA.0
        | Capabilities::CONNECT_ATTRS.0,
);

/// The capabilities the gate sends TiDB for a client that agreed to
/// `agreed`: the client's relay-sensitive flags (so result framing matches
/// on both legs) plus [`GATE_OWN_UPSTREAM`], limited to TiDB's `profile`.
/// Nothing else of the client's reaches TiDB; a plaintext loopback client
/// still gets TLS upstream.
pub fn upstream_capabilities(agreed: Capabilities, profile: Capabilities) -> Capabilities {
    (agreed.intersect(RELAY_SENSITIVE) | GATE_OWN_UPSTREAM).intersect(profile)
}

/// A 20-byte authentication nonce, every byte in `1..=127` (it is sent
/// NUL-terminated).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Nonce([u8; 20]);

impl Nonce {
    /// Maps 20 random bytes (the caller's CSPRNG) into the nonce alphabet.
    pub fn from_random(random: [u8; 20]) -> Self {
        Self(random.map(|b| b % 127 + 1))
    }

    /// The 20 bytes.
    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    fn from_wire(b: &[u8]) -> Result<Self, DecodeError> {
        let a: [u8; 20] = b.try_into().map_err(|_| invalid("nonce", "not 20 bytes"))?;
        if a.contains(&0) {
            return Err(invalid("nonce", "contains NUL"));
        }
        Ok(Self(a))
    }
}

impl fmt::Debug for Nonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Nonce(..)")
    }
}

/// The server greeting (protocol version 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeV10 {
    /// Advertised version, e.g. `8.0.11-TiDB-v8.5.8-Loams`.
    pub server_version: String,
    /// The connection id.
    pub connection_id: u32,
    /// The authentication nonce.
    pub nonce: Nonce,
    /// Server capabilities.
    pub capabilities: Capabilities,
    /// Default collation id.
    pub charset: u8,
    /// Server status flags.
    pub status: u16,
    /// Default authentication plugin.
    pub auth_plugin: String,
}

const MAX_VERSION: usize = 255;
const MAX_PLUGIN: usize = 64;

impl HandshakeV10 {
    /// Encodes the greeting payload, with `CLIENT_PLUGIN_AUTH` and
    /// `CLIENT_SECURE_CONNECTION` layout.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(96);
        out.push(10);
        out.extend_from_slice(self.server_version.as_bytes());
        out.push(0);
        out.extend_from_slice(&self.connection_id.to_le_bytes());
        out.extend_from_slice(&self.nonce.0[..8]);
        out.push(0);
        let caps = self.capabilities.0.to_le_bytes();
        out.extend_from_slice(&caps[..2]);
        out.push(self.charset);
        out.extend_from_slice(&self.status.to_le_bytes());
        out.extend_from_slice(&caps[2..]);
        out.push(21);
        out.extend_from_slice(&[0; 10]);
        out.extend_from_slice(&self.nonce.0[8..]);
        out.push(0);
        out.extend_from_slice(self.auth_plugin.as_bytes());
        out.push(0);
        out
    }

    /// Decodes TiDB's greeting. Requires protocol 10, 4.1, secure
    /// connection and plugin auth, a 20-byte nonce and no trailing bytes.
    pub fn decode(payload: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        if r.u8("protocol version")? != 10 {
            return Err(invalid("protocol version", "not 10"));
        }
        let server_version = utf8(
            r.nul_terminated(MAX_VERSION, "server version")?,
            "server version",
        )?;
        let connection_id = r.u32("connection id")?;
        let part1 = r.bytes(8, "nonce")?;
        if r.u8("filler")? != 0 {
            return Err(invalid("filler", "not 0"));
        }
        let low = r.u16("capabilities")?;
        let charset = r.u8("charset")?;
        let status = r.u16("status")?;
        let high = r.u16("capabilities")?;
        let capabilities = Capabilities(u32::from(low) | u32::from(high) << 16);
        if !capabilities.contains(REQUIRED) {
            return Err(invalid(
                "capabilities",
                "server lacks 4.1, secure connection or plugin auth",
            ));
        }
        if r.u8("auth data length")? != 21 {
            return Err(invalid("auth data length", "not 21"));
        }
        r.bytes(10, "reserved")?;
        let part2 = r.bytes(12, "nonce")?;
        if r.u8("nonce")? != 0 {
            return Err(invalid("nonce", "not NUL-terminated"));
        }
        let nonce = Nonce::from_wire(&[part1, part2].concat())?;
        let auth_plugin = utf8(r.nul_terminated(MAX_PLUGIN, "auth plugin")?, "auth plugin")?;
        if !r.is_empty() {
            return Err(invalid("greeting", "trailing bytes"));
        }
        Ok(Self {
            server_version,
            connection_id,
            nonce,
            capabilities,
            charset,
            status,
            auth_plugin,
        })
    }
}

/// Bounds on what a client may send in its `HandshakeResponse41`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    /// User name bytes (MySQL allows 32 characters).
    pub max_username: usize,
    /// Authentication response bytes (a token in the password field fits).
    pub max_auth_response: usize,
    /// Database name bytes (64 characters).
    pub max_database: usize,
    /// Plugin name bytes.
    pub max_plugin: usize,
    /// Total connection-attribute bytes (MySQL allows 64 KiB).
    pub max_attributes_bytes: usize,
    /// Connection-attribute pairs.
    pub max_attributes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_username: 128,
            max_auth_response: 4096,
            max_database: 256,
            max_plugin: MAX_PLUGIN,
            max_attributes_bytes: 65_536,
            max_attributes: 128,
        }
    }
}

/// The client's `SSLRequest`: the first 32 bytes of a response, sent in
/// plaintext before TLS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SslRequest {
    /// Client capabilities (with `CLIENT_SSL`).
    pub capabilities: Capabilities,
    /// The client's maximum packet size.
    pub max_packet: u32,
    /// Collation id.
    pub charset: u8,
}

impl SslRequest {
    /// The 32-byte payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        put_fixed_header(&mut out, self.capabilities, self.max_packet, self.charset);
        out
    }
}

/// The client's `HandshakeResponse41`.
#[derive(Clone, PartialEq, Eq)]
pub struct HandshakeResponse41 {
    /// Client capabilities, as sent (see [`negotiate`]).
    pub capabilities: Capabilities,
    /// The client's maximum packet size.
    pub max_packet: u32,
    /// Collation id.
    pub charset: u8,
    /// The user name.
    pub username: String,
    /// The authentication response (a scramble, or a secret: never logged).
    pub auth_response: Vec<u8>,
    /// The database, with `CLIENT_CONNECT_WITH_DB`.
    pub database: Option<String>,
    /// The client's plugin, with `CLIENT_PLUGIN_AUTH`.
    pub auth_plugin: Option<String>,
    /// Connection attributes, in order, with `CLIENT_CONNECT_ATTRS`.
    pub attributes: Vec<(String, String)>,
    /// The zstd level, with `CLIENT_ZSTD_COMPRESSION_ALGORITHM`.
    pub zstd_level: Option<u8>,
}

impl fmt::Debug for HandshakeResponse41 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HandshakeResponse41")
            .field("capabilities", &self.capabilities)
            .field("max_packet", &self.max_packet)
            .field("charset", &self.charset)
            .field("username", &self.username)
            .field("auth_response", &"[redacted]")
            .field("database", &self.database)
            .field("auth_plugin", &self.auth_plugin)
            .field("attributes", &self.attributes)
            .field("zstd_level", &self.zstd_level)
            .finish()
    }
}

impl HandshakeResponse41 {
    /// Encodes the payload, laid out by `self.capabilities`.
    pub fn encode(&self) -> Vec<u8> {
        let caps = self.capabilities;
        let mut out = Vec::with_capacity(128);
        put_fixed_header(&mut out, caps, self.max_packet, self.charset);
        out.extend_from_slice(self.username.as_bytes());
        out.push(0);
        if caps.contains(Capabilities::PLUGIN_AUTH_LENENC_CLIENT_DATA) {
            put_lenenc_bytes(&mut out, &self.auth_response);
        } else if caps.contains(Capabilities::SECURE_CONNECTION) {
            out.push(u8::try_from(self.auth_response.len()).unwrap_or(u8::MAX));
            out.extend_from_slice(&self.auth_response[..self.auth_response.len().min(255)]);
        } else {
            out.extend_from_slice(&self.auth_response);
            out.push(0);
        }
        if caps.contains(Capabilities::CONNECT_WITH_DB) {
            out.extend_from_slice(self.database.as_deref().unwrap_or_default().as_bytes());
            out.push(0);
        }
        if caps.contains(Capabilities::PLUGIN_AUTH) {
            out.extend_from_slice(self.auth_plugin.as_deref().unwrap_or_default().as_bytes());
            out.push(0);
        }
        if caps.contains(Capabilities::CONNECT_ATTRS) {
            let mut attrs = Vec::new();
            for (k, v) in &self.attributes {
                put_lenenc_bytes(&mut attrs, k.as_bytes());
                put_lenenc_bytes(&mut attrs, v.as_bytes());
            }
            put_lenenc(&mut out, attrs.len() as u64);
            out.extend_from_slice(&attrs);
        }
        if caps.contains(Capabilities::ZSTD_COMPRESSION) {
            out.push(self.zstd_level.unwrap_or(3));
        }
        out
    }
}

/// The client's first packet: an `SSLRequest` or a full response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientHello {
    /// Upgrade to TLS, then the client sends a [`HandshakeResponse41`].
    Ssl(SslRequest),
    /// The response.
    Response(HandshakeResponse41),
}

/// Decodes the client's answer to the greeting. A 32-byte payload with
/// `CLIENT_SSL` is an `SSLRequest`; anything else must be a complete
/// `HandshakeResponse41` within `limits`, with nothing after it.
pub fn decode_client_hello(payload: &[u8], limits: &Limits) -> Result<ClientHello, DecodeError> {
    let mut r = Reader::new(payload);
    let capabilities = Capabilities(r.u32("capabilities")?);
    if !capabilities.contains(Capabilities::PROTOCOL_41) {
        return Err(invalid("capabilities", "pre-4.1 client"));
    }
    let max_packet = r.u32("max packet")?;
    let charset = r.u8("charset")?;
    // MariaDB keeps extended capabilities in the last 4 filler bytes; they
    // are not used, so the filler is skipped unchecked.
    r.bytes(23, "filler")?;
    if r.is_empty() {
        if capabilities.contains(Capabilities::SSL) {
            return Ok(ClientHello::Ssl(SslRequest {
                capabilities,
                max_packet,
                charset,
            }));
        }
        return Err(invalid("handshake response", "32 bytes without CLIENT_SSL"));
    }
    let username = utf8(
        r.nul_terminated(limits.max_username, "username")?,
        "username",
    )?;
    let auth_response = if capabilities.contains(Capabilities::PLUGIN_AUTH_LENENC_CLIENT_DATA) {
        r.lenenc_bytes(limits.max_auth_response, "auth response")?
    } else if capabilities.contains(Capabilities::SECURE_CONNECTION) {
        let n = usize::from(r.u8("auth response")?);
        if n > limits.max_auth_response {
            return Err(DecodeError::TooLong {
                what: "auth response",
                limit: limits.max_auth_response,
            });
        }
        r.bytes(n, "auth response")?
    } else {
        r.nul_terminated(limits.max_auth_response, "auth response")?
    }
    .to_vec();
    let database = if capabilities.contains(Capabilities::CONNECT_WITH_DB) {
        Some(utf8(
            r.nul_terminated(limits.max_database, "database")?,
            "database",
        )?)
    } else {
        None
    };
    let auth_plugin = if capabilities.contains(Capabilities::PLUGIN_AUTH) {
        Some(utf8(
            r.nul_terminated(limits.max_plugin, "auth plugin")?,
            "auth plugin",
        )?)
    } else {
        None
    };
    let mut attributes = Vec::new();
    if capabilities.contains(Capabilities::CONNECT_ATTRS) {
        let block = r.lenenc_bytes(limits.max_attributes_bytes, "attributes")?;
        let mut a = Reader::new(block);
        while !a.is_empty() {
            if attributes.len() == limits.max_attributes {
                return Err(DecodeError::TooLong {
                    what: "attribute count",
                    limit: limits.max_attributes,
                });
            }
            let k = utf8(
                a.lenenc_bytes(block.len(), "attribute name")?,
                "attribute name",
            )?;
            let v = utf8(
                a.lenenc_bytes(block.len(), "attribute value")?,
                "attribute value",
            )?;
            attributes.push((k, v));
        }
    }
    let zstd_level = if capabilities.contains(Capabilities::ZSTD_COMPRESSION) {
        Some(r.u8("zstd level")?)
    } else {
        None
    };
    if !r.is_empty() {
        return Err(invalid("handshake response", "trailing bytes"));
    }
    Ok(ClientHello::Response(HandshakeResponse41 {
        capabilities,
        max_packet,
        charset,
        username,
        auth_response,
        database,
        auth_plugin,
        attributes,
        zstd_level,
    }))
}

fn put_fixed_header(out: &mut Vec<u8>, caps: Capabilities, max_packet: u32, charset: u8) {
    out.extend_from_slice(&caps.0.to_le_bytes());
    out.extend_from_slice(&max_packet.to_le_bytes());
    out.push(charset);
    out.extend_from_slice(&[0; 23]);
}
