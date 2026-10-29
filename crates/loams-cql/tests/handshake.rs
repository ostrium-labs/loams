use std::sync::Arc;

use async_trait::async_trait;
use cassandra_protocol::{
    frame::{
        Direction, Envelope, Flags, Opcode, Serialize, Version,
        message_auth_response::BodyReqAuthResponse, message_startup::BodyReqStartup,
    },
    types::CBytes,
};
use loams_cql::{
    handshake::{AuthenticationError, Authenticator, Handshake},
    query::SessionContext,
};

struct FixedAuthenticator;

#[async_trait]
impl Authenticator for FixedAuthenticator {
    async fn authenticate(
        &self,
        namespace: &str,
        token: &str,
    ) -> Result<SessionContext, AuthenticationError> {
        if namespace == "tenant" && token == "valid-token" {
            Ok(SessionContext {
                namespace: namespace.into(),
                principal: "principal".into(),
                keyspace: None,
            })
        } else {
            Err(AuthenticationError)
        }
    }
}

fn request(opcode: Opcode, body: Vec<u8>) -> Envelope {
    Envelope::new(
        Version::V4,
        Direction::Request,
        Flags::empty(),
        opcode,
        12,
        body,
        None,
        vec![],
    )
}

#[tokio::test]
async fn options_startup_and_plain_auth_complete_the_handshake() {
    let mut handshake = Handshake::new(Arc::new(FixedAuthenticator));
    let response = handshake
        .handle(request(Opcode::Options, vec![]))
        .await
        .unwrap();
    assert_eq!(response.opcode, Opcode::Supported);
    assert_eq!(response.stream_id, 12);
    let body = BodyReqStartup::new(None, Version::V4).serialize_to_vec(Version::V4);
    let response = handshake
        .handle(request(Opcode::Startup, body))
        .await
        .unwrap();
    assert_eq!(response.opcode, Opcode::Authenticate);
    assert!(handshake.session().is_none());
    let body = BodyReqAuthResponse::new(CBytes::new(b"\0tenant\0valid-token".to_vec()))
        .serialize_to_vec(Version::V4);
    let response = handshake
        .handle(request(Opcode::AuthResponse, body))
        .await
        .unwrap();
    assert_eq!(response.opcode, Opcode::AuthSuccess);
    assert_eq!(handshake.session().unwrap().namespace, "tenant");
}

#[tokio::test]
async fn bad_plain_credentials_never_authenticate() {
    let mut handshake = Handshake::new(Arc::new(FixedAuthenticator));
    let body = BodyReqStartup::new(None, Version::V4).serialize_to_vec(Version::V4);
    handshake
        .handle(request(Opcode::Startup, body))
        .await
        .unwrap();
    let body = BodyReqAuthResponse::new(CBytes::new(b"\0tenant\0wrong".to_vec()))
        .serialize_to_vec(Version::V4);
    assert!(
        handshake
            .handle(request(Opcode::AuthResponse, body))
            .await
            .is_err()
    );
    assert!(handshake.session().is_none());
}

#[tokio::test]
async fn query_before_authentication_is_refused() {
    let mut handshake = Handshake::new(Arc::new(FixedAuthenticator));
    assert!(
        handshake
            .handle(request(Opcode::Query, vec![]))
            .await
            .is_err()
    );
    assert!(handshake.session().is_none());
}

#[tokio::test]
async fn malformed_plain_credentials_are_refused() {
    let mut handshake = Handshake::new(Arc::new(FixedAuthenticator));
    let body = BodyReqStartup::new(None, Version::V4).serialize_to_vec(Version::V4);
    handshake
        .handle(request(Opcode::Startup, body))
        .await
        .unwrap();
    let body = BodyReqAuthResponse::new(CBytes::new(b"tenant\0valid-token".to_vec()))
        .serialize_to_vec(Version::V4);
    assert!(
        handshake
            .handle(request(Opcode::AuthResponse, body))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn oversized_auth_length_is_refused_without_allocation() {
    let mut handshake = Handshake::new(Arc::new(FixedAuthenticator));
    let body = BodyReqStartup::new(None, Version::V4).serialize_to_vec(Version::V4);
    handshake
        .handle(request(Opcode::Startup, body))
        .await
        .unwrap();
    assert!(
        handshake
            .handle(request(
                Opcode::AuthResponse,
                i32::MAX.to_be_bytes().to_vec()
            ))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn oversized_startup_entry_count_is_refused_without_allocation() {
    let mut handshake = Handshake::new(Arc::new(FixedAuthenticator));
    assert!(
        handshake
            .handle(request(Opcode::Startup, vec![255, 255]))
            .await
            .is_err()
    );
}
