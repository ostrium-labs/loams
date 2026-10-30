//! The PgDog config push (PG2 Task 21): send, wait for the generation,
//! `RELOAD`; the debounce; and the drift check. The sink and the admin
//! database are fakes; the records come from a local store.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use loams_pg_control::PgControlStore;
use loams_pg_control::model::{
    BranchRec, BranchState, Cu, EndpointRec, EndpointType, ProjectRec, ProjectState, WalService,
    project_lease,
};
use loams_pg_control::pgdog::push::{
    ConfigSink, DirSink, NoCredentials, PgDogPusher, PushError, SidecarSink,
};
use loams_pg_control::pgdog::render::{RenderSettings, RenderedConfig};
use loams_pg_control::pgdog::{InstanceId, PgDogAdmin, PgDogError, PoolRow};
use loams_pg_control::secrets::Secret;
use loams_pg_control::store::conformance::local_factory;
use loams_pg_control::store::{KvControlStore, StoreOptions};
use tokio::sync::mpsc;

const NS: &str = "acme";
const PID: &str = "prj-01JZ0000000000000000000001";
const BID: &str = "br-01JZ0000000000000000000001";

/// What happened, in order, across the sink and the admin database.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    Sent(String, u64),
    /// `RELOAD`, with the generation the replica's sink held at that moment.
    Reload(String, Option<u64>),
}

#[derive(Default)]
struct Shared {
    events: Vec<Event>,
    /// The generation each replica's sink holds.
    held: HashMap<String, u64>,
    /// A sent generation becomes visible only after this many polls.
    ack_after: usize,
    /// Generations sent but not yet visible, with the polls left.
    pending: HashMap<String, (u64, usize)>,
    /// Replicas whose sink never makes a sent generation visible.
    stuck: Vec<String>,
    /// Replicas whose `RELOAD` fails.
    reload_fails: Vec<String>,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<Shared>>);

impl Fake {
    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.0.lock().unwrap()
    }
    fn events(&self) -> Vec<Event> {
        self.lock().events.clone()
    }
    fn sends(&self) -> usize {
        self.events()
            .iter()
            .filter(|e| matches!(e, Event::Sent(..)))
            .count()
    }
}

#[async_trait]
impl ConfigSink for Fake {
    async fn send(
        &self,
        _namespace: &str,
        instance: &InstanceId,
        config: &RenderedConfig,
    ) -> Result<(), PushError> {
        let mut s = self.lock();
        s.events
            .push(Event::Sent(instance.0.clone(), config.generation));
        if s.stuck.contains(&instance.0) {
            return Ok(());
        }
        let polls = s.ack_after;
        if polls == 0 {
            s.held.insert(instance.0.clone(), config.generation);
        } else {
            s.pending
                .insert(instance.0.clone(), (config.generation, polls));
        }
        Ok(())
    }

    async fn generation(
        &self,
        _namespace: &str,
        instance: &InstanceId,
    ) -> Result<Option<u64>, PushError> {
        let mut s = self.lock();
        if let Some((generation, left)) = s.pending.get_mut(&instance.0) {
            if *left == 0 {
                let generation = *generation;
                s.pending.remove(&instance.0);
                s.held.insert(instance.0.clone(), generation);
            } else {
                *left -= 1;
            }
        }
        Ok(s.held.get(&instance.0).copied())
    }
}

/// Two replicas for [`NS`].
#[async_trait]
impl PgDogAdmin for Fake {
    async fn instances(&self, namespace: &str) -> Result<Vec<InstanceId>, PgDogError> {
        Ok(if namespace == NS {
            vec![InstanceId("pgdog-a".into()), InstanceId("pgdog-b".into())]
        } else {
            vec![InstanceId(format!("pgdog-{namespace}"))]
        })
    }
    async fn reload(&self, instance: &InstanceId) -> Result<(), PgDogError> {
        let mut s = self.lock();
        let held = s.held.get(&instance.0).copied();
        s.events.push(Event::Reload(instance.0.clone(), held));
        if s.reload_fails.contains(&instance.0) {
            return Err(PgDogError("reload refused".into()));
        }
        Ok(())
    }
    async fn show_pools(&self, _: &InstanceId) -> Result<Vec<PoolRow>, PgDogError> {
        Ok(Vec::new())
    }
    async fn show_databases(&self, _: &InstanceId) -> Result<Vec<String>, PgDogError> {
        Ok(Vec::new())
    }
}

