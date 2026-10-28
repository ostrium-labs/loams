//! Bounded Cassandra V4 startup and SASL PLAIN authentication.

use std::{collections::HashMap, fmt, sync::Arc};

use async_trait::async_trait;
use cassandra_protocol::{
    frame::{
        Direction, Envelope, Flags, Opcode, Serialize, Version,
        message_auth_success::BodyReqAuthSuccess, message_authenticate::BodyResAuthenticate,
        message_request::RequestBody, message_supported::BodyResSupported,
    },
    types::CBytes,
};

use crate::query::SessionContext;

const MAX_STARTUP_OPTIONS: usize = 64;
const MAX_STARTUP_STRING: usize = 4096;
const MAX_CREDENTIAL_BYTES: usize = 16 * 1024;

/// An authentication failure. Credentials are never included in the error.
#[derive(Clone, Copy, Debug)]
pub struct AuthenticationError;

/// Checks a namespace and token using the gateway's authorization service.
#[async_trait]
pub trait Authenticator: Send + Sync {
    /// Authenticate without retaining the token in the resulting session.
    async fn authenticate(
        &self,
        namespace: &str,
        token: &str,
    ) -> Result<SessionContext, AuthenticationError>;
}

/// A protocol or authentication rejection during startup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeError {
    /// A malformed, unsupported, or out-of-order message.
    Protocol,
    /// The supplied credentials were rejected.
    Authentication,
}
impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol => f.write_str("invalid or unsupported CQL handshake"),
            Self::Authentication => f.write_str("CQL authentication failed"),
        }
    }
}
impl std::error::Error for HandshakeError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Initial,
    Authenticate,
    Ready,
}

/// Per-connection startup state. Every connection must authenticate.
pub struct Handshake {
    authenticator: Arc<dyn Authenticator>,
    state: State,
    session: Option<SessionContext>,
}

impl fmt::Debug for Handshake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Handshake")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Handshake {
    /// Construct a handshake requiring the provided authenticator.
    #[must_use]
    pub fn new(authenticator: Arc<dyn Authenticator>) -> Self {
        Self {
            authenticator,
            state: State::Initial,
            session: None,
        }
    }

    /// The authenticated session, available only after SASL succeeds.
    #[must_use]
    pub fn session(&self) -> Option<&SessionContext> {
        self.session.as_ref()
    }

    /// Handle an initial V4 request and return its correlated response.
    ///
    /// V5 is refused until its segmented transport is implemented, so clients
    /// may negotiate down instead of receiving invalid V5 frames.
    pub async fn handle(&mut self, request: Envelope) -> Result<Envelope, HandshakeError> {
        if request.version != Version::V4
            || request.direction != Direction::Request
            || !request.flags.is_empty()
        {
            return Err(HandshakeError::Protocol);
        }
        let version = request.version;
        let (opcode, body) = match (self.state, request.opcode) {
            (State::Initial, Opcode::Options) if request.body.is_empty() => {
                let data = HashMap::from([("CQL_VERSION".into(), vec!["3.0.0".into()])]);
                (
                    Opcode::Supported,
                    BodyResSupported { data }.serialize_to_vec(version),
                )
            }
            (State::Initial, Opcode::Startup) => {
                validate_startup_map(&request.body)?;
                let RequestBody::Startup(body) = request
                    .request_body()
                    .map_err(|_| HandshakeError::Protocol)?
                else {
                    return Err(HandshakeError::Protocol);
                };
                if body.map.get("CQL_VERSION").map(String::as_str) != Some("3.0.0")
                    || body.map.contains_key("COMPRESSION")
                {
                    return Err(HandshakeError::Protocol);
                }
                self.state = State::Authenticate;
                let body = BodyResAuthenticate {
                    data: "org.apache.cassandra.auth.PasswordAuthenticator".into(),
                };
                (Opcode::Authenticate, body.serialize_to_vec(version))
            }
            (State::Authenticate, Opcode::AuthResponse) => {
                validate_auth_bytes(&request.body)?;
                let RequestBody::AuthResponse(body) = request
                    .request_body()
                    .map_err(|_| HandshakeError::Protocol)?
                else {
                    return Err(HandshakeError::Protocol);
                };
                let data = body.data.as_slice().ok_or(HandshakeError::Authentication)?;
                let parts: Vec<&[u8]> = data.split(|&byte| byte == 0).collect();
                if parts.len() != 3
                    || parts[1].is_empty()
                    || parts[2].is_empty()
                    || (!parts[0].is_empty() && parts[0] != parts[1])
                {
                    return Err(HandshakeError::Authentication);
                }
                let namespace =
                    std::str::from_utf8(parts[1]).map_err(|_| HandshakeError::Authentication)?;
                let token =
                    std::str::from_utf8(parts[2]).map_err(|_| HandshakeError::Authentication)?;
                let session = self
                    .authenticator
                    .authenticate(namespace, token)
                    .await
                    .map_err(|_| HandshakeError::Authentication)?;
                if session.namespace != namespace {
                    return Err(HandshakeError::Authentication);
                }
                self.session = Some(session);
                self.state = State::Ready;
                (
                    Opcode::AuthSuccess,
                    BodyReqAuthSuccess {
                        data: CBytes::new_null(),
                    }
                    .serialize_to_vec(version),
                )
            }
            _ => return Err(HandshakeError::Protocol),
        };
        Ok(Envelope::new(
            version,
            Direction::Response,
            Flags::empty(),
            opcode,
            request.stream_id,
            body,
            None,
            vec![],
        ))
    }
}

// `cassandra-protocol` allocates using lengths in the payload. Check those
// lengths against the existing body before invoking its parsers.
fn validate_auth_bytes(body: &[u8]) -> Result<(), HandshakeError> {
    let header: [u8; 4] = body
        .get(..4)
        .ok_or(HandshakeError::Protocol)?
        .try_into()
        .map_err(|_| HandshakeError::Protocol)?;
    let len = usize::try_from(i32::from_be_bytes(header)).map_err(|_| HandshakeError::Protocol)?;
    if len > MAX_CREDENTIAL_BYTES || body.len() != 4 + len {
        return Err(HandshakeError::Protocol);
    }
    Ok(())
}

fn validate_startup_map(body: &[u8]) -> Result<(), HandshakeError> {
    let count_bytes: [u8; 2] = body
        .get(..2)
        .ok_or(HandshakeError::Protocol)?
        .try_into()
        .map_err(|_| HandshakeError::Protocol)?;
    let count = usize::from(u16::from_be_bytes(count_bytes));
    if count > MAX_STARTUP_OPTIONS {
        return Err(HandshakeError::Protocol);
    }
    let mut offset = 2_usize;
    for _ in 0..count * 2 {
        let len_bytes: [u8; 2] = body
            .get(offset..offset + 2)
            .ok_or(HandshakeError::Protocol)?
            .try_into()
            .map_err(|_| HandshakeError::Protocol)?;
        let len = usize::from(u16::from_be_bytes(len_bytes));
        offset += 2;
        if len > MAX_STARTUP_STRING {
            return Err(HandshakeError::Protocol);
        }
        let value = body
            .get(offset..offset + len)
            .ok_or(HandshakeError::Protocol)?;
        std::str::from_utf8(value).map_err(|_| HandshakeError::Protocol)?;
        offset += len;
    }
    if offset != body.len() {
        return Err(HandshakeError::Protocol);
    }
    Ok(())
}
