//! `loams-transfer serve`: the transfer API without the engine, for tests,
//! benchmarks and dev (§85 §4, D2421). It runs a single-node metastore and a
//! bucket (a local folder or any `loams-store` URL), and serves HTTP/3 on UDP
//! and HTTP/2+TLS on TCP at the same port number (D2437).

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use loams_common::NamespaceId;
use loams_common::meta::{ApplyError, Consistency, MetaError, MetaStore};
use loams_meta::{
    MetaClient, MetaClientConfig, MetaConfig, MetaNode, Router as MetaRouter, SystemClock,
};
use loams_store::Store;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::checkpoint::{Checkpoints, MemCheckpoints, MetaCheckpoints};
use crate::net::{ServerIdentity, h2, h3};
use crate::run::Runs;
use crate::server::{Access, Api, Authorizer, MemNamespaces, Namespaces, StaticTokens, router};
use crate::transfer::{Transfers, TransfersConfig};

/// Namespaces in the metastore, created on first use.
#[derive(Debug)]
pub struct MetaNamespaces(pub Arc<dyn MetaStore>);

#[async_trait]
impl Namespaces for MetaNamespaces {
    async fn resolve(&self, name: &str) -> Option<NamespaceId> {
        if let Ok(Some(ns)) = self
            .0
            .namespace_by_name(Consistency::Linearizable, name)
            .await
        {
            return Some(ns.id);
        }
        match self.0.create_namespace(name).await {
            Ok(id) => Some(id),
            Err(MetaError::Rejected(ApplyError::NamespaceExists(id))) => Some(id),
            Err(e) => {
                tracing::warn!(error = %e, name, "namespace");
                None
            }
        }
    }
}

/// Builds the API state over checkpoints, a bucket and an authorizer.
pub fn api(
    store: Store,
    ck: Arc<dyn Checkpoints>,
    cfg: TransfersConfig,
    auth: Arc<dyn Authorizer>,
) -> Api {
    let lease = cfg.lease_ttl;
    Api {
        transfers: Arc::new(Transfers::new(store.clone(), Arc::clone(&ck), cfg)),
        runs: Arc::new(Runs::new(store, ck, lease)),
        auth,
        tail_poll: Duration::from_millis(250),
    }
}

/// An in-memory API with one token that has every access: for tests.
pub fn mem_api(token: &str, cfg: TransfersConfig) -> (Api, MemCheckpoints) {
    let ck = MemCheckpoints::new();
    let auth = StaticTokens::new(Arc::new(MemNamespaces::default()))
        .with(token, &[Access::Read, Access::Write, Access::Runner]);
    let a = api(
        Store::in_memory(),
        Arc::new(ck.clone()),
        cfg,
        Arc::new(auth),
    );
    (a, ck)
}

/// A single-node metastore under `dir`, started and led.
pub async fn start_meta(dir: &Path, store: Store) -> io::Result<(MetaNode, Arc<dyn MetaStore>)> {
    std::fs::create_dir_all(dir)?;
    let clock = Arc::new(SystemClock);
    let mut config = MetaConfig::new(1, dir, store);
    config.clock = clock.clone();
    let node = MetaNode::start(config, &MetaRouter::new())
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;
    // A node restarted on its own directory is already initialized.
    let _ = node.initialize([1]).await;
    node.wait_for_leader(Duration::from_secs(30))
        .await
        .map_err(|e| io::Error::other(e.to_string()))?;
    let meta: Arc<dyn MetaStore> = Arc::new(MetaClient::new(
        node.clone(),
        vec![],
        clock,
        MetaClientConfig::default(),
    ));
    Ok((node, meta))
}

/// The API on a metastore: checkpoints and namespaces come from it.
pub fn meta_api(
    meta: Arc<dyn MetaStore>,
    store: Store,
    cfg: TransfersConfig,
    tokens: &[(String, Vec<Access>)],
) -> Api {
    let mut auth = StaticTokens::new(Arc::new(MetaNamespaces(Arc::clone(&meta))));
    for (t, a) in tokens {
        auth = auth.with(t, a);
    }
    api(
        store,
        Arc::new(MetaCheckpoints::new(meta)),
        cfg,
        Arc::new(auth),
    )
}

/// Which listeners to run.
#[derive(Clone, Copy, Debug)]
pub struct Serve {
    /// HTTP/3 on UDP.
    pub h3: bool,
    /// HTTP/2+TLS on TCP.
    pub h2: bool,
}

/// Running listeners; dropping this stops them.
#[derive(Debug)]
pub struct Listeners {
    /// The bound address (UDP and TCP share the port).
    pub addr: SocketAddr,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Listeners {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// Binds HTTP/3 on UDP and HTTP/2+TLS on TCP at `addr` (a port of 0 picks
/// one free on both) and serves `app` on them.
pub async fn bind(
    addr: SocketAddr,
    id: &ServerIdentity,
    app: Router,
    which: Serve,
) -> io::Result<Listeners> {
    let mut last = None;
    for _ in 0..20 {
        let tcp = TcpListener::bind(addr).await?;
        let bound = tcp.local_addr()?;
        let quic = if which.h3 {
            match h3::bind_server(bound, id) {
                Ok(e) => Some(e),
                // The TCP port's UDP twin is taken: try another port.
                Err(e) if addr.port() == 0 => {
                    last = Some(e);
                    continue;
                }
                Err(e) => return Err(e),
            }
        } else {
            None
        };
        let mut tasks = Vec::new();
        if which.h2 {
            let (id, app) = (id.clone(), app.clone());
            tasks.push(tokio::spawn(async move {
                if let Err(e) = h2::serve_tls(tcp, &id, app).await {
                    tracing::warn!(error = %e, "TCP listener stopped");
                }
            }));
        }
        if let Some(endpoint) = quic {
            tasks.push(tokio::spawn(h3::serve(endpoint, app.clone())));
        }
        return Ok(Listeners { addr: bound, tasks });
    }
    Err(last.unwrap_or_else(|| io::Error::other("no free port for UDP and TCP")))
}

/// Serves `app` without TLS on a loopback address (D2437).
pub async fn bind_plain(addr: SocketAddr, app: Router) -> io::Result<Listeners> {
    if !addr.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "plain HTTP binds loopback only",
        ));
    }
    let tcp = TcpListener::bind(addr).await?;
    let bound = tcp.local_addr()?;
    let task = tokio::spawn(async move {
        if let Err(e) = h2::serve_plain(tcp, app).await {
            tracing::warn!(error = %e, "plain listener stopped");
        }
    });
    Ok(Listeners {
        addr: bound,
        tasks: vec![task],
    })
}

/// The router for an API.
pub fn app(api: Api) -> Router {
    router(api)
}