fn project(id: &str, namespace: &str, name: &str, branch: &str) -> ProjectRec {
    ProjectRec {
        namespace: namespace.into(),
        id: id.into(),
        name: name.into(),
        tenant_id: [1; 16],
        pg_version: 17,
        wal: WalService::LoamsWal {
            pool: "default".into(),
        },
        history_retention_s: 86_400,
        region: "local".into(),
        default_branch_id: Some(branch.into()),
        settings: BTreeMap::new(),
        state: ProjectState::Ready,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn branch(project: &str, id: &str) -> BranchRec {
    BranchRec {
        project_id: project.into(),
        id: id.into(),
        name: "main".into(),
        timeline_id: [1; 16],
        parent_id: None,
        ancestor_lsn: None,
        expires_at_ms: None,
        protected: false,
        stripe_size: None,
        shards: Vec::new(),
        state: BranchState::Ready,
        created_at_ms: 1,
        parent_time_ms: None,
        updated_at_ms: 1,
        pg_version: 17,
    }
}

fn endpoint(project: &str, branch: &str, id: &str, addr: &str) -> EndpointRec {
    let mut e = EndpointRec::new(project, id, branch, EndpointType::ReadWrite);
    e.backend_addr = Some(addr.into());
    e.min_cu = Cu(1);
    e.max_cu = Cu(2);
    e
}

/// A local store holding one project of `namespace` with a branch and a
/// read-write endpoint.
async fn store_with(namespace: &str, pid: &str, bid: &str) -> KvControlStore {
    let store = local_factory(env!("CARGO_TARGET_TMPDIR"))
        .store(StoreOptions::default())
        .await
        .unwrap();
    seed(&store, namespace, pid, bid).await;
    store
}

async fn seed(store: &KvControlStore, namespace: &str, pid: &str, bid: &str) {
    let fence = store
        .acquire_lease(&project_lease(pid), "test", Duration::from_secs(60))
        .await
        .unwrap();
    store
        .put(&project(pid, namespace, "shop", bid), None, &fence)
        .await
        .unwrap();
    store.put(&branch(pid, bid), None, &fence).await.unwrap();
    store
        .put(
            &endpoint(pid, bid, "ep-01JZ0000000000000000000001", "10.0.0.1:5432"),
            None,
            &fence,
        )
        .await
        .unwrap();
}

/// Adds a second endpoint to the project, so the records render another
/// generation.
async fn add_endpoint(store: &KvControlStore) {
    let fence = store
        .acquire_lease(&project_lease(PID), "test", Duration::from_secs(60))
        .await
        .unwrap();
    let mut ro = endpoint(PID, BID, "ep-01JZ0000000000000000000002", "10.0.0.2:5432");
    ro.kind = EndpointType::ReadOnly;
    store.put(&ro, None, &fence).await.unwrap();
}

fn pusher(store: KvControlStore, fake: &Fake) -> PgDogPusher<KvControlStore> {
    let mut p = PgDogPusher::new(
        store,
        RenderSettings::new(Secret::new("admin-password".into())),
        Arc::new(NoCredentials),
        Arc::new(fake.clone()),
        Arc::new(fake.clone()),
    );
    p.ack_timeout = Duration::from_millis(500);
    p.ack_poll = Duration::from_millis(10);
    p.debounce = Duration::from_millis(200);
    p
}

fn a() -> InstanceId {
    InstanceId("pgdog-a".into())
}
fn b() -> InstanceId {
    InstanceId("pgdog-b".into())
}

/// `RELOAD` goes to a replica only once its sink answers the pushed
/// generation, and never to one whose sink does not.
#[tokio::test]
async fn reload_after_generation_ack() {
    let fake = Fake::default();
    fake.lock().ack_after = 3;
    fake.lock().stuck.push("pgdog-b".into());
    let p = pusher(store_with(NS, PID, BID).await, &fake);

    let pushed = p.push(NS).await.unwrap();
    let g = pushed.generation;
    assert_eq!(pushed.applied, vec![a()]);
    assert_eq!(pushed.failed.len(), 1);
    assert_eq!(pushed.failed[0].0, b());
    assert_eq!(
        fake.events(),
        vec![
            Event::Sent("pgdog-a".into(), g),
            Event::Reload("pgdog-a".into(), Some(g)),
            Event::Sent("pgdog-b".into(), g),
        ],
        "a reloaded after its sink held {g}; b never acknowledged, never reloaded"
    );
    assert_eq!(p.applied(NS, &a()), Some(g));
    assert_eq!(p.applied(NS, &b()), None);
}

/// A replica whose `RELOAD` failed is not recorded as applied, and the
/// next drift check pushes to it again although its files are current.
#[tokio::test]
async fn failed_reload_is_retried_by_resync() {
    let fake = Fake::default();
    fake.lock().reload_fails.push("pgdog-b".into());
    let p = pusher(store_with(NS, PID, BID).await, &fake);

    let pushed = p.push(NS).await.unwrap();
    assert_eq!(pushed.applied, vec![a()]);
    assert_eq!(p.applied(NS, &b()), None);

    fake.lock().reload_fails.clear();
    fake.lock().events.clear();
    assert!(p.resync(NS).await.unwrap(), "b's reload is outstanding");
    assert_eq!(p.applied(NS, &b()), Some(pushed.generation));
    assert!(!p.resync(NS).await.unwrap());
}

/// Many kicks for a namespace within the debounce window make one push;
/// each namespace is pushed on its own; a later kick pushes again.
#[tokio::test]
async fn push_debounces() {
    let store = store_with(NS, PID, BID).await;
    seed(
        &store,
        "other",
        "prj-01JZ0000000000000000000009",
        "br-01JZ0000000000000000000009",
    )
    .await;
    let fake = Fake::default();
    let p = Arc::new(pusher(store, &fake));
    let (kick, kicks) = mpsc::unbounded_channel();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let run = tokio::spawn(p.clone().run(kicks, async {
        let _ = stopped.await;
    }));

    for _ in 0..20 {
        kick.send(NS.to_owned()).unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    kick.send("other".to_owned()).unwrap();
    assert_eq!(fake.sends(), 0, "nothing before the window closes");
    tokio::time::sleep(Duration::from_millis(600)).await;
    // NS has two replicas, "other" one: one push each.
    assert_eq!(fake.sends(), 3, "{:?}", fake.events());
    assert!(p.applied(NS, &a()).is_some());
    assert!(
        p.applied("other", &InstanceId("pgdog-other".into()))
            .is_some()
    );

    kick.send(NS.to_owned()).unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(fake.sends(), 5, "{:?}", fake.events());

    stop.send(()).unwrap();
    run.await.unwrap();
}

/// The drift check pushes when a replica holds another generation than the
/// records render (a restart onto an older copy, a change never pushed),
/// and does nothing when every replica is current.
#[tokio::test]
async fn drift_triggers_repush() {
    let store = store_with(NS, PID, BID).await;
    let fake = Fake::default();
    let p = pusher(store.clone(), &fake);

    // pg-control has never pushed: both replicas drift.
    assert!(p.resync(NS).await.unwrap());
    let g1 = p.applied(NS, &a()).unwrap();
    assert_eq!(p.applied(NS, &b()), Some(g1));
    assert_eq!(fake.sends(), 2);
    assert!(!p.resync(NS).await.unwrap(), "current: no push");
    assert_eq!(fake.sends(), 2);

    // b restarted onto an older persisted copy.
    fake.lock().held.insert("pgdog-b".into(), 1);
    assert!(p.resync(NS).await.unwrap());
    assert_eq!(fake.lock().held["pgdog-b"], g1);

    // The records changed and nobody kicked the pusher.
    add_endpoint(&store).await;
    let before = fake.sends();
    assert!(p.resync(NS).await.unwrap());
    let g2 = p.applied(NS, &a()).unwrap();
    assert_ne!(g1, g2);
    assert_eq!(fake.sends(), before + 2);
    assert!(!p.resync(NS).await.unwrap());
}

/// Compose: the files land in `<root>/<namespace>/`, and the generation
/// read back is the one written.
#[tokio::test]
async fn dir_sink_writes_the_namespace_directory() {
    let root = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    let mut p = pusher(store_with(NS, PID, BID).await, &fake);
    p.sink = Arc::new(DirSink {
        root: root.path().to_path_buf(),
    });
    let pushed = p.push(NS).await.unwrap();
    assert_eq!(pushed.applied, vec![a(), b()]);
    let dir = root.path().join(NS);
    assert_eq!(
        loams_pgdog_sync::read_generation(&dir),
        Some(pushed.generation)
    );
    let toml = std::fs::read_to_string(dir.join(loams_pgdog_sync::PGDOG_TOML)).unwrap();
    assert!(toml.contains("10.0.0.1"), "{toml}");
}

/// A CA and a leaf it signs, as PEM.
fn ca(name: &str) -> (rcgen::KeyPair, rcgen::CertificateParams, String) {
    let mut p = rcgen::CertificateParams::new(vec![]).unwrap();
    p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    p.distinguished_name.push(rcgen::DnType::CommonName, name);
    let key = rcgen::KeyPair::generate().unwrap();
    let pem = p.clone().self_signed(&key).unwrap().pem();
    (key, p, pem)
}

fn leaf(name: &str, ca: &(rcgen::KeyPair, rcgen::CertificateParams, String)) -> (String, String) {
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = rcgen::CertificateParams::new(vec![name.into()])
        .unwrap()
        .signed_by(&key, &rcgen::Issuer::new(ca.1.clone(), &ca.0))
        .unwrap();
    (cert.pem(), key.serialize_pem())
}

/// Kubernetes: the pusher reaches each replica's `pgdog-config-sync` over
/// mutual TLS, the files land, the generation is acknowledged, and only
/// then `RELOAD` goes out. A client certificate from another CA gets
/// nothing written.
#[tokio::test]
async fn sidecar_sink_pushes_over_mtls() {
    let authority = ca("pg-control CA");
    let (server_cert, server_key) = leaf("localhost", &authority);
    let (client_cert, client_key) = leaf("pg-control", &authority);
    let stranger = leaf("pg-control", &ca("another CA"));
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = loams_pgdog_sync::Server {
        dir: dir.path().to_path_buf(),
        tls: Arc::new(
            loams_pgdog_sync::server_tls(
                server_cert.as_bytes(),
                server_key.as_bytes(),
                authority.2.as_bytes(),
            )
            .unwrap(),
        ),
    };
    tokio::spawn(server.serve(listener, std::future::pending()));

    /// One replica, reached as `localhost` (the name on the sidecar's
    /// certificate).
    #[derive(Clone)]
    struct Local(Fake);
    #[async_trait]
    impl PgDogAdmin for Local {
        async fn instances(&self, _: &str) -> Result<Vec<InstanceId>, PgDogError> {
            Ok(vec![InstanceId("localhost".into())])
        }
        async fn reload(&self, i: &InstanceId) -> Result<(), PgDogError> {
            self.0.reload(i).await
        }
        async fn show_pools(&self, _: &InstanceId) -> Result<Vec<PoolRow>, PgDogError> {
            Ok(Vec::new())
        }
        async fn show_databases(&self, _: &InstanceId) -> Result<Vec<String>, PgDogError> {
            Ok(Vec::new())
        }
    }
    let fake = Fake::default();
    let store = store_with(NS, PID, BID).await;
    let sink = |(cert, key): &(String, String)| {
        SidecarSink::new(
            port,
            cert.as_bytes(),
            key.as_bytes(),
            authority.2.as_bytes(),
            Duration::from_secs(5),
        )
        .unwrap()
    };

    let mut refused = pusher(store.clone(), &fake);
    refused.admin = Arc::new(Local(fake.clone()));
    refused.sink = Arc::new(sink(&stranger));
    let pushed = refused.push(NS).await.unwrap();
    assert!(pushed.applied.is_empty(), "{pushed:?}");
    assert_eq!(loams_pgdog_sync::read_generation(dir.path()), None);
    assert!(fake.events().is_empty(), "no RELOAD without the files");

    let mut p = pusher(store, &fake);
    p.admin = Arc::new(Local(fake.clone()));
    p.sink = Arc::new(sink(&(client_cert, client_key)));
    let pushed = p.push(NS).await.unwrap();
    assert_eq!(pushed.applied, vec![InstanceId("localhost".into())]);
    assert_eq!(
        loams_pgdog_sync::read_generation(dir.path()),
        Some(pushed.generation)
    );
    assert_eq!(
        fake.events(),
        vec![Event::Reload("localhost".into(), None)],
        "RELOAD once the sidecar answered the generation"
    );
    assert!(!p.resync(NS).await.unwrap());
}
