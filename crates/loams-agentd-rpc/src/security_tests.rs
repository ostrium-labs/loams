//! The server's security rules at the transport level (plan DD1 Task 7):
//! peers, `Hello` first, roles, scoped tokens and limits (the log canary is
//! `tests/token_canary.rs`, with a process-wide subscriber). The
//! daemon's own tests (`crates/loams-agentd/tests/security.rs`) cover the
//! upgrade checks over raw HTTP against the real binary.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use loams_agentd_proto::rpc::{FORBIDDEN_ROLE, LIMIT_EXCEEDED, UNAUTHENTICATED};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::test_support::{self, OWNER_TOKEN, dial, hello_params};
use crate::{Limits, RpcError, RpcReply, RpcService, connect_ws_with_token, methods};

/// `Echo` answers its params; `Open` is a stream that never ends.
struct Service;

#[async_trait]
impl RpcService for Service {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        match method {
            "Echo" | "Secret" => Ok(RpcReply::Value(params)),
            "Open" => Ok(RpcReply::Stream(
                futures::stream::once(async { serde_json::json!("open") })
                    .chain(futures::stream::pending())
                    .boxed(),
            )),
            other => Err(RpcError::UnknownMethod(other.into())),
        }
    }
}

fn upgrade_request(port: u16, token: Option<&str>) -> String {
    let mut request = format!(
        "GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");
    request
}

/// Whatever the server answers before closing (or within a second).
async fn answer_of(mut stream: tokio::io::DuplexStream) -> String {
    let mut bytes = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut bytes)).await;
    String::from_utf8_lossy(&bytes).into_owned()
}

#[tokio::test]
async fn a_non_loopback_peer_gets_no_answer() {
    let server = crate::WsServer::new(
        Arc::new(crate::Credentials::new(
            crate::Secret::new(OWNER_TOKEN),
            Arc::new(crate::ScopedTokens::new()),
        )),
        Arc::new(NoConnections),
        Arc::new(test_support::OwnerSecrets),
    );
    let shared = crate::server::testing::shared(server, 50217);
    for peer in ["192.0.2.7:40000", "[2001:db8::1]:40000", "10.0.0.2:1"] {
        let (mut client, server_end) = tokio::io::duplex(64 * 1024);
        let serving = tokio::spawn(crate::server::testing::serve_duplex(
            shared.clone(),
            server_end,
            peer.parse().unwrap(),
        ));
        client
            .write_all(upgrade_request(50217, Some(OWNER_TOKEN)).as_bytes())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), serving)
            .await
            .expect("the connection is dropped at once")
            .unwrap();
        assert_eq!(answer_of(client).await, "", "{peer} got an answer");
    }
    // The same request from loopback is upgraded.
    let (mut client, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(crate::server::testing::serve_duplex(
        shared,
        server_end,
        "127.0.0.1:40000".parse().unwrap(),
    ));
    client
        .write_all(upgrade_request(50217, Some(OWNER_TOKEN)).as_bytes())
        .await
        .unwrap();
    let mut head = vec![0u8; 12];
    client.read_exact(&mut head).await.unwrap();
    assert_eq!(&head, b"HTTP/1.1 101");
}

struct NoConnections;

impl crate::Connections for NoConnections {
    fn open(&self, _identity: &crate::Identity) -> Arc<dyn RpcService> {
        Arc::new(Service)
    }
}

#[tokio::test]
async fn a_listener_off_loopback_is_refused() {
    let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
    let server = crate::WsServer::new(
        Arc::new(crate::Credentials::new(
            crate::Secret::new(OWNER_TOKEN),
            Arc::new(crate::ScopedTokens::new()),
        )),
        Arc::new(NoConnections),
        Arc::new(test_support::OwnerSecrets),
    );
    let error = server
        .serve_until(listener, std::future::pending())
        .await
        .expect_err("0.0.0.0 is refused");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}

#[tokio::test]
async fn a_call_before_hello_is_refused_and_closes() {
    let served = test_support::serve(Arc::new(Service)).await;
    let client = connect_ws_with_token(&served.url(), OWNER_TOKEN)
        .await
        .unwrap();
    let err = client
        .call("Echo", serde_json::json!(1))
        .await
        .expect_err("Echo before Hello");
    assert!(err.to_string().starts_with(UNAUTHENTICATED), "{err}");
    // The connection is gone: not even Hello is answered now.
    let next = tokio::time::timeout(
        Duration::from_secs(5),
        client.call(methods::HELLO, hello_params()),
    )
    .await
    .expect("a closed connection answers at once");
    assert!(next.is_err(), "{next:?}");
}

#[tokio::test]
async fn a_call_batched_ahead_of_hello_is_refused() {
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let served = test_support::serve(Arc::new(Service)).await;
    let loopback = served.url();
    let mut request = loopback.into_client_request().unwrap();
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {OWNER_TOKEN}").parse().unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    ws.send(Message::Text(format!(
        "{{\"id\":1,\"method\":\"Echo\",\"params\":1}}\n{{\"id\":2,\"method\":\"{}\",\"params\":{}}}",
        methods::HELLO,
        hello_params()
    ).into()))
    .await
    .unwrap();
    let mut texts = Vec::new();
    while let Ok(Some(Ok(message))) = tokio::time::timeout(Duration::from_secs(5), ws.next()).await
    {
        match message {
            Message::Text(text) => texts.push(text.to_string()),
            Message::Close(_) => break,
            _ => {}
        }
    }
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(texts[0].contains(UNAUTHENTICATED), "{texts:?}");
}

