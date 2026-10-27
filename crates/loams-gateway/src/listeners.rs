//! Listeners (design §57 §5, §7.5, §8.2): the shared Postgres port and the
//! admin port, bound at start; the dedicated namespace ports, reconciled to
//! the route table after every swap; a connection cap over all of them, and
//! a drain on shutdown.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;

use loams_meter_gate::entitle::Entitlements;

use crate::admin;
use crate::config::{GatewayConfig, TlsMode};
use crate::observe::{ConnectionObserver, NoopObserver};
use crate::pg::{Entry, FrontDoorOptions, PgFrontDoor};
use crate::reload::{self, RouteRx};
use crate::risingwave::RwFrontDoor;
use crate::tls::{self, ServerCerts, TlsError};

#[derive(Debug, thiserror::Error)]
pub enum GatewayError {
    /// A fixed listener could not be bound.
    #[error("binding {addr}: {source}")]
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
    /// A certificate, key or CA file could not be loaded.
    #[error(transparent)]
    Tls(#[from] TlsError),
    /// The RisingWave module's certificates or credential check.
    #[error("risingwave: {0}")]
    RisingWave(String),
    /// The Spark Connect module's keys, routes or authorizer.
    #[error("spark: {0}")]
    Spark(String),
}

/// What the admin listener reports about listeners.
#[derive(Debug, Default)]
pub struct ListenerStatus {
    pub dedicated_bound: BTreeSet<u16>,
    pub bind_failures: BTreeMap<u16, String>,
}

pub type SharedStatus = Arc<Mutex<ListenerStatus>>;

/// A running gateway.
#[derive(Debug)]
pub struct Gateway {
    pg_addr: SocketAddr,
    admin_addr: SocketAddr,
    spark_addr: Option<SocketAddr>,
    routes: RouteRx,
    shutdown: watch::Sender<bool>,
    tasks: Vec<JoinHandle<()>>,
    conns: Arc<Semaphore>,
    max_connections: u32,
    drain_timeout: Duration,
}

impl Gateway {
    /// Loads the TLS material, binds the fixed listeners, starts the routes
    /// watcher and the dedicated-port reconciler.
    pub async fn start(config: GatewayConfig) -> Result<Gateway, GatewayError> {
        Self::start_with_observer(config, Arc::new(NoopObserver)).await
    }

    /// [`Gateway::start`] with an observer that receives every routed
    /// connection's open and close.
    pub async fn start_with_observer(
        config: GatewayConfig,
        observer: Arc<dyn ConnectionObserver>,
    ) -> Result<Gateway, GatewayError> {
        Self::start_metered(config, observer, Entitlements::default()).await
    }

