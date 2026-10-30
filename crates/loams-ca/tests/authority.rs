//! The CA service over its document (design §83 §6.2–§6.6, CT1 Task 3 part
//! 2b).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use loams_ca::authority::RETIRING_INTERMEDIATE_KEPT;
use loams_ca::{Authority, CaServiceError, KeySealer, MemoryDocStore, Revoke};
use loams_transport::{
    Alpn, DenyList, Identity, SpiffeId, TokenError, VerifiedPeer, new_key_and_csr, peer_identity,
};
use rustls::{ClientConnection, ServerConnection};
use rustls_pki_types::ServerName;
use tower::ServiceExt as _;

fn id(s: &str) -> SpiffeId {
    s.parse().unwrap()
}

fn ids(list: &[&str]) -> Vec<SpiffeId> {
    list.iter().map(|s| id(s)).collect()
}

struct World {
    ca: Arc<Authority>,
    root_key: String,
}

async fn world(now: SystemTime) -> World {
    let docs = Arc::new(MemoryDocStore::default());
    let ca = Arc::new(Authority::new(docs, Ok(KeySealer::generate())));
    let root_key = ca.init("c1", now).await.unwrap().expose_pem().to_string();
    World { ca, root_key }
}

/// A joining process: a key in memory, a CSR, a leaf.
async fn join(
    ca: &Authority,
    token: &str,
    asked: &[SpiffeId],
    now: SystemTime,
) -> Result<Identity, CaServiceError> {
    let (key, csr) = new_key_and_csr(asked, &["localhost".to_string()]).unwrap();
    let issued = ca.issue(token, &csr, now).await?;
    let bundle = ca.bundle(now).await.unwrap();
    Ok(Identity::new(issued.chain, key, bundle.roots).unwrap())
}

fn peer(identity: &Identity) -> VerifiedPeer {
    VerifiedPeer {
        identity: identity.ids().clone(),
        serial: identity.serial().to_vec(),
    }
}

async fn token(ca: &Authority, grant: &[&str], now: SystemTime) -> String {
    ca.create_token(ids(grant), now).await.unwrap().expose()
}

fn handshake(server: &Identity, client: &Identity) -> Result<ServerConnection, rustls::Error> {
    let mut s = ServerConnection::new(server.server_config(Alpn::H2).unwrap()).unwrap();
    let mut c = ClientConnection::new(
        client.client_config(Alpn::H2).unwrap(),
        ServerName::try_from("localhost").unwrap(),
    )
    .unwrap();
    for _ in 0..16 {
        let mut buf = Vec::new();
        while c.wants_write() {
            c.write_tls(&mut buf).unwrap();
        }
        s.read_tls(&mut buf.as_slice()).unwrap();
        s.process_new_packets()?;
        let mut buf = Vec::new();
        while s.wants_write() {
            s.write_tls(&mut buf).unwrap();
        }
        c.read_tls(&mut buf.as_slice()).unwrap();
        c.process_new_packets()?;
        if !s.is_handshaking() && !c.is_handshaking() {
            let mut buf = Vec::new();
            while c.wants_write() {
                c.write_tls(&mut buf).unwrap();
            }
            s.read_tls(&mut buf.as_slice()).unwrap();
            s.process_new_packets()?;
            return Ok(s);
        }
    }
    panic!("handshake did not finish");
}

