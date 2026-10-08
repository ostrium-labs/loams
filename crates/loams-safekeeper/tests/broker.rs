//! Storage broker publication and discovery (PG2 Task 32), against a
//! stand-in broker built from the vendored proto's server side.
#![cfg(feature = "server")]
#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use loams_safekeeper::Error;
use loams_safekeeper::broker::proto::broker_service_server::{BrokerService, BrokerServiceServer};
use loams_safekeeper::broker::proto::{
    MessageType, SafekeeperDiscoveryRequest, SafekeeperTimelineInfo, SubscribeByFilterRequest,
    SubscribeSafekeeperInfoRequest, TenantTimelineId as ProtoTtid, TypedMessage,
};
use loams_safekeeper::broker::{self, BrokerConfig};
use loams_safekeeper::propose::{WalRange, push_committed};
use loams_safekeeper::proto::ProposerElected;
use loams_safekeeper::service::{WalService, WalServiceConfig};
use loams_safekeeper::store::{AppendBatch, Deposed, MemWalStore, WalStore};
use loams_safekeeper::types::{
    AcceptorState, Configuration, Id, Lsn, ServerInfo, Term, TimelineId,
};
use tokio::sync::{mpsc, oneshot};
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
    /// A broker going down: open streams end, as its connections would.
    down: Arc<AtomicBool>,
}

impl Fake {
    fn infos_for(&self, tl: TimelineId) -> Vec<(Instant, SafekeeperTimelineInfo)> {
        self.published
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, i)| i.tenant_timeline_id == Some(ttid(tl)))
            .cloned()
            .collect()
    }

    fn last_seen(&self, tl: TimelineId) -> Option<Instant> {
        self.infos_for(tl).last().map(|(t, _)| *t)
    }

    /// End every open stream (the client sees its connection fail).
    fn go_down(&self) {
        self.down.store(true, Ordering::Relaxed);
        self.subscribers.lock().unwrap().clear();
    }
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
            if self.down.load(Ordering::Relaxed) {
                return Err(Status::unavailable("the broker is going down"));
            }
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

/// A stand-in broker on `addr` (port 0: any), until `stop` fires.
async fn fake_broker_on(
    fake: Fake,
    addr: SocketAddr,
) -> (SocketAddr, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let l = tokio::net::TcpListener::bind(addr).await.unwrap();
    let addr = l.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(BrokerServiceServer::new(fake))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(l),
                async {
                    let _ = stopped.await;
                },
            )
            .await;
    });
    (addr, stop, task)
}

async fn fake_broker() -> (Fake, SocketAddr) {
    let fake = Fake::default();
    let (addr, stop, _) = fake_broker_on(fake.clone(), "127.0.0.1:0".parse().unwrap()).await;
    std::mem::forget(stop); // runs until the test ends
    (fake, addr)
}

async fn wal_service_on<S: WalStore>(store: Arc<S>) -> (Arc<WalService<S>>, String) {
    let svc = WalService::new(
        store,
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

async fn wal_service() -> (Arc<WalService<MemWalStore>>, String) {
    wal_service_on(Arc::new(MemWalStore::new())).await
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

/// Wait until `f` holds, for at most `secs`.
async fn eventually(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    f()
}

/// A served timeline is published about once a second, with the fields the
/// pageserver and the control plane read; an idle timeline whose pageserver
/// has caught up is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publishes_every_second() {
    let (fake, baddr) = fake_broker().await;
    let (svc, pg) = wal_service().await;
    let tl = timeline(1);
    let r = range(5000);
    let _proposer = push_committed(&pg, tl, &r).await.unwrap();
    let idle = timeline(2);
    drop(push_committed(&pg, idle, &r).await.unwrap());
    tokio::time::sleep(Duration::from_millis(300)).await;
    svc.store()
        .record_remote_consistent_lsn(&idle, r.end())
        .await
        .unwrap();

    let _tasks = broker::spawn(svc.clone(), config(baddr, &pg));
    tokio::time::sleep(Duration::from_millis(3500)).await;

    let mine = fake.infos_for(tl);
    assert!((2..=6).contains(&mine.len()), "{} publications", mine.len());
    for w in mine.windows(2) {
        let gap = w[1].0 - w[0].0;
        assert!(
            gap >= Duration::from_millis(500) && gap <= Duration::from_millis(2000),
            "{gap:?}"
        );
    }
    let head = svc.store().load(&tl).await.unwrap().unwrap();
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
    assert_eq!(Lsn(info.local_start_lsn), r.start);
    assert_eq!(Lsn(info.local_start_lsn), head.local_start_lsn);
    assert_eq!(Lsn(info.peer_horizon_lsn), head.peer_horizon_lsn);
    assert!(
        fake.infos_for(idle).is_empty(),
        "an idle, caught-up timeline"
    );
}

/// Active to inactive: once the proposer leaves and the pageserver has
/// caught up, the timeline stops being published at once, and it leaves
/// the published set after the staleness timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_inactive_timeline_stops_being_published_and_the_set_shrinks() {
    let (fake, baddr) = fake_broker().await;
    let (svc, pg) = wal_service().await;
    let tl = timeline(4);
    let r = range(2000);
    let proposer = push_committed(&pg, tl, &r).await.unwrap();
    let mut cfg = config(baddr, &pg);
    cfg.interval = Duration::from_millis(200);
    cfg.staleness = Duration::from_secs(2);
    let tasks = broker::spawn(svc.clone(), cfg);
    assert!(eventually(5, || fake.infos_for(tl).len() >= 2).await);
    assert_eq!(tasks.published.timelines(), vec![tl]);

    drop(proposer);
    svc.store()
        .record_remote_consistent_lsn(&tl, r.end())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let stopped_at = Instant::now();
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        fake.last_seen(tl).is_none_or(|t| t < stopped_at),
        "still published after it went inactive"
    );
    assert!(
        eventually(5, || tasks.published.timelines().is_empty()).await,
        "the set did not shrink"
    );
}