    /// [`Gateway::start_with_observer`] with the meter's gate, asked when a
    /// Postgres connection's namespace is resolved (design §63 §10.2).
    pub async fn start_metered(
        config: GatewayConfig,
        observer: Arc<dyn ConnectionObserver>,
        entitlements: Entitlements,
    ) -> Result<Gateway, GatewayError> {
        let server_certs = ServerCerts::load(&config.pg.certs)?;
        let upstream_tls = match &config.pg.upstream_tls {
            Some(u) => Some(tls::upstream_client_config(&tls::read(&u.ca)?)?),
            None => None,
        };
        if config.pg.tls_mode == TlsMode::Passthrough {
            tracing::info!("tls_mode = passthrough: direct-TLS clients are refused");
        }
        let risingwave = if config.risingwave.certs.is_empty()
            && config.risingwave.self_signed_names.is_empty()
            && config.risingwave.auth.is_none()
        {
            None
        } else {
            Some(Arc::new(
                RwFrontDoor::from_config(&config.risingwave)
                    .await
                    .map_err(GatewayError::RisingWave)?,
            ))
        };
        let (tx, routes) = reload::channel();
        let (shutdown, shutdown_rx) = watch::channel(false);
        let mut tasks = Vec::new();
        tasks.push(tokio::spawn(reload::watch_file(
            config.routes.path.clone(),
            config.routes.poll_interval(),
            config.pg.dedicated_ports.0.clone(),
            config.routes.require_generation,
            tx,
            stopped(shutdown_rx.clone()),
        )));

        let front = Arc::new(PgFrontDoor::new(
            routes.clone(),
            FrontDoorOptions {
                mode: config.pg.tls_mode,
                handshake_timeout: config.pg.handshake_timeout(),
                connect_timeout: config.pg.upstream_connect_timeout(),
                server_certs,
                upstream_tls,
                observer,
                risingwave,
                entitlements,
            },
        ));
        let max_connections = u32::try_from(config.pg.max_connections).unwrap_or(u32::MAX);
        let conns = Arc::new(Semaphore::new(max_connections as usize));
        let status = SharedStatus::default();

        let pg = bind(config.pg.listen).await?;
        let pg_addr = pg.local_addr().map_err(|source| GatewayError::Bind {
            addr: config.pg.listen,
            source,
        })?;
        let admin_listener = bind(config.admin.listen).await?;
        let admin_addr = admin_listener
            .local_addr()
            .map_err(|source| GatewayError::Bind {
                addr: config.admin.listen,
                source,
            })?;
        tracing::info!(%pg_addr, %admin_addr, "loams-gateway listening");

        // The Spark Connect module: bound now, so a taken port fails start.
        #[cfg(feature = "spark")]
        let spark_addr = match &config.spark {
            Some(spark) => {
                let proxy = loams_spark::standalone::build(spark).map_err(GatewayError::Spark)?;
                let listener = bind(spark.listen).await?;
                let addr = listener.local_addr().map_err(|source| GatewayError::Bind {
                    addr: spark.listen,
                    source,
                })?;
                let stop = stopped(shutdown_rx.clone());
                tasks.push(tokio::spawn(async move {
                    if let Err(e) = loams_spark::standalone::serve(proxy, listener, stop).await {
                        tracing::error!(error = %e, "spark module stopped");
                    }
                }));
                tracing::info!(spark_addr = %addr, "spark module listening");
                Some(addr)
            }
            None => None,
        };
        #[cfg(not(feature = "spark"))]
        let spark_addr = None;

        tasks.push(tokio::spawn(accept_loop(
            pg,
            Entry::Shared,
            front.clone(),
            conns.clone(),
            shutdown_rx.clone(),
        )));
        tasks.push(tokio::spawn(admin::serve(
            admin_listener,
            admin::State {
                routes: routes.clone(),
                listeners: status.clone(),
            },
            stopped(shutdown_rx.clone()),
        )));
        tasks.push(tokio::spawn(reconcile_dedicated(
            routes.clone(),
            config.pg.dedicated_bind,
            front,
            conns.clone(),
            status,
            shutdown_rx,
        )));
        Ok(Gateway {
            pg_addr,
            admin_addr,
            spark_addr,
            routes,
            shutdown,
            tasks,
            conns,
            max_connections,
            drain_timeout: config.pg.drain_timeout(),
        })
    }

    /// The shared Postgres listener's address.
    pub fn pg_addr(&self) -> SocketAddr {
        self.pg_addr
    }

    /// The admin listener's address.
    pub fn admin_addr(&self) -> SocketAddr {
        self.admin_addr
    }

    /// The Spark Connect module's address, when `[spark]` is configured.
    pub fn spark_addr(&self) -> Option<SocketAddr> {
        self.spark_addr
    }

    /// The current route table.
    pub fn routes(&self) -> RouteRx {
        self.routes.clone()
    }

    /// Stops accepting everywhere, then waits up to the drain timeout for
    /// live relays to finish.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        for t in self.tasks {
            let _ = t.await;
        }
        let drained = tokio::time::timeout(
            self.drain_timeout,
            self.conns.acquire_many(self.max_connections),
        )
        .await;
        if drained.is_err() {
            tracing::warn!("drain timeout: closing live connections");
        }
    }
}

