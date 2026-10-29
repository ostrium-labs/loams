use std::{net::{IpAddr, Ipv4Addr, SocketAddr}, sync::Arc, time::Duration};
use async_trait::async_trait;
use cassandra_protocol::{compression::Compression, frame::{Direction, Envelope, Flags, Opcode, Serialize, Version, message_auth_response::BodyReqAuthResponse, message_startup::BodyReqStartup, message_response::ResponseBody, message_result::ResResultBody}, types::CBytes};
use loams_cql::{handshake::{Authenticator,AuthenticationError},query::{QueryError, QueryExecutor, QueryResult, SessionContext},server::{Server,ServerConfig}};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Backend;
#[async_trait]
impl Authenticator for Backend {
    async fn authenticate(&self, namespace: &str, token: &str) -> Result<SessionContext, AuthenticationError> {
        if namespace != "tenant" || token != "token" { return Err(AuthenticationError); }
        Ok(SessionContext { namespace:namespace.into(), principal:"user".into(), keyspace:None })
    }
}
#[async_trait]
impl QueryExecutor for Backend {
    async fn execute(&self, session: &SessionContext, query: &str, _values: &[Option<Vec<u8>>]) -> Result<QueryResult, QueryError> {
        assert_eq!(session.namespace, "tenant");
        if query == "USE ks" { Ok(QueryResult::SetKeyspace("ks".into())) } else { Err(QueryError::Invalid("bad query".into())) }
    }
}
fn request(opcode: Opcode, body: Vec<u8>) -> Envelope { Envelope::new(Version::V4, Direction::Request, Flags::empty(), opcode, 3, body, None, vec![]) }
async fn exchange(stream: &mut tokio::net::TcpStream, frame: Envelope) -> Envelope {
    stream.write_all(&frame.encode_with(Compression::None).unwrap()).await.unwrap();
    let mut header = [0_u8; 9];
    stream.read_exact(&mut header).await.unwrap();
    let len = u32::from_be_bytes(header[5..9].try_into().unwrap()) as usize;
    let mut bytes = header.to_vec(); bytes.resize(9 + len, 0);
    stream.read_exact(&mut bytes[9..]).await.unwrap();
    Envelope::from_buffer(&bytes, Compression::None).unwrap().envelope
}

#[tokio::test]
async fn authenticated_query_round_trip_through_listener() {
    let backend = Arc::new(Backend);
    let server = Server::start(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0), backend.clone(), backend, ServerConfig::default()).await.unwrap();
    let mut stream = tokio::net::TcpStream::connect(server.local_addr()).await.unwrap();
    let startup = BodyReqStartup::new(None, Version::V4).serialize_to_vec(Version::V4);
    assert_eq!(exchange(&mut stream, request(Opcode::Startup, startup)).await.opcode, Opcode::Authenticate);
    let auth = BodyReqAuthResponse::new(CBytes::new(b"\0tenant\0token".to_vec())).serialize_to_vec(Version::V4);
    assert_eq!(exchange(&mut stream, request(Opcode::AuthResponse, auth)).await.opcode, Opcode::AuthSuccess);
    let query = b"USE ks"; let mut body = (query.len() as i32).to_be_bytes().to_vec(); body.extend_from_slice(query); body.extend_from_slice(&[0,1,0]);
    let response = exchange(&mut stream, request(Opcode::Query, body)).await;
    assert!(matches!(response.response_body().unwrap(), ResponseBody::Result(ResResultBody::SetKeyspace(_))));
    drop(stream); server.shutdown().await.unwrap();
}

#[tokio::test]
async fn non_loopback_listener_is_refused_without_tls() {
    let backend = Arc::new(Backend);
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0);
    assert!(Server::start(address, backend.clone(), backend, ServerConfig::default()).await.is_err());
}

#[tokio::test]
async fn oversized_frame_is_rejected_without_waiting_for_body() {
    let backend = Arc::new(Backend);
    let config = ServerConfig { max_body_bytes: 100, handshake_timeout: Duration::from_secs(1), ..ServerConfig::default() };
    let server = Server::start(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0), backend.clone(), backend, config).await.unwrap();
    let mut stream = tokio::net::TcpStream::connect(server.local_addr()).await.unwrap();
    stream.write_all(&[4,0,0,1,5,0,0,0,101]).await.unwrap();
    let mut byte = [0];
    let result = tokio::time::timeout(Duration::from_secs(2),stream.read(&mut byte)).await.unwrap().unwrap();
    assert_eq!(result,0);
    server.shutdown().await.unwrap();
}
