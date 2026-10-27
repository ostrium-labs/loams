//! The Spark Connect proxy against a fake Spark Connect backend (SP-a plan
//! task 5): auth, tenant routing, identity pinning, credential stripping,
//! streaming in both directions, session limits and the idle sweeper.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::Stream;
use loams_authz::SubjectKind;
use loams_spark::auth::{KeyEntry, KeyTable};
use loams_spark::proto::connect::{
    self,
    spark_connect_service_client::SparkConnectServiceClient,
    spark_connect_service_server::{SparkConnectService, SparkConnectServiceServer},
};
use loams_spark::proxy::{AccessCheck, NoLifecycle, ProxySettings, SparkConnectProxy};
use loams_spark::route::{self, Backend, HostPattern, StaticRouter};
use loams_spark::sessions::Limits;
use loams_spark::standalone::KeyGrants;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, Endpoint, Server};
use tonic::{Code, Request, Response, Status, Streaming};

const KEY_A: &str = "lk_01JAAAAAAAAAAAAAAAAAAAAAAA_abcdefghijklmnopqrstuvwxyz012345";
const KEY_B: &str = "lk_01JBBBBBBBBBBBBBBBBBBBBBBB_abcdefghijklmnopqrstuvwxyz012345";
const BACKEND_TOKEN: &str = "backend-preshared-token";
const S1: &str = "8b0b7a8e-6d4c-4f7e-9f63-1c2d3e4f5a6b";
const S2: &str = "11111111-2222-4333-8444-555555555555";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Seen {
    method: &'static str,
    session_id: String,
    user_id: String,
    user_name: String,
    authorization: Option<String>,
    api_key_header: bool,
}