#[tokio::test]
async fn joined_processes_talk_mutual_tls_and_renew() {
    let now = SystemTime::now();
    let w = world(now).await;
    let node_ids = ["spiffe://c1/node/1", "spiffe://c1/role/meta"];
    let t = token(&w.ca, &node_ids, now).await;
    let node = join(&w.ca, &t, &ids(&node_ids), now).await.unwrap();
    let t = token(&w.ca, &["spiffe://c1/svc/meter-agent"], now).await;
    let agent = join(&w.ca, &t, &ids(&["spiffe://c1/svc/meter-agent"]), now)
        .await
        .unwrap();
    let s = handshake(&node, &agent).unwrap();
    let caller = peer_identity(s.peer_certificates().unwrap()).unwrap();
    assert!(caller.has("svc/meter-agent"));

    // Renewal: a new serial, the same identities, renewal due at two-thirds
    // of the leaf's life. (Signed at `now`: the handshake below checks the
    // validity window against the real clock.)
    let (key, csr) = new_key_and_csr(&ids(&node_ids), &["localhost".to_string()]).unwrap();
    let renewed = w.ca.renew(&peer(&node), &csr, now).await.unwrap();
    assert_ne!(renewed.serial, node.serial());
    let life = renewed
        .not_after
        .duration_since(now - loams_transport::ca::BACKDATE)
        .unwrap();
    assert_eq!(
        renewed.renew_at,
        now - loams_transport::ca::BACKDATE + life * 2 / 3
    );
    let later = now + Duration::from_secs(16 * 3600);
    let renewed = Identity::new(renewed.chain, key, node.bundle().to_vec()).unwrap();
    assert_eq!(renewed.ids(), node.ids());
    handshake(&renewed, &agent).unwrap();

    let status = w.ca.status(later).await.unwrap();
    assert_eq!(status.issued_live, 3);
    assert_eq!(status.tokens_spent, 2);
    assert_eq!(status.next_serial, 4);
}

#[tokio::test]
async fn issue_refuses_spent_token() {
    let now = SystemTime::now();
    let w = world(now).await;
    let t = token(&w.ca, &["spiffe://c1/node/1"], now).await;
    join(&w.ca, &t, &ids(&["spiffe://c1/node/1"]), now)
        .await
        .unwrap();
    let again = join(&w.ca, &t, &ids(&["spiffe://c1/node/1"]), now).await;
    assert_eq!(again.unwrap_err(), CaServiceError::Token(TokenError::Spent));

    // Expired: an hour later. Unknown: a token this CA never made.
    let t = token(&w.ca, &["spiffe://c1/node/2"], now).await;
    let late = join(
        &w.ca,
        &t,
        &ids(&["spiffe://c1/node/2"]),
        now + Duration::from_secs(3601),
    )
    .await;
    assert_eq!(
        late.unwrap_err(),
        CaServiceError::Token(TokenError::Expired)
    );
    let other = loams_transport::JoinToken::generate().unwrap().expose();
    let unknown = join(&w.ca, &other, &ids(&["spiffe://c1/node/2"]), now).await;
    assert_eq!(
        unknown.unwrap_err(),
        CaServiceError::Token(TokenError::Unknown)
    );
    // A wrong secret for a real id is unknown too.
    let real = loams_transport::JoinToken::parse(&t).unwrap();
    let forged = format!("loamsjt1.{}.{}", real.id, "A".repeat(43));
    let forged = join(&w.ca, &forged, &ids(&["spiffe://c1/node/2"]), now).await;
    assert_eq!(
        forged.unwrap_err(),
        CaServiceError::Token(TokenError::Unknown)
    );
}

#[tokio::test]
async fn issue_refuses_identity_outside_token_grant() {
    let now = SystemTime::now();
    let w = world(now).await;
    let t = token(&w.ca, &["spiffe://c1/svc/meter-agent"], now).await;
    let err = join(
        &w.ca,
        &t,
        &ids(&["spiffe://c1/svc/meter-agent", "spiffe://c1/role/meta"]),
        now,
    )
    .await
    .unwrap_err();
    assert_eq!(err.reason(), "identity_not_granted");
    // Another cluster's identity is refused too.
    let err = join(&w.ca, &t, &ids(&["spiffe://c2/svc/meter-agent"]), now)
        .await
        .unwrap_err();
    assert_eq!(err.reason(), "identity_not_granted");
    // A refused CSR does not spend the token.
    join(&w.ca, &t, &ids(&["spiffe://c1/svc/meter-agent"]), now)
        .await
        .unwrap();
    // A token cannot grant another cluster's identity.
    let err =
        w.ca.create_token(ids(&["spiffe://c2/node/1"]), now)
            .await
            .unwrap_err();
    assert_eq!(err.reason(), "identity_not_granted");
}