/// An acceptor no pageserver streams from (Arm A) never learns the
/// pageserver's progress, so its `remote_consistent_lsn` stays behind; it
/// still stops publishing after the staleness timeout (ruling R32.5).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lagging_timeline_nobody_reads_goes_stale() {
    let (fake, baddr) = fake_broker().await;
    let (svc, pg) = wal_service().await;
    let tl = timeline(5);
    let proposer = push_committed(&pg, tl, &range(2000)).await.unwrap();
    let mut cfg = config(baddr, &pg);
    cfg.interval = Duration::from_millis(200);
    cfg.staleness = Duration::from_secs(2);
    let tasks = broker::spawn(svc.clone(), cfg);
    assert!(eventually(5, || !fake.infos_for(tl).is_empty()).await);
    let left = Instant::now();
    drop(proposer);
    // Still lagging: published for a while...
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert!(
        fake.last_seen(tl)
            .is_some_and(|t| t > left + Duration::from_millis(300))
    );
    // ...then not, and gone from the set.
    assert!(eventually(6, || tasks.published.timelines().is_empty()).await);
    let gone = Instant::now();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(fake.last_seen(tl).is_some_and(|t| t <= gone));
}

/// Discovery requests are answered for timelines the store holds, and
/// only for those; the timeline is published while it is asked about.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discovery_answers_for_known_timeline() {
    let (fake, baddr) = fake_broker().await;
    // A pool: the timeline was written through another instance over the
    // same store, so this one learns it only from the request.
    let store = Arc::new(MemWalStore::new());
    let (_writer, waddr) = wal_service_on(store.clone()).await;
    let tl = timeline(3);
    let r = range(3000);
    drop(push_committed(&waddr, tl, &r).await.unwrap());

    let (reader, _) = wal_service_on(store).await;
    let mut cfg = config(baddr, "10.0.0.9:5454");
    cfg.interval = Duration::from_millis(200);
    cfg.discovery_window = Duration::from_secs(1);
    cfg.staleness = Duration::from_secs(1);
    let tasks = broker::spawn(reader.clone(), cfg);
    assert!(eventually(5, || !fake.subscribers.lock().unwrap().is_empty()).await);
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
    assert!(eventually(5, || !fake.published_one.lock().unwrap().is_empty()).await);
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
    // Published while asked about...
    assert!(eventually(3, || !fake.infos_for(tl).is_empty()).await);
    assert_eq!(tasks.published.timelines(), vec![tl]);
    // ...and not after, when no pageserver came to read it.
    assert!(eventually(5, || tasks.published.timelines().is_empty()).await);
    let gone = Instant::now();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(fake.last_seen(tl).is_some_and(|t| t <= gone));
}