async fn bind(addr: SocketAddr) -> Result<TcpListener, GatewayError> {
    TcpListener::bind(addr)
        .await
        .map_err(|source| GatewayError::Bind { addr, source })
}

/// Resolves when the shutdown flag is set.
async fn stopped(mut rx: watch::Receiver<bool>) {
    let _ = rx.wait_for(|stop| *stop).await;
}

/// Accepts until shutdown. Connections run in their own tasks, so ending
/// this loop (shutdown, or a dedicated port leaving the table) never cuts
/// a live connection.
async fn accept_loop(
    listener: TcpListener,
    entry: Entry,
    front: Arc<PgFrontDoor>,
    conns: Arc<Semaphore>,
    shutdown: watch::Receiver<bool>,
) {
    let stop = stopped(shutdown);
    tokio::pin!(stop);
    loop {
        let accepted = tokio::select! {
            _ = &mut stop => return,
            a = listener.accept() => a,
        };
        let (stream, peer) = match accepted {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(?entry, error = %e, "accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let Ok(permit) = conns.clone().try_acquire_owned() else {
            tracing::warn!(?entry, %peer, "connection cap reached; closing");
            drop(stream);
            continue;
        };
        let _ = stream.set_nodelay(true);
        let front = front.clone();
        tokio::spawn(async move {
            front.serve(stream, entry).await;
            drop(permit);
        });
    }
}

/// Binds every dedicated port in the table and closes the ones that left
/// it, after every route swap. A failed bind is reported and retried on
/// the next swap.
async fn reconcile_dedicated(
    mut routes: RouteRx,
    bind_ip: IpAddr,
    front: Arc<PgFrontDoor>,
    conns: Arc<Semaphore>,
    status: SharedStatus,
    shutdown: watch::Receiver<bool>,
) {
    let mut running: HashMap<u16, (watch::Sender<bool>, JoinHandle<()>)> = HashMap::new();
    let stop = stopped(shutdown.clone());
    tokio::pin!(stop);
    loop {
        let wanted: BTreeSet<u16> = routes
            .borrow_and_update()
            .as_ref()
            .map(|t| t.dedicated_ports().collect())
            .unwrap_or_default();
        running.retain(|port, (stop_tx, _)| {
            let keep = wanted.contains(port);
            if !keep {
                let _ = stop_tx.send(true);
                tracing::info!(port, "dedicated port closed");
            }
            keep
        });
        let mut failures = BTreeMap::new();
        for port in &wanted {
            if running.contains_key(port) {
                continue;
            }
            let addr = SocketAddr::new(bind_ip, *port);
            match TcpListener::bind(addr).await {
                Ok(listener) => {
                    let (stop_tx, stop_rx) = watch::channel(false);
                    let handle = tokio::spawn(accept_loop(
                        listener,
                        Entry::Dedicated(*port),
                        front.clone(),
                        conns.clone(),
                        merge(shutdown.clone(), stop_rx),
                    ));
                    tracing::info!(port, "dedicated port bound");
                    running.insert(*port, (stop_tx, handle));
                }
                Err(e) => {
                    tracing::error!(port, error = %e, "dedicated port bind failed");
                    failures.insert(*port, e.to_string());
                }
            }
        }
        {
            let mut s = status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            s.dedicated_bound = running.keys().copied().collect();
            s.bind_failures = failures;
        }
        tokio::select! {
            _ = &mut stop => break,
            changed = routes.changed() => if changed.is_err() { break },
        }
    }
    for (_, (stop_tx, handle)) in running {
        let _ = stop_tx.send(true);
        let _ = handle.await;
    }
}

/// A flag set when either input is set.
fn merge(a: watch::Receiver<bool>, b: watch::Receiver<bool>) -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(*a.borrow() || *b.borrow());
    tokio::spawn(async move {
        tokio::select! {
            _ = stopped(a) => {}
            _ = stopped(b) => {}
        }
        let _ = tx.send(true);
    });
    rx
}
