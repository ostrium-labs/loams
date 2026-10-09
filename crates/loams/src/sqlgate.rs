//! The Loams SQL gate listener (`--sqlgate-listen`, feature `sqldb`; plan
//! SQ1 Task 4). Until `loams.sqldb.v1` is wired (Task 12) it knows no users,
//! pools or credentials: every login gets 1045, and nothing reaches TiDB.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use loams_sqlgate::auth::StaticUsers;
use loams_sqlgate::limits::ActivityCounter;
use loams_sqlgate::server::{
    Gate, GateConfig, GateDeps, sni_cert_from_pem, sni_server_config, upstream_tls_from_ca_pem,
};
use loams_sqlgate::upstream::{NoCredentials, NoPools};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Where the gate listens and its TLS material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlgateConfig {
    /// The MySQL listener (TLS; plaintext only from loopback peers).
    pub listen: SocketAddr,
    /// The certificate chain (PEM) clients see.
    pub tls_cert: PathBuf,
    /// Its private key (PEM).
    pub tls_key: PathBuf,
    /// The CA (PEM) that signs the TiDB pools' certificates.
    pub upstream_ca: PathBuf,
}

/// A running gate.
#[derive(Debug)]
pub struct SqlgateHandle {
    /// The bound address.
    pub addr: SocketAddr,
    task: JoinHandle<()>,
}

impl SqlgateHandle {
    /// Stops accepting; open connections are dropped with the runtime.
    pub async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

/// Binds the listener (before any task starts, like the other listeners).
pub async fn listen(config: &SqlgateConfig) -> std::io::Result<(TcpListener, SocketAddr)> {
    let listener = TcpListener::bind(config.listen).await?;
    let addr = listener.local_addr()?;
    Ok((listener, addr))
}

/// Loads the TLS material and serves the gate on `listener`.
pub fn start(
    listener: TcpListener,
    addr: SocketAddr,
    config: &SqlgateConfig,
) -> std::io::Result<SqlgateHandle> {
    let cert = sni_cert_from_pem(Vec::new(), &config.tls_cert, &config.tls_key)?;
    let tls = sni_server_config(vec![cert])
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let upstream = upstream_tls_from_ca_pem(&config.upstream_ca)?;
    let deps = GateDeps {
        users: Arc::new(StaticUsers::default()),
        credentials: Arc::new(NoCredentials),
        pools: Arc::new(NoPools),
        activity: Arc::new(ActivityCounter::default()),
    };
    let gate = Gate::new(GateConfig::new(tls, upstream), deps);
    let task = tokio::spawn(async move {
        if let Err(err) = gate.serve(listener).await {
            tracing::error!(%err, "Loams SQL gate failed");
        }
    });
    tracing::info!(%addr, "Loams SQL gate listening");
    Ok(SqlgateHandle { addr, task })
}