/// When the broker restarts, publication resumes on the new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publication_resumes_after_a_broker_restart() {
    let first = Fake::default();
    let (baddr, stop, task) = fake_broker_on(first.clone(), "127.0.0.1:0".parse().unwrap()).await;
    let (svc, pg) = wal_service().await;
    let tl = timeline(6);
    let _proposer = push_committed(&pg, tl, &range(1000)).await.unwrap();
    let mut cfg = config(baddr, &pg);
    cfg.interval = Duration::from_millis(200);
    let _tasks = broker::spawn(svc.clone(), cfg);
    assert!(eventually(5, || !first.infos_for(tl).is_empty()).await);

    first.go_down();
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("the first broker stops")
        .unwrap();
    let second = Fake::default();
    let (_, _stop2, _) = fake_broker_on(second.clone(), baddr).await;
    assert!(
        eventually(10, || !second.infos_for(tl).is_empty()).await,
        "no publication after the broker restarted"
    );
}

/// A store whose head reads can be made slow.
#[derive(Debug, Default)]
struct SlowLoads {
    inner: MemWalStore,
    slow: AtomicBool,
}

#[async_trait::async_trait]
impl WalStore for SlowLoads {
    async fn load(&self, tl: &TimelineId) -> Result<Option<AcceptorState>, Error> {
        if self.slow.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        self.inner.load(tl).await
    }
    async fn create(&self, tl: &TimelineId, s: ServerInfo, l: Lsn) -> Result<AcceptorState, Error> {
        self.inner.create(tl, s, l).await
    }
    async fn update_meta(
        &self,
        tl: &TimelineId,
        s: ServerInfo,
        m: Option<Configuration>,
    ) -> Result<AcceptorState, Error> {
        self.inner.update_meta(tl, s, m).await
    }
    async fn vote(&self, tl: &TimelineId, term: Term) -> Result<(bool, AcceptorState), Error> {
        self.inner.vote(tl, term).await
    }
    async fn elected(
        &self,
        tl: &TimelineId,
        msg: &ProposerElected,
    ) -> Result<Result<AcceptorState, Deposed>, Error> {
        self.inner.elected(tl, msg).await
    }
    async fn append(
        &self,
        tl: &TimelineId,
        batch: &AppendBatch,
    ) -> Result<Result<AcceptorState, Deposed>, Error> {
        self.inner.append(tl, batch).await
    }
    async fn record_commit_lsn(
        &self,
        tl: &TimelineId,
        term: Term,
        c: Lsn,
    ) -> Result<Result<(), Deposed>, Error> {
        self.inner.record_commit_lsn(tl, term, c).await
    }
    async fn record_backup_lsn(&self, tl: &TimelineId, l: Lsn) -> Result<(), Error> {
        self.inner.record_backup_lsn(tl, l).await
    }
    async fn record_remote_consistent_lsn(&self, tl: &TimelineId, l: Lsn) -> Result<(), Error> {
        self.inner.record_remote_consistent_lsn(tl, l).await
    }
    async fn read(&self, tl: &TimelineId, f: Lsn, m: usize) -> Result<Vec<(Lsn, Bytes)>, Error> {
        self.inner.read(tl, f, m).await
    }
    async fn trim(&self, tl: &TimelineId, l: Lsn) -> Result<Lsn, Error> {
        self.inner.trim(tl, l).await
    }
}

/// With slow head reads, a round reads several heads at once within its
/// budget, and rounds that run out of time pick up where they stopped:
/// every timeline is still published, with no round over its budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_head_reads_stay_within_the_round_budget() {
    let (fake, baddr) = fake_broker().await;
    let store = Arc::new(SlowLoads::default());
    let (svc, pg) = wal_service_on(store.clone()).await;
    let mut proposers = Vec::new();
    let tls: Vec<TimelineId> = (10..30).map(timeline).collect();
    for tl in &tls {
        proposers.push(push_committed(&pg, *tl, &range(500)).await.unwrap());
    }
    store.slow.store(true, Ordering::Relaxed);
    let mut cfg = config(baddr, &pg);
    cfg.interval = Duration::from_millis(500);
    cfg.round_budget = Duration::from_millis(400);
    cfg.concurrency = 4;
    // 20 reads of 300 ms: 6 s one at a time; 4 at a time within 400 ms
    // covers 4 a round, and rotation reaches all 20 in 5 rounds.
    let started = Instant::now();
    let _tasks = broker::spawn(svc.clone(), cfg);
    assert!(
        eventually(10, || tls.iter().all(|tl| !fake.infos_for(*tl).is_empty())).await,
        "not every timeline was published"
    );
    assert!(started.elapsed() < Duration::from_secs(8));
}