#[tokio::test]
async fn renew_requires_matching_identity() {
    let now = SystemTime::now();
    let w = world(now).await;
    let t = token(&w.ca, &["spiffe://c1/svc/meter-agent"], now).await;
    let agent = join(&w.ca, &t, &ids(&["spiffe://c1/svc/meter-agent"]), now)
        .await
        .unwrap();
    // The agent asks to renew as a meta node: refused.
    let (_, csr) = new_key_and_csr(&ids(&["spiffe://c1/role/meta"]), &[]).unwrap();
    let err = w.ca.renew(&peer(&agent), &csr, now).await.unwrap_err();
    assert_eq!(err.reason(), "identity_not_granted");
    // Its own identity: renewed.
    let (_, csr) = new_key_and_csr(&ids(&["spiffe://c1/svc/meter-agent"]), &[]).unwrap();
    w.ca.renew(&peer(&agent), &csr, now).await.unwrap();
}

#[tokio::test]
async fn revoked_identity_cannot_renew() {
    let now = SystemTime::now();
    let w = world(now).await;
    let node_ids = ["spiffe://c1/node/7", "spiffe://c1/role/gateway"];
    let t = token(&w.ca, &node_ids, now).await;
    let node = join(&w.ca, &t, &ids(&node_ids), now).await.unwrap();
    let t = token(&w.ca, &["spiffe://c1/svc/agentd"], now).await;
    let agentd = join(&w.ca, &t, &ids(&["spiffe://c1/svc/agentd"]), now)
        .await
        .unwrap();

    // By node id.
    w.ca.revoke(Revoke::Node("7".into()), now).await.unwrap();
    let (_, csr) = new_key_and_csr(&ids(&node_ids), &[]).unwrap();
    let err = w.ca.renew(&peer(&node), &csr, now).await.unwrap_err();
    assert_eq!(err.reason(), "identity_revoked");
    // A new token for a revoked identity is refused too.
    let err =
        w.ca.create_token(ids(&["spiffe://c1/node/7"]), now)
            .await
            .unwrap_err();
    assert_eq!(err.reason(), "identity_revoked");

    // By serial: that leaf only.
    w.ca.revoke(Revoke::Serial(agentd.serial().to_vec()), now)
        .await
        .unwrap();
    let (_, csr) = new_key_and_csr(&ids(&["spiffe://c1/svc/agentd"]), &[]).unwrap();
    let err = w.ca.renew(&peer(&agentd), &csr, now).await.unwrap_err();
    assert_eq!(err.reason(), "identity_revoked");

    // Nodes refresh their copy of the list from the document.
    let list = DenyList::default();
    w.ca.refresh_deny_list(&list).await.unwrap();
    assert!(list.denies(agentd.serial(), agentd.ids()));
    assert!(list.denies(&[0xff], node.ids()));
    let status = w.ca.status(now).await.unwrap();
    assert_eq!(status.denied_identities, vec!["spiffe://c1/node/7"]);
    assert_eq!(status.denied_serials.len(), 1);
    // The revoked serial leaves the list once its leaf has expired.
    let day = now + Duration::from_secs(25 * 3600);
    w.ca.revoke(Revoke::Identity(id("spiffe://c1/svc/x")), day)
        .await
        .unwrap();
    let status = w.ca.status(day).await.unwrap();
    assert!(status.denied_serials.is_empty());
    assert_eq!(status.denied_identities.len(), 2);
}