#[derive(Default, Clone)]
struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    fn record<T>(
        &self,
        method: &'static str,
        req: &Request<T>,
        session_id: &str,
        ctx: Option<&connect::UserContext>,
    ) {
        self.seen.lock().unwrap().push(Seen {
            method,
            session_id: session_id.to_owned(),
            user_id: ctx.map(|c| c.user_id.clone()).unwrap_or_default(),
            user_name: ctx.map(|c| c.user_name.clone()).unwrap_or_default(),
            authorization: req
                .metadata()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
            api_key_header: req.metadata().get("x-loams-api-key").is_some(),
        });
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

// With `generate_default_stubs`, tonic-build gives server-streaming methods
// a `BoxStream` return type instead of an associated stream type; this is
// the same type.
type ExecStream = Pin<Box<dyn Stream<Item = Result<connect::ExecutePlanResponse, Status>> + Send>>;

#[tonic::async_trait]
impl SparkConnectService for Fake {
    async fn config(
        &self,
        req: Request<connect::ConfigRequest>,
    ) -> Result<Response<connect::ConfigResponse>, Status> {
        let r = req.get_ref();
        self.record("Config", &req, &r.session_id, r.user_context.as_ref());
        Ok(Response::new(connect::ConfigResponse {
            session_id: r.session_id.clone(),
            server_side_session_id: "server-1".into(),
            ..Default::default()
        }))
    }

    async fn execute_plan(
        &self,
        req: Request<connect::ExecutePlanRequest>,
    ) -> Result<Response<ExecStream>, Status> {
        let r = req.get_ref();
        self.record("ExecutePlan", &req, &r.session_id, r.user_context.as_ref());
        let sid = r.session_id.clone();
        let answers = (0..3).map(move |i| {
            Ok(connect::ExecutePlanResponse {
                session_id: sid.clone(),
                response_id: format!("r{i}"),
                ..Default::default()
            })
        });
        Ok(Response::new(Box::pin(futures::stream::iter(answers))))
    }

    async fn add_artifacts(
        &self,
        req: Request<Streaming<connect::AddArtifactsRequest>>,
    ) -> Result<Response<connect::AddArtifactsResponse>, Status> {
        let (md, _ext, mut stream) = req.into_parts();
        let mut sid = String::new();
        while let Some(chunk) = stream.message().await? {
            let probe = Request::from_parts(md.clone(), tonic::Extensions::default(), ());
            self.record(
                "AddArtifacts",
                &probe,
                &chunk.session_id,
                chunk.user_context.as_ref(),
            );
            sid = chunk.session_id;
        }
        Ok(Response::new(connect::AddArtifactsResponse {
            session_id: sid,
            ..Default::default()
        }))
    }

    async fn release_session(
        &self,
        req: Request<connect::ReleaseSessionRequest>,
    ) -> Result<Response<connect::ReleaseSessionResponse>, Status> {
        let r = req.get_ref();
        self.record(
            "ReleaseSession",
            &req,
            &r.session_id,
            r.user_context.as_ref(),
        );
        Ok(Response::new(connect::ReleaseSessionResponse {
            session_id: r.session_id.clone(),
            ..Default::default()
        }))
    }
}

async fn serve_backend(fake: Fake) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(
        Server::builder()
            .add_service(SparkConnectServiceServer::new(fake))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    addr
}

fn keys() -> KeyTable {
    let entry = |key: &str, ns: &str, subject: &str| KeyEntry {
        id: loams_spark::auth::key_id(key).unwrap().to_owned(),
        sha256: KeyTable::hash(key),
        namespace: ns.into(),
        subject: subject.into(),
        kind: SubjectKind::ServiceAccount,
    };
    KeyTable::new([entry(KEY_A, "ns-a", "ci"), entry(KEY_B, "ns-b", "other")]).unwrap()
}

async fn start(limits: Limits, host_suffix: &str) -> (Fake, SparkConnectProxy, SocketAddr) {
    let fake = Fake::default();
    let backend_addr = serve_backend(fake.clone()).await;
    let backend = Backend {
        instance_id: "eng_a".into(),
        channel: route::channel(&format!("http://{backend_addr}"), Duration::from_secs(2)).unwrap(),
        token: Some(Arc::from(BACKEND_TOKEN)),
    };
    let proxy = SparkConnectProxy::new(
        Arc::new(keys()),
        Arc::new(KeyGrants),
        Arc::new(StaticRouter::single("ns-a", "etl", backend)),
        Arc::new(NoLifecycle),
        ProxySettings {
            hosts: HostPattern {
                suffix: host_suffix.into(),
            },
            limits,
            access: AccessCheck::default(),
            ..ProxySettings::default()
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(
        Server::builder()
            .add_service(proxy.clone())
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    (fake, proxy, addr)
}

async fn client(addr: SocketAddr) -> SparkConnectServiceClient<Channel> {
    let channel = Endpoint::from_shared(format!("http://{addr}"))
        .unwrap()
        .connect()
        .await
        .unwrap();
    SparkConnectServiceClient::new(channel)
}

fn with_key<T>(msg: T, key: &str) -> Request<T> {
    let mut r = Request::new(msg);
    r.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from(format!("Bearer {key}")).unwrap(),
    );
    r
}

fn config(session: &str) -> connect::ConfigRequest {
    connect::ConfigRequest {
        session_id: session.into(),
        user_context: Some(connect::UserContext {
            user_id: "root".into(),
            user_name: "alice".into(),
            extensions: Vec::new(),
        }),
        ..Default::default()
    }
}

fn limits() -> Limits {
    Limits {
        max_sessions: 1,
        max_operations: 8,
        idle_timeout: Duration::from_secs(600),
        instance_idle_timeout: Duration::from_secs(600),
    }
}

#[tokio::test]
async fn refuses_calls_without_a_valid_key() {
    let (fake, _p, addr) = start(limits(), "").await;
    let mut c = client(addr).await;
    let err = c.config(Request::new(config(S1))).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    let bad = "lk_01JAAAAAAAAAAAAAAAAAAAAAAA_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
    let err = c.config(with_key(config(S1), bad)).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated);
    assert!(fake.seen().is_empty(), "nothing reached the backend");
}

#[tokio::test]
async fn forwards_with_pinned_identity_and_backend_token() {
    let (fake, _p, addr) = start(limits(), "").await;
    let mut c = client(addr).await;
    let resp = c.config(with_key(config(S1), KEY_A)).await.unwrap();
    assert_eq!(resp.get_ref().session_id, S1);
    assert_eq!(resp.get_ref().server_side_session_id, "server-1");
    let seen = fake.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].user_id, "service_account:ci",
        "client's user_id replaced"
    );
    assert_eq!(seen[0].user_name, "alice", "other user_context fields kept");
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some(format!("Bearer {BACKEND_TOKEN}").as_str()),
        "the user's key never reaches the backend"
    );
    assert!(!seen[0].api_key_header);
}

#[tokio::test]
async fn accepts_the_key_as_an_sc_uri_parameter_header() {
    let (fake, _p, addr) = start(limits(), "").await;
    let mut c = client(addr).await;
    let mut r = Request::new(config(S1));
    r.metadata_mut()
        .insert("x-loams-api-key", MetadataValue::try_from(KEY_A).unwrap());
    c.config(r).await.unwrap();
    let seen = fake.seen();
    assert!(!seen[0].api_key_header, "stripped before forwarding");
}

#[tokio::test]
async fn namespace_without_an_instance_is_not_found() {
    let (fake, _p, addr) = start(limits(), "").await;
    let mut c = client(addr).await;
    let err = c.config(with_key(config(S1), KEY_B)).await.unwrap_err();
    assert_eq!(err.code(), Code::NotFound);
    assert!(fake.seen().is_empty());
}

