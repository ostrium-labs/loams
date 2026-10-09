//! The connection phase as a sans-I/O state machine (controller ruling,
//! Task 3 review; R3.13): greeting → optional `SSLRequest` and TLS →
//! `HandshakeResponse41` → `caching_sha2_password` → OK.
//!
//! Rules it enforces:
//! - TLS is required, unless the caller allows plaintext (loopback only);
//! - at most one `SSLRequest`;
//! - after TLS, the response must carry `CLIENT_SSL` and the same
//!   capabilities as the `SSLRequest`;
//! - a plaintext response with `CLIENT_SSL` set (no `SSLRequest`) is refused;
//! - handshake messages are capped at [`HANDSHAKE_MAX_MESSAGE`].
//!
//! The caller owns the socket and TLS. It feeds bytes to [`ConnectionPhase::on_bytes`]
//! and acts on each [`Step`]; verdicts come back through
//! [`ConnectionPhase::fast_result`] and [`ConnectionPhase::full_result`].

use std::fmt;

use super::DecodeError;
use super::auth::{Action, AuthError, CachingSha2Server, Password};
use super::command::{ErrPacket, OkPacket};
use super::handshake::{
    Capabilities, ClientHello, HandshakeResponse41, HandshakeV10, Limits, NegotiationError,
    decode_client_hello, negotiate,
};
use super::packet::{Assembler, FrameError, encode};

/// The largest handshake message: [`Limits::default`]'s fields (attributes
/// up to 64 KiB) with room for the fixed parts.
pub const HANDSHAKE_MAX_MESSAGE: usize = 96 * 1024;

/// What the caller does next.
pub enum Step {
    /// More bytes are needed.
    NeedMore,
    /// Start server-side TLS on the connection (bytes after the consumed
    /// ones belong to TLS), then call [`ConnectionPhase::tls_established`].
    StartTls,
    /// Write these bytes (already framed) and keep reading.
    Write(Vec<u8>),
    /// Look `user` up in the fast-auth cache, verify `scramble` and report
    /// with [`ConnectionPhase::fast_result`].
    CheckFast {
        /// The user name.
        user: String,
        /// The client's scramble.
        scramble: Vec<u8>,
    },
    /// Check `password` against `user`'s stored hash and report with
    /// [`ConnectionPhase::full_result`].
    CheckFull {
        /// The user name.
        user: String,
        /// The cleartext password (zeroed on drop).
        password: Password,
    },
    /// Authenticated: write these bytes (the OK); the command phase begins.
    Done(Vec<u8>),
}

impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::NeedMore => f.write_str("NeedMore"),
            Step::StartTls => f.write_str("StartTls"),
            Step::Write(b) => write!(f, "Write({} bytes)", b.len()),
            Step::CheckFast { user, .. } => {
                write!(f, "CheckFast {{ user: {user:?}, scramble: [redacted] }}")
            }
            Step::CheckFull { user, .. } => {
                write!(f, "CheckFull {{ user: {user:?}, password: [redacted] }}")
            }
            Step::Done(b) => write!(f, "Done({} bytes)", b.len()),
        }
    }
}

