//! Storage broker publication and discovery (PG2 Task 32), against a
//! stand-in broker built from the vendored proto's server side.
#![cfg(feature = "server")]
#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use loams_safekeeper::broker::proto::broker_service_server::{BrokerService, BrokerServiceServer};
use loams_safekeeper::broker::proto::{
    MessageType, SafekeeperDiscoveryRequest, SafekeeperTimelineInfo, SubscribeByFilterRequest,
    SubscribeSafekeeperInfoRequest, TenantTimelineId as ProtoTtid, TypedMessage,
};
use loams_safekeeper::broker::{self, BrokerConfig};
use loams_safekeeper::propose::{WalRange, push_committed};
use loams_safekeeper::service::{WalService, WalServiceConfig};
use loams_safekeeper::store::{MemWalStore, WalStore};
use loams_safekeeper::types::{Id, Lsn, TimelineId};
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;
type Published = Arc<Mutex<Vec<(Instant, SafekeeperTimelineInfo)>>>;
type Subscribers = Arc<Mutex<Vec<mpsc::Sender<Result<TypedMessage, Status>>>>>;

/// What the stand-in broker saw, and the discovery requests it hands out.
#[derive(Clone, Default)]
struct Fake {
    published: Published,
    published_one: Arc<Mutex<Vec<TypedMessage>>>,
    subscribers: Subscribers,
}

#[tonic::async_trait]
impl BrokerService for Fake {
    type SubscribeSafekeeperInfoStream = BoxStream<SafekeeperTimelineInfo>;
    type SubscribeByFilterStream = BoxStream<TypedMessage>;

    async fn subscribe_safekeeper_info(
        &self,
        _: Request<SubscribeSafekeeperInfoRequest>,
    ) -> Result<Response<Self::SubscribeSafekeeperInfoStream>, Status> {
        Err(Status::unimplemented("not used by loams-wal"))
    }

    async fn publish_safekeeper_info(
        &self,
        req: Request<Streaming<SafekeeperTimelineInfo>>,
    ) -> Result<Response<()>, Status> {
        let mut s = req.into_inner();
        while let Some(info) = s.message().await? {
            self.published.lock().unwrap().push((Instant::now(), info));
        }
        Ok(Response::new(()))
    }

    async fn subscribe_by_filter(
        &self,
        req: Request<SubscribeByFilterRequest>,
    ) -> Result<Response<Self::SubscribeByFilterStream>, Status> {
        let r = req.into_inner();
        assert!(
            r.types
                .iter()
                .any(|t| t.r#type == MessageType::SafekeeperDiscoveryRequest as i32)
        );
        let (tx, rx) = mpsc::channel(16);
        self.subscribers.lock().unwrap().push(tx);
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn publish_one(&self, req: Request<TypedMessage>) -> Result<Response<()>, Status> {
        self.published_one.lock().unwrap().push(req.into_inner());
        Ok(Response::new(()))
    }
}

async fn fake_broker() -> (Fake, SocketAddr) {
    let fake = Fake::default();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let svc = BrokerServiceServer::new(fake.clone());
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(l)),
    );
    (fake, addr)
}