#[tokio::test]
async fn hostname_of_another_namespace_is_refused() {
    let (fake, _p, addr) = start(limits(), "spark.loams.test").await;
    let channel = Endpoint::from_shared(format!("http://{addr}"))
        .unwrap()
        .origin("http://etl.ns-a.eu1.spark.loams.test:443".parse().unwrap())
        .connect()
        .await
        .unwrap();
    let mut c = SparkConnectServiceClient::new(channel);
    // ns-a's hostname with ns-b's key.
    let err = c.config(with_key(config(S1), KEY_B)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied);
    // ns-a's own key works.
    c.config(with_key(config(S1), KEY_A)).await.unwrap();
    assert_eq!(fake.seen().len(), 1);
}

#[tokio::test]
async fn server_streaming_execute_plan() {
    let (_fake, _p, addr) = start(limits(), "").await;
    let mut c = client(addr).await;
    let req = connect::ExecutePlanRequest {
        session_id: S1.into(),
        operation_id: Some("6f1d1c8e-0000-4000-8000-000000000001".into()),
        ..Default::default()
    };
    let mut stream = c
        .execute_plan(with_key(req, KEY_A))
        .await
        .unwrap()
        .into_inner();
    let mut ids = Vec::new();
    while let Some(r) = stream.message().await.unwrap() {
        ids.push(r.response_id);
    }
    assert_eq!(ids, ["r0", "r1", "r2"]);
}

#[tokio::test]
async fn client_streaming_add_artifacts_pins_every_chunk() {
    let (fake, _p, addr) = start(limits(), "").await;
    let mut c = client(addr).await;
    let chunks: Vec<connect::AddArtifactsRequest> = (0..3)
        .map(|_| connect::AddArtifactsRequest {
            session_id: S1.into(),
            user_context: Some(connect::UserContext {
                user_id: "spoof".into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .collect();
    let resp = c
        .add_artifacts(with_key(futures::stream::iter(chunks), KEY_A))
        .await
        .unwrap();
    assert_eq!(resp.get_ref().session_id, S1);
    let seen: Vec<_> = fake
        .seen()
        .into_iter()
        .filter(|s| s.method == "AddArtifacts")
        .collect();
    assert_eq!(seen.len(), 3);
    assert!(seen.iter().all(|s| s.user_id == "service_account:ci"));
}

#[tokio::test]
async fn session_limit_and_release() {
    let (fake, proxy, addr) = start(limits(), "").await;
    let mut c = client(addr).await;
    c.config(with_key(config(S1), KEY_A)).await.unwrap();
    let err = c.config(with_key(config(S2), KEY_A)).await.unwrap_err();
    assert_eq!(err.code(), Code::ResourceExhausted);
    assert_eq!(proxy.list_sessions("ns-a").len(), 1);

    let release = connect::ReleaseSessionRequest {
        session_id: S1.into(),
        ..Default::default()
    };
    c.release_session(with_key(release, KEY_A)).await.unwrap();
    assert!(proxy.list_sessions("ns-a").is_empty());
    c.config(with_key(config(S2), KEY_A)).await.unwrap();
    assert!(fake.seen().iter().any(|s| s.method == "ReleaseSession"));
}

#[tokio::test]
async fn invalid_session_ids_are_refused() {
    let (fake, _p, addr) = start(limits(), "").await;
    let mut c = client(addr).await;
    let err = c
        .config(with_key(config("not-a-uuid"), KEY_A))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument);
    assert!(fake.seen().is_empty());
}

#[tokio::test]
async fn sweeper_releases_idle_sessions_on_the_backend() {
    let mut l = limits();
    l.idle_timeout = Duration::ZERO;
    let (fake, proxy, addr) = start(l, "").await;
    let mut c = client(addr).await;
    c.config(with_key(config(S1), KEY_A)).await.unwrap();
    // The call ends when the server drops the answer stream, a moment after
    // the client has its answer.
    for _ in 0..100 {
        proxy.sweep().await;
        if proxy.list_sessions("ns-a").is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(proxy.list_sessions("ns-a").is_empty());
    let released: Vec<_> = fake
        .seen()
        .into_iter()
        .filter(|s| s.method == "ReleaseSession")
        .collect();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].session_id, S1);
    assert_eq!(released[0].user_id, "service_account:ci");
    assert_eq!(
        released[0].authorization.as_deref(),
        Some(format!("Bearer {BACKEND_TOKEN}").as_str())
    );
}

#[tokio::test]
async fn unimplemented_backend_methods_pass_status_through() {
    let (_fake, _p, addr) = start(limits(), "").await;
    let mut c = client(addr).await;
    let req = connect::AnalyzePlanRequest {
        session_id: S1.into(),
        ..Default::default()
    };
    let err = c.analyze_plan(with_key(req, KEY_A)).await.unwrap_err();
    assert_eq!(err.code(), Code::Unimplemented);
}