/// Why the connection phase stopped. [`ConnectionPhase::error_packet`]
/// gives the ERR to send before closing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PhaseError {
    /// Bad framing (sequence, size).
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// A malformed handshake message.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// A plaintext response where TLS is required.
    #[error("TLS is required")]
    TlsRequired,
    /// A second `SSLRequest`.
    #[error("duplicate SSLRequest")]
    DuplicateSslRequest,
    /// After TLS: no `CLIENT_SSL`, or capabilities unlike the `SSLRequest`'s.
    #[error("response capabilities do not match the SSLRequest")]
    SslMismatch,
    /// A plaintext response claiming `CLIENT_SSL`.
    #[error("CLIENT_SSL without an SSLRequest")]
    SslWithoutRequest,
    /// Missing required capabilities.
    #[error(transparent)]
    Negotiation(#[from] NegotiationError),
    /// Authentication failed.
    #[error(transparent)]
    Auth(#[from] AuthError),
    /// A call out of order.
    #[error("unexpected in this state")]
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitHello,
    AwaitTls,
    AwaitTlsResponse,
    Authenticating,
    AwaitFast,
    AwaitFull,
    Done,
    Failed,
}

/// One client connection's handshake.
#[derive(Debug)]
pub struct ConnectionPhase {
    greeting: HandshakeV10,
    limits: Limits,
    plaintext_allowed: bool,
    tls: bool,
    ssl_request: Option<Capabilities>,
    assembler: Assembler,
    phase: Phase,
    auth: Option<CachingSha2Server>,
    response: Option<HandshakeResponse41>,
    agreed: Option<Capabilities>,
}

impl ConnectionPhase {
    /// Starts a connection with `greeting` (its nonce fresh from `OsRng`,
    /// its capabilities from [`super::handshake::advertise`]). Returns the
    /// machine and the framed greeting to write. `plaintext_allowed` only
    /// for loopback connections.
    pub fn new(greeting: HandshakeV10, plaintext_allowed: bool, limits: Limits) -> (Self, Vec<u8>) {
        let mut out = Vec::new();
        let mut seq = 0;
        encode(&greeting.encode(), &mut seq, &mut out);
        let mut assembler = Assembler::new(HANDSHAKE_MAX_MESSAGE);
        assembler.expect_seq(seq);
        let phase = Self {
            greeting,
            limits,
            plaintext_allowed,
            tls: false,
            ssl_request: None,
            assembler,
            phase: Phase::AwaitHello,
            auth: None,
            response: None,
            agreed: None,
        };
        (phase, out)
    }

    /// Feeds client bytes. Returns how many were consumed and the next step.
    pub fn on_bytes(&mut self, input: &[u8]) -> Result<(usize, Step), PhaseError> {
        if !matches!(
            self.phase,
            Phase::AwaitHello | Phase::AwaitTlsResponse | Phase::Authenticating
        ) {
            return Err(self.failed(PhaseError::Unexpected));
        }
        let (used, msg) = match self.assembler.push(input) {
            Ok(r) => r,
            Err(e) => return Err(self.failed(e.into())),
        };
        let Some(msg) = msg else {
            return Ok((used, Step::NeedMore));
        };
        let mut payload = msg.payload;
        let step = match self.phase {
            Phase::AwaitHello => self.on_hello(&payload),
            Phase::AwaitTlsResponse => self.on_tls_response(&payload),
            _ => self.on_auth_packet(&payload),
        };
        // The payload may hold a cleartext password (full auth).
        payload.fill(0);
        std::hint::black_box(&payload);
        match step {
            Ok(step) => Ok((used, step)),
            Err(e) => Err(self.failed(e)),
        }
    }

    /// TLS is up after [`Step::StartTls`].
    pub fn tls_established(&mut self) -> Result<(), PhaseError> {
        if self.phase != Phase::AwaitTls {
            return Err(self.failed(PhaseError::Unexpected));
        }
        self.tls = true;
        self.phase = Phase::AwaitTlsResponse;
        Ok(())
    }

    /// Reports the fast-auth lookup for [`Step::CheckFast`].
    pub fn fast_result(&mut self, hit: bool) -> Result<Step, PhaseError> {
        if self.phase != Phase::AwaitFast {
            return Err(self.failed(PhaseError::Unexpected));
        }
        let action = match self.auth.as_mut() {
            Some(a) => a.fast_result(hit),
            None => return Err(self.failed(PhaseError::Unexpected)),
        };
        match action {
            Action::Send(p) if hit => {
                let mut out = self.frame(&p);
                out.extend(self.ok());
                self.phase = Phase::Done;
                Ok(Step::Done(out))
            }
            other => {
                let r = self.act(other);
                r.map_err(|e| self.failed(e))
            }
        }
    }

    /// Reports the full check for [`Step::CheckFull`].
    pub fn full_result(&mut self, ok: bool) -> Result<Step, PhaseError> {
        if self.phase != Phase::AwaitFull {
            return Err(self.failed(PhaseError::Unexpected));
        }
        if !ok {
            return Err(self.failed(PhaseError::Auth(AuthError::AccessDenied)));
        }
        self.phase = Phase::Done;
        Ok(Step::Done(self.ok()))
    }

    /// The framed ERR packet to send for `err` before closing.
    pub fn error_packet(&mut self, err: &PhaseError) -> Vec<u8> {
        let user = self
            .response
            .as_ref()
            .map_or("", |r| r.username.as_str())
            .to_owned();
        let packet = match err {
            PhaseError::TlsRequired | PhaseError::Auth(AuthError::SecureTransportRequired) => {
                ErrPacket::new(
                    3159,
                    *b"HY000",
                    "Connections using insecure transport are prohibited",
                )
            }
            PhaseError::Auth(_) => {
                ErrPacket::new(1045, *b"28000", &format!("Access denied for user '{user}'"))
            }
            _ => ErrPacket::new(1043, *b"08S01", "Bad handshake"),
        };
        self.frame(&packet.encode())
    }

    /// The client's response, once received, without its auth data.
    pub fn response(&self) -> Option<&HandshakeResponse41> {
        self.response.as_ref()
    }

    /// The capabilities in force, once negotiated.
    pub fn agreed(&self) -> Option<Capabilities> {
        self.agreed
    }

    /// Whether the client is on TLS.
    pub fn is_tls(&self) -> bool {
        self.tls
    }

    /// Authenticated.
    pub fn is_done(&self) -> bool {
        self.phase == Phase::Done
    }

    fn on_hello(&mut self, payload: &[u8]) -> Result<Step, PhaseError> {
        match decode_client_hello(payload, &self.limits)? {
            ClientHello::Ssl(s) => {
                self.ssl_request = Some(s.capabilities);
                self.phase = Phase::AwaitTls;
                Ok(Step::StartTls)
            }
            ClientHello::Response(r) => {
                if r.capabilities.contains(Capabilities::SSL) {
                    return Err(PhaseError::SslWithoutRequest);
                }
                if !self.plaintext_allowed {
                    return Err(PhaseError::TlsRequired);
                }
                self.start_auth(r)
            }
        }
    }

    fn on_tls_response(&mut self, payload: &[u8]) -> Result<Step, PhaseError> {
        match decode_client_hello(payload, &self.limits)? {
            ClientHello::Ssl(_) => Err(PhaseError::DuplicateSslRequest),
            ClientHello::Response(r) => {
                if !r.capabilities.contains(Capabilities::SSL)
                    || Some(r.capabilities) != self.ssl_request
                {
                    return Err(PhaseError::SslMismatch);
                }
                self.start_auth(r)
            }
        }
    }

    fn start_auth(&mut self, mut r: HandshakeResponse41) -> Result<Step, PhaseError> {
        // The first packet's auth data is used once, then zeroed (dropped
        // here); the kept response holds an empty Password.
        let auth_data = std::mem::replace(&mut r.auth_response, Password::new(Vec::new()));
        let agreed = negotiate(r.capabilities, self.greeting.capabilities)?;
        self.agreed = Some(agreed);
        let mut auth = CachingSha2Server::new(self.greeting.nonce, self.tls);
        let action = auth.start(r.auth_plugin.as_deref(), auth_data.expose());
        drop(auth_data);
        self.auth = Some(auth);
        self.response = Some(r);
        self.act(action)
    }

    fn on_auth_packet(&mut self, payload: &[u8]) -> Result<Step, PhaseError> {
        let action = self
            .auth
            .as_mut()
            .ok_or(PhaseError::Unexpected)?
            .on_packet(payload)?;
        self.act(action)
    }

    fn act(&mut self, action: Action) -> Result<Step, PhaseError> {
        let user = self
            .response
            .as_ref()
            .map(|r| r.username.clone())
            .unwrap_or_default();
        match action {
            Action::Send(p) => {
                self.phase = Phase::Authenticating;
                Ok(Step::Write(self.frame(&p)))
            }
            Action::CheckFast { scramble } => {
                self.phase = Phase::AwaitFast;
                Ok(Step::CheckFast { user, scramble })
            }
            Action::CheckFull { password } => {
                self.phase = Phase::AwaitFull;
                Ok(Step::CheckFull { user, password })
            }
            Action::Fail(e) => Err(e.into()),
        }
    }

    fn ok(&mut self) -> Vec<u8> {
        let caps = self.agreed.unwrap_or(Capabilities(0));
        let ok = OkPacket {
            affected_rows: 0,
            last_insert_id: 0,
            status: 0x0002,
            warnings: 0,
            info: Vec::new(),
        };
        self.frame(&ok.encode(caps))
    }

    fn frame(&mut self, payload: &[u8]) -> Vec<u8> {
        let mut seq = self.assembler.next_seq();
        let mut out = Vec::with_capacity(payload.len() + 4);
        encode(payload, &mut seq, &mut out);
        self.assembler.expect_seq(seq);
        out
    }

    fn failed(&mut self, e: PhaseError) -> PhaseError {
        self.phase = Phase::Failed;
        e
    }
}