#[tokio::test]
async fn trust_bundle_rotation_keeps_old_intermediate_30_days() {
    let now = SystemTime::now();
    let w = world(now).await;
    let t = token(&w.ca, &["spiffe://c1/node/1"], now).await;
    let before = join(&w.ca, &t, &ids(&["spiffe://c1/node/1"]), now)
        .await
        .unwrap();
    let old = w.ca.bundle(now).await.unwrap();
    assert_eq!(old.intermediates.len(), 1);

    // The wrong root key is refused and changes nothing.
    let other = Authority::new(
        Arc::new(MemoryDocStore::default()),
        Ok(KeySealer::generate()),
    );
    let other_root = other.init("c1", now).await.unwrap();
    let err =
        w.ca.rotate_intermediate(other_root.expose_pem(), now)
            .await
            .unwrap_err();
    assert_eq!(err.reason(), "invalid_argument");

    let rotated = now + Duration::from_secs(60);
    w.ca.rotate_intermediate(&w.root_key, rotated)
        .await
        .unwrap();
    let bundle = w.ca.bundle(rotated).await.unwrap();
    assert_eq!(bundle.roots, old.roots, "the root does not change");
    assert_eq!(bundle.intermediates.len(), 2);
    assert_eq!(bundle.intermediates[1], old.intermediates[0], "old kept");
    // New leaves come from the new intermediate, and verify against the
    // same root as leaves from the old one.
    let t = token(&w.ca, &["spiffe://c1/node/2"], rotated).await;
    let after = join(&w.ca, &t, &ids(&["spiffe://c1/node/2"]), rotated)
        .await
        .unwrap();
    assert_eq!(after.chain()[1], bundle.intermediates[0]);
    assert_ne!(after.chain()[1], before.chain()[1]);
    handshake(&after, &before).unwrap();
    handshake(&before, &after).unwrap();

    // 30 days on, the old intermediate has left the bundle; a day before, not.
    let almost = rotated + RETIRING_INTERMEDIATE_KEPT - Duration::from_secs(86_400);
    assert_eq!(w.ca.bundle(almost).await.unwrap().intermediates.len(), 2);
    let gone = rotated + RETIRING_INTERMEDIATE_KEPT + Duration::from_secs(1);
    assert_eq!(w.ca.bundle(gone).await.unwrap().intermediates.len(), 1);
    let status = w.ca.status(gone).await.unwrap();
    assert_eq!(status.intermediates.len(), 2, "pruned on the next write");
    w.ca.revoke(Revoke::Identity(id("spiffe://c1/svc/x")), gone)
        .await
        .unwrap();
    let status = w.ca.status(gone).await.unwrap();
    assert_eq!(status.intermediates.len(), 1);
    assert!(status.intermediates[0].active);
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[tokio::test]
async fn keys_never_written_to_disk() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "loams-ca-keys-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let key_file = dir.join("ca").join("sealing.key");
    let now = SystemTime::now();
    let docs = Arc::new(MemoryDocStore::default());
    let ca = Authority::new(docs.clone(), KeySealer::create(&key_file));
    let root = ca.init("c1", now).await.unwrap();
    assert_eq!(format!("{root:?}"), "RootKey(<redacted>)");
    let t = token(&ca, &["spiffe://c1/node/1"], now).await;
    let node = join(&ca, &t, &ids(&["spiffe://c1/node/1"]), now)
        .await
        .unwrap();
    let (_, csr) = new_key_and_csr(&ids(&["spiffe://c1/node/1"]), &[]).unwrap();
    ca.renew(&peer(&node), &csr, now).await.unwrap();

    // On disk: only the sealing key, an age identity and nothing else.
    let files = files_under(&dir);
    assert_eq!(files, vec![key_file.clone()]);
    let text = std::fs::read_to_string(&key_file).unwrap();
    assert!(text.starts_with("AGE-SECRET-KEY-"));
    assert!(!text.contains("PRIVATE KEY"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // In the document: no private key, no token secret.
    let doc = String::from_utf8(docs.raw().unwrap()).unwrap();
    assert!(!doc.contains("PRIVATE KEY"), "{doc}");
    assert!(!doc.contains(root.expose_pem()));
    let secret = t.rsplit('.').next().unwrap();
    assert!(!doc.contains(secret));
    assert!(doc.contains("\"key_sealed\":\"age:"));
    // Another node with the same key file opens it; one without cannot.
    let same = Authority::new(docs.clone(), KeySealer::open_file(&key_file));
    let t = token(&same, &["spiffe://c1/node/2"], now).await;
    join(&same, &t, &ids(&["spiffe://c1/node/2"]), now)
        .await
        .unwrap();
    let missing = Authority::new(docs.clone(), KeySealer::open_file(&dir.join("absent.key")));
    let t = token(&missing, &["spiffe://c1/node/3"], now).await;
    let err = join(&missing, &t, &ids(&["spiffe://c1/node/3"]), now)
        .await
        .unwrap_err();
    assert_eq!(err.reason(), "ca_not_initialized");
    // It still serves the public bundle.
    missing.bundle(now).await.unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn init_happens_once_and_nothing_answers_before_it() {
    let now = SystemTime::now();
    let ca = Authority::new(
        Arc::new(MemoryDocStore::default()),
        Ok(KeySealer::generate()),
    );
    let err = ca.bundle(now).await.unwrap_err();
    assert_eq!(err.reason(), "ca_not_initialized");
    let err = ca.issue("loamsjt1.x.y", b"", now).await.unwrap_err();
    assert_eq!(err.reason(), "invalid_argument", "a malformed token");
    let err = ca.init("Not A Domain!", now).await.unwrap_err();
    assert_eq!(err.reason(), "invalid_argument");
    ca.init("c1", now).await.unwrap();
    assert_eq!(
        ca.init("c1", now).await.unwrap_err(),
        CaServiceError::AlreadyInitialized
    );
}

/// The Connect face, in Connect's JSON: `Issue` with a token, `Renew` only
/// with the verified peer the mTLS layer would add, `TrustBundle` for anyone.
#[tokio::test]
async fn certificate_service_over_connect() {
    let now = SystemTime::now();
    let w = world(now).await;
    let router = loams_ca::connect::register(connectrpc::Router::new(), w.ca.clone());
    let paths: Vec<String> = router.methods().map(|p| format!("/{p}")).collect();
    for path in [
        loams_ca::connect::ISSUE_PATH,
        loams_ca::connect::RENEW_PATH,
        loams_ca::connect::TRUST_BUNDLE_PATH,
    ] {
        assert!(paths.iter().any(|p| p == path), "{path} in {paths:?}");
    }
    let service = connectrpc::ConnectRpcService::new(router);
    let call = |path: &str, body: serde_json::Value, peer: Option<VerifiedPeer>| {
        let mut request = Request::post(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        if let Some(peer) = peer {
            request.extensions_mut().insert(peer);
        }
        let service = service.clone();
        async move {
            let response = service.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(Body::new(response.into_body()), 1 << 20)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            )
        }
    };
    let ids_ = ids(&["spiffe://c1/node/1"]);
    let t = token(&w.ca, &["spiffe://c1/node/1"], now).await;
    let (key, csr) = new_key_and_csr(&ids_, &[]).unwrap();
    let (status, issued) = call(
        loams_ca::connect::ISSUE_PATH,
        serde_json::json!({"joinToken": t, "csr": STANDARD.encode(&csr)}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let chain: Vec<rustls_pki_types::CertificateDer<'static>> = issued["certificate"]["chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| STANDARD.decode(c.as_str().unwrap()).unwrap().into())
        .collect();
    assert_eq!(chain.len(), 2);
    assert!(issued["certificate"]["notAfter"].is_string());
    assert!(issued["certificate"]["renewAt"].is_string());
    let (status, bundle) = call(
        loams_ca::connect::TRUST_BUNDLE_PATH,
        serde_json::json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let roots: Vec<rustls_pki_types::CertificateDer<'static>> = bundle["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| STANDARD.decode(c.as_str().unwrap()).unwrap().into())
        .collect();
    let node = Identity::new(chain, key, roots).unwrap();

    // The token again: unauthenticated, reason join_token_spent.
    let (status, err) = call(
        loams_ca::connect::ISSUE_PATH,
        serde_json::json!({"joinToken": t, "csr": STANDARD.encode(&csr)}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(err["code"], "unauthenticated");
    assert!(
        err["message"].as_str().unwrap().contains("already used"),
        "{err}"
    );

    // Renew without a verified peer: unauthenticated; with one: a leaf.
    let (_, csr) = new_key_and_csr(&ids_, &[]).unwrap();
    let body = serde_json::json!({"csr": STANDARD.encode(&csr)});
    let (status, _) = call(loams_ca::connect::RENEW_PATH, body.clone(), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, renewed) = call(loams_ca::connect::RENEW_PATH, body, Some(peer(&node))).await;
    assert_eq!(status, StatusCode::OK, "{renewed}");
    assert_eq!(renewed["certificate"]["chain"].as_array().unwrap().len(), 2);
}