async fn wal_service() -> (Arc<WalService<MemWalStore>>, String) {
    let svc = WalService::new(
        Arc::new(MemWalStore::new()),
        WalServiceConfig {
            poll_interval: Duration::from_millis(5),
            ..Default::default()
        },
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(svc.clone().serve(l, std::future::pending()));
    (svc, addr)
}

fn timeline(n: u8) -> TimelineId {
    TimelineId::new(Id([n; 16]), Id([n.wrapping_add(100); 16]))
}

fn range(len: usize) -> WalRange {
    WalRange {
        start: Lsn(0x0149_6F10),
        pg_version: 170_005,
        system_id: 7,
        wal: Bytes::from(vec![9u8; len]),
    }
}

fn config(broker: SocketAddr, pg: &str) -> BrokerConfig {
    let mut cfg = BrokerConfig::new(
        format!("http://{broker}"),
        42,
        pg.to_string(),
        "127.0.0.1:7676".into(),
    );
    cfg.availability_zone = Some("az-1".into());
    cfg.retry = Duration::from_millis(100);
    cfg
}

fn ttid(tl: TimelineId) -> ProtoTtid {
    ProtoTtid {
        tenant_id: tl.tenant.0.to_vec(),
        timeline_id: tl.timeline.0.to_vec(),
    }
}

/// A served timeline is published about once a second, with the fields the
/// pageserver and the control plane read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publishes_every_second() {
    let (fake, baddr) = fake_broker().await;
    let (svc, pg) = wal_service().await;
    let tl = timeline(1);
    let r = range(5000);
    let _proposer = push_committed(&pg, tl, &r).await.unwrap();
    // A timeline whose pageserver has caught up and that no proposer writes
    // to is not published.
    let idle = timeline(2);
    drop(push_committed(&pg, idle, &r).await.unwrap());
    tokio::time::sleep(Duration::from_millis(300)).await;
    svc.store()
        .record_remote_consistent_lsn(&idle, r.end())
        .await
        .unwrap();

    let _tasks = broker::spawn(svc.clone(), config(baddr, &pg));
    tokio::time::sleep(Duration::from_millis(3500)).await;

    let seen = fake.published.lock().unwrap().clone();
    let mine: Vec<_> = seen
        .iter()
        .filter(|(_, i)| i.tenant_timeline_id == Some(ttid(tl)))
        .collect();
    assert!((3..=5).contains(&mine.len()), "{} publications", mine.len());
    for w in mine.windows(2) {
        let gap = w[1].0 - w[0].0;
        assert!(
            gap >= Duration::from_millis(800) && gap <= Duration::from_millis(1500),
            "{gap:?}"
        );
    }
    let info = &mine.last().unwrap().1;
    assert_eq!(info.safekeeper_id, 42);
    assert_eq!(info.safekeeper_connstr, pg);
    assert_eq!(info.http_connstr, "127.0.0.1:7676");
    assert_eq!(info.availability_zone.as_deref(), Some("az-1"));
    assert_eq!(info.term, 1);
    assert_eq!(info.last_log_term, 1);
    assert_eq!(Lsn(info.flush_lsn), r.end());
    assert_eq!(Lsn(info.commit_lsn), r.end());
    assert_eq!(Lsn(info.remote_consistent_lsn), r.start);
    assert_eq!(Lsn(info.backup_lsn), r.start);
    assert!(
        !seen
            .iter()
            .any(|(_, i)| i.tenant_timeline_id == Some(ttid(idle))),
        "a caught-up, idle timeline is not published"
    );
}

/// Discovery requests are answered for timelines the store holds, and
/// only for those; the timeline is published from then on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_answers_for_known_timeline() {
    let (fake, baddr) = fake_broker().await;
    // A pool: the timeline was written through another instance over the
    // same store, so this one learns it only from the request.
    let store = Arc::new(MemWalStore::new());
    let writer = WalService::new(store.clone(), WalServiceConfig::default());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let waddr = l.local_addr().unwrap().to_string();
    tokio::spawn(writer.serve(l, std::future::pending()));
    let tl = timeline(3);
    let r = range(3000);
    drop(push_committed(&waddr, tl, &r).await.unwrap());

    let reader = WalService::new(store, WalServiceConfig::default());
    assert!(reader.known_timelines().is_empty());
    let _tasks = broker::spawn(reader.clone(), config(baddr, "10.0.0.9:5454"));
    // Wait for the subscription.
    for _ in 0..100 {
        if !fake.subscribers.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let sub = fake.subscribers.lock().unwrap()[0].clone();
    for t in [timeline(99), tl] {
        sub.send(Ok(TypedMessage {
            r#type: MessageType::SafekeeperDiscoveryRequest as i32,
            safekeeper_timeline_info: None,
            safekeeper_discovery_request: Some(SafekeeperDiscoveryRequest {
                tenant_timeline_id: Some(ttid(t)),
            }),
            safekeeper_discovery_response: None,
        }))
        .await
        .unwrap();
    }
    for _ in 0..100 {
        if !fake.published_one.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let answers = fake.published_one.lock().unwrap().clone();
    assert_eq!(answers.len(), 1, "{answers:?}");
    assert_eq!(
        answers[0].r#type,
        MessageType::SafekeeperDiscoveryResponse as i32
    );
    let resp = answers[0].safekeeper_discovery_response.clone().unwrap();
    assert_eq!(resp.tenant_timeline_id, Some(ttid(tl)));
    assert_eq!(resp.safekeeper_id, 42);
    assert_eq!(resp.safekeeper_connstr, "10.0.0.9:5454");
    assert_eq!(Lsn(resp.commit_lsn), r.end());
    assert_eq!(resp.availability_zone.as_deref(), Some("az-1"));
    // From now on it is published (its pageserver has WAL to ingest).
    assert_eq!(reader.known_timelines(), vec![tl]);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        fake.published
            .lock()
            .unwrap()
            .iter()
            .any(|(_, i)| i.tenant_timeline_id == Some(ttid(tl)))
    );
}