#[tokio::test]
async fn a_policy_refusal_is_forbidden_role_and_keeps_the_connection() {
    let served = test_support::serve(Arc::new(Service)).await;
    let run = served.scoped.issue("chat-1").unwrap();
    let mcp = dial(&served.url(), run.token.expose()).await;
    let err = mcp
        .call("Secret", serde_json::json!(1))
        .await
        .expect_err("Secret is owner-only");
    assert!(err.to_string().starts_with(FORBIDDEN_ROLE), "{err}");
    assert_eq!(
        mcp.call("Echo", serde_json::json!(2)).await.unwrap(),
        serde_json::json!(2)
    );
    let owner = dial(&served.url(), OWNER_TOKEN).await;
    assert_eq!(
        owner.call("Secret", serde_json::json!(3)).await.unwrap(),
        serde_json::json!(3)
    );
}

#[tokio::test]
async fn hello_reports_the_role_of_the_token() {
    let served = test_support::serve(Arc::new(Service)).await;
    let run = served.scoped.issue("chat-1").unwrap();
    for (token, role) in [(OWNER_TOKEN, "owner"), (run.token.expose(), "mcp")] {
        let client = connect_ws_with_token(&served.url(), token).await.unwrap();
        let reply = client.call(methods::HELLO, hello_params()).await.unwrap();
        assert_eq!(reply["role"], role);
    }
}

#[tokio::test]
async fn a_revoked_run_token_ends_its_connection_and_is_refused() {
    let served = test_support::serve(Arc::new(Service)).await;
    let run = served.scoped.issue("chat-1").unwrap();
    let client = dial(&served.url(), run.token.expose()).await;
    let mut stream = client
        .subscribe("Open", serde_json::Value::Null)
        .await
        .unwrap();
    assert_eq!(stream.recv().await, Some(serde_json::json!("open")));

    served.scoped.revoke(&run.scope.run_id);
    // The open stream ends with the connection, without another call.
    let ended = tokio::time::timeout(Duration::from_secs(5), stream.recv())
        .await
        .expect("the revoked connection closes");
    assert_eq!(ended, None);
    assert!(client.call("Echo", serde_json::json!(1)).await.is_err());
    let again = connect_ws_with_token(&served.url(), run.token.expose()).await;
    let error = again.expect_err("a revoked token is refused");
    assert!(error.to_string().contains("401"), "{error}");
}

#[tokio::test]
async fn frame_limit_closes_the_connection() {
    let limits = Limits {
        max_frame_bytes: 4096,
        ..Limits::default()
    };
    let served = test_support::serve_with(Arc::new(Service), limits).await;
    let client = dial(&served.url(), OWNER_TOKEN).await;
    let small = "x".repeat(1024);
    assert_eq!(
        client.call("Echo", serde_json::json!(small)).await.unwrap(),
        serde_json::json!(small)
    );
    let big = "x".repeat(8192);
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        client.call("Echo", serde_json::json!(big)),
    )
    .await
    .expect("an oversized frame ends the connection");
    assert!(result.is_err(), "{result:?}");
}

#[tokio::test]
async fn stream_limit_is_limit_exceeded() {
    let limits = Limits {
        max_streams: 2,
        ..Limits::default()
    };
    let served = test_support::serve_with(Arc::new(Service), limits).await;
    let client = dial(&served.url(), OWNER_TOKEN).await;
    let mut first = client
        .subscribe_scoped("Open", serde_json::Value::Null)
        .await
        .unwrap();
    let mut second = client
        .subscribe_scoped("Open", serde_json::Value::Null)
        .await
        .unwrap();
    assert!(first.recv().await.is_some());
    assert!(second.recv().await.is_some());
    let third = client
        .subscribe_checked("Open", serde_json::Value::Null)
        .await;
    let error = third.expect_err("a third stream is refused");
    assert!(error.to_string().starts_with(LIMIT_EXCEEDED), "{error}");
    // Unary calls still work, and a closed stream frees its slot.
    assert!(client.call("Echo", serde_json::json!(1)).await.is_ok());
    drop(first);
    let mut fourth = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client
                .subscribe_checked("Open", serde_json::Value::Null)
                .await
            {
                Ok(stream) => break stream,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await
    .expect("the slot frees once the stream is cancelled");
    assert!(fourth.recv().await.is_some());
}

#[tokio::test]
async fn owner_connections_are_limited() {
    let limits = Limits {
        max_owner_connections: 2,
        ..Limits::default()
    };
    let served = test_support::serve_with(Arc::new(Service), limits).await;
    let first = dial(&served.url(), OWNER_TOKEN).await;
    let _second = dial(&served.url(), OWNER_TOKEN).await;
    let third = connect_ws_with_token(&served.url(), OWNER_TOKEN).await;
    let error = third.expect_err("a third owner connection is refused");
    assert!(error.to_string().contains("429"), "{error}");
    // Scoped tokens do not count against the owner limit.
    let run = served.scoped.issue("chat").unwrap();
    let _mcp = dial(&served.url(), run.token.expose()).await;
    drop(first);
    tokio::time::timeout(Duration::from_secs(5), async {
        while connect_ws_with_token(&served.url(), OWNER_TOKEN)
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a closed connection frees its slot");
}

#[tokio::test]
async fn unauthenticated_connections_are_bounded() {
    let limits = Limits {
        max_pending_upgrades: 1,
        ..Limits::default()
    };
    let served = test_support::serve_with(Arc::new(Service), limits).await;
    // A peer that connects and never finishes its upgrade holds the one slot.
    let stalled = tokio::net::TcpStream::connect(("127.0.0.1", served.port))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let refused = connect_ws_with_token(&served.url(), OWNER_TOKEN).await;
    assert!(refused.is_err(), "a second pending upgrade was served");
    drop(stalled);
    tokio::time::timeout(Duration::from_secs(5), async {
        while connect_ws_with_token(&served.url(), OWNER_TOKEN)
            .await
            .is_err()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the slot frees when the stalled peer leaves");
}
