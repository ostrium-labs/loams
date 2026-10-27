//! An in-process gateway on a loopback port: `MemStore`, the fake Resonate,
//! and a dev authenticator.
#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use loams_temporal_frontend::{DevAllowAll, FrontendConfig, Gateway, Parts, Principal, TokenKeys};
use loams_temporal_proto::temporal::api::workflowservice::v1::RegisterNamespaceRequest;
use loams_temporal_proto::temporal::api::workflowservice::v1::workflow_service_client::WorkflowServiceClient;
use loams_temporal_resonate::FakeDurability;
use loams_temporal_store::MemStore;
use tonic::transport::Channel;

pub struct TestGateway {
    pub client: WorkflowServiceClient<Channel>,
    pub channel: Channel,
    pub fake: Arc<FakeDurability>,
    pub gateway: Gateway,
    _stop: tokio::sync::watch::Sender<bool>,
}

pub async fn start() -> TestGateway {
    start_with(|c| c).await
}

/// A gateway whose config the test adjusts (a short history wait, say).
pub async fn start_with(adjust: impl FnOnce(FrontendConfig) -> FrontendConfig) -> TestGateway {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();
    let fake = Arc::new(FakeDurability::new());
    let (stop, rx) = tokio::sync::watch::channel(false);
    let gateway = Gateway::start(
        Parts {
            store: Arc::new(MemStore::new(64).expect("store")),
            durability: fake.clone(),
            auth: Arc::new(DevAllowAll(Principal {
                loams_namespace_id: "lns".into(),
                admin: true,
            })),
            tokens: TokenKeys::new(vec![("k1".into(), vec![7; 32])]),
            pid: "gw-test".into(),
            shards: 64,
            lease: Duration::from_secs(60),
            config: adjust(FrontendConfig {
                poll_timeout: Duration::from_secs(3),
                query_timeout: Duration::from_secs(5),
                history_wait: Duration::from_secs(5),
                ..Default::default()
            }),
        },
        rx,
    )
    .await
    .expect("gateway");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let svc = gateway.service();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .expect("serve");
    });
    let channel = Channel::from_shared(format!("http://{addr}"))
        .expect("uri")
        .connect()
        .await
        .expect("connect");
    let mut client = WorkflowServiceClient::new(channel.clone());
    client
        .register_namespace(RegisterNamespaceRequest {
            namespace: "default".into(),
            ..Default::default()
        })
        .await
        .expect("register default");
    TestGateway {
        client,
        channel,
        fake,
        gateway,
        _stop: stop,
    }
}
