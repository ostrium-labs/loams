//! Health and readiness probes (D1956).
//!
//! - **Live**: the frontend answers `SELECT 1` over PG wire (:4566), and
//!   the meta node's dashboard answers `GET /api/version` (:5691).
//! - **Ready**: the dashboard's `GET /api/clusters/{worker type}` lists as
//!   many `RUNNING` compute nodes and compactors as the spec asks, and at
//!   least one frontend.
//!
//! The frontend's :6786 listener is gRPC health only, so it is not used.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_postgres::{NoTls, SimpleQueryMessage};

use crate::backend::Secret;
use crate::model::{Role, ports};

/// One probe's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// Liveness and readiness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// The processes answer.
    pub live: bool,
    /// Every role has as many running workers as the spec asks.
    pub ready: bool,
    pub checks: Vec<Check>,
}

impl Health {
    /// `live` when every check named in `live_names` passed, `ready` when
    /// every check passed.
    pub fn from_checks(checks: Vec<Check>, live_names: &[&str]) -> Self {
        let live = checks
            .iter()
            .filter(|c| live_names.contains(&c.name.as_str()))
            .all(|c| c.ok);
        let ready = live && checks.iter().all(|c| c.ok);
        Self {
            live,
            ready,
            checks,
        }
    }
}

/// Where and as whom to probe a cluster.
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub frontend_host: String,
    pub frontend_port: u16,
    pub meta_host: String,
    pub dashboard_port: u16,
    /// The RisingWave user the probe logs in as.
    pub user: String,
    /// `None` before the bootstrap set the root password.
    pub password: Option<Secret>,
    /// Replicas the spec asks for, by role.
    pub expect: BTreeMap<Role, u32>,
    pub timeout: Duration,
}

impl ProbeTarget {
    pub fn new(frontend_host: String, meta_host: String) -> Self {
        Self {
            frontend_host,
            frontend_port: ports::PGWIRE,
            meta_host,
            dashboard_port: ports::DASHBOARD,
            user: "root".into(),
            password: None,
            expect: BTreeMap::new(),
            timeout: Duration::from_secs(5),
        }
    }

    fn dashboard_url(&self, path: &str) -> String {
        format!(
            "http://{}:{}/api/{path}",
            bracket(&self.meta_host),
            self.dashboard_port
        )
    }
}

fn bracket(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

/// The names of the liveness checks.
pub const LIVE_CHECKS: [&str; 2] = ["pgwire", "dashboard"];

/// Probes a cluster.
#[async_trait]
pub trait Prober: Send + Sync {
    async fn probe(&self, target: &ProbeTarget) -> Health;
}

/// Probes over the network.
#[derive(Debug, Clone)]
pub struct LiveProber {
    http: reqwest::Client,
}

impl Default for LiveProber {
    fn default() -> Self {
        Self::new()
    }
}

impl LiveProber {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }

    async fn pgwire(&self, t: &ProbeTarget) -> Check {
        let result = tokio::time::timeout(t.timeout, select_one(t)).await;
        let (ok, detail) = match result {
            Ok(Ok(())) => (true, "SELECT 1 ok".to_owned()),
            Ok(Err(e)) => (false, e),
            Err(_) => (false, format!("no answer within {:?}", t.timeout)),
        };
        Check {
            name: "pgwire".into(),
            ok,
            detail,
        }
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        t: &ProbeTarget,
        path: &str,
    ) -> Result<T, String> {
        let resp = self
            .http
            .get(t.dashboard_url(path))
            .timeout(t.timeout)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("GET /api/{path}: {status}"));
        }
        resp.json::<T>().await.map_err(|e| e.to_string())
    }

    async fn dashboard(&self, t: &ProbeTarget) -> Check {
        let (ok, detail) = match self.get_json::<String>(t, "version").await {
            Ok(v) => (true, format!("version {v}")),
            Err(e) => (false, e),
        };
        Check {
            name: "dashboard".into(),
            ok,
            detail,
        }
    }

    async fn workers(&self, t: &ProbeTarget, role: Role) -> Check {
        let want = match role {
            // At least one frontend serves; the gateway balances over the rest.
            Role::Frontend => t.expect.get(&role).copied().unwrap_or(1).min(1),
            _ => t.expect.get(&role).copied().unwrap_or(1),
        };
        let (ok, detail) = match self
            .get_json::<Vec<WorkerNode>>(t, &role.worker_type().to_string())
            .await
        {
            Ok(nodes) => {
                let running = running_workers(&nodes);
                (
                    running >= want as usize,
                    format!("{running}/{want} {role} running"),
                )
            }
            Err(e) => (false, e),
        };
        Check {
            name: role.name().into(),
            ok,
            detail,
        }
    }
}

#[async_trait]
impl Prober for LiveProber {
    async fn probe(&self, t: &ProbeTarget) -> Health {
        let (pgwire, dashboard) = tokio::join!(self.pgwire(t), self.dashboard(t));
        let mut checks = vec![pgwire, dashboard];
        // Readiness needs the dashboard.
        if checks[1].ok {
            for role in [Role::Compute, Role::Compactor, Role::Frontend] {
                checks.push(self.workers(t, role).await);
            }
        }
        let mut health = Health::from_checks(checks, &LIVE_CHECKS);
        if !health.checks.iter().any(|c| c.name == "compute") {
            health.ready = false;
        }
        health
    }
}

/// `SELECT 1` as the target's user.
async fn select_one(t: &ProbeTarget) -> Result<(), String> {
    let client = connect(
        &t.frontend_host,
        t.frontend_port,
        &t.user,
        t.password.as_ref(),
        t.timeout,
    )
    .await?;
    let rows = client
        .simple_query("SELECT 1")
        .await
        .map_err(|e| e.to_string())?;
    let one = rows.iter().any(|m| match m {
        SimpleQueryMessage::Row(r) => r.get(0) == Some("1"),
        _ => false,
    });
    if one {
        Ok(())
    } else {
        Err("SELECT 1 returned no row".into())
    }
}

/// A PG wire session to a RisingWave frontend (database `dev`, its
/// default). Plaintext: frontends are reached only inside the instance's
/// network; clients come through loams-gateway's TLS.
pub async fn connect(
    host: &str,
    port: u16,
    user: &str,
    password: Option<&Secret>,
    timeout: Duration,
) -> Result<tokio_postgres::Client, String> {
    let mut config = tokio_postgres::Config::new();
    config
        .host(host)
        .port(port)
        .user(user)
        .dbname("dev")
        .application_name("loams-risingwave")
        .connect_timeout(timeout);
    if let Some(p) = password {
        config.password(p.expose());
    }
    let (client, connection) = config.connect(NoTls).await.map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::debug!(error = %e, "risingwave probe connection closed");
        }
    });
    Ok(client)
}

/// The fields of `common.WorkerNode` the probe reads, as the dashboard
/// serializes it.
#[derive(Debug, Clone, Deserialize)]
pub struct WorkerNode {
    #[serde(default)]
    pub id: Option<serde_json::Value>,
    /// `RUNNING`, `STARTING` (or the `STATE_`-prefixed forms; the probe
    /// accepts both, and a number).
    #[serde(default)]
    pub state: Option<serde_json::Value>,
}

/// Workers in the running state.
pub fn running_workers(nodes: &[WorkerNode]) -> usize {
    nodes
        .iter()
        .filter(|n| match &n.state {
            Some(serde_json::Value::String(s)) => s == "RUNNING" || s.ends_with("_RUNNING"),
            // `State::Running` is 2.
            Some(serde_json::Value::Number(n)) => n.as_u64() == Some(2),
            _ => false,
        })
        .count()
}

/// A prober with a scripted answer, for tests.
#[derive(Debug, Clone)]
pub struct FixedProber(pub Health);

impl FixedProber {
    pub fn ready() -> Self {
        Self(Health::from_checks(
            vec![
                Check {
                    name: "pgwire".into(),
                    ok: true,
                    detail: "fixed".into(),
                },
                Check {
                    name: "dashboard".into(),
                    ok: true,
                    detail: "fixed".into(),
                },
                Check {
                    name: "compute".into(),
                    ok: true,
                    detail: "fixed".into(),
                },
            ],
            &LIVE_CHECKS,
        ))
    }

    pub fn down() -> Self {
        Self(Health::from_checks(
            vec![Check {
                name: "pgwire".into(),
                ok: false,
                detail: "fixed".into(),
            }],
            &LIVE_CHECKS,
        ))
    }
}

#[async_trait]
impl Prober for FixedProber {
    async fn probe(&self, _target: &ProbeTarget) -> Health {
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, ok: bool) -> Check {
        Check {
            name: name.into(),
            ok,
            detail: String::new(),
        }
    }

    #[test]
    fn health_from_checks() {
        let h = Health::from_checks(
            vec![check("pgwire", true), check("compute", false)],
            &["pgwire"],
        );
        assert!(h.live && !h.ready);
        let h = Health::from_checks(
            vec![check("pgwire", false), check("compute", true)],
            &["pgwire"],
        );
        assert!(!h.live && !h.ready);
        let h = Health::from_checks(vec![check("pgwire", true)], &["pgwire"]);
        assert!(h.live && h.ready);
    }

    #[test]
    fn counts_running_workers() {
        let nodes: Vec<WorkerNode> = serde_json::from_str(
            r#"[{"id":1,"type":"WORKER_TYPE_COMPUTE_NODE","state":"RUNNING"},
                {"id":2,"state":"STARTING"},
                {"id":3,"state":"STATE_RUNNING"},
                {"id":4,"state":2},
                {"id":5}]"#,
        )
        .unwrap();
        assert_eq!(running_workers(&nodes), 3);
    }

    #[test]
    fn dashboard_urls() {
        let mut t = ProbeTarget::new("fe".into(), "rw-acme-meta-0".into());
        assert_eq!(
            t.dashboard_url("clusters/2"),
            "http://rw-acme-meta-0:5691/api/clusters/2"
        );
        t.meta_host = "fd00::1".into();
        assert_eq!(
            t.dashboard_url("version"),
            "http://[fd00::1]:5691/api/version"
        );
    }

    #[tokio::test]
    async fn fixed_probers() {
        let t = ProbeTarget::new("fe".into(), "meta".into());
        assert!(FixedProber::ready().probe(&t).await.ready);
        let down = FixedProber::down().probe(&t).await;
        assert!(!down.live && !down.ready);
    }

    #[tokio::test]
    async fn live_prober_reports_an_unreachable_cluster() {
        let mut t = ProbeTarget::new("127.0.0.1".into(), "127.0.0.1".into());
        // Port 9 (discard) is closed on a CI runner; either way nothing
        // speaks PG wire or HTTP there.
        t.frontend_port = 9;
        t.dashboard_port = 9;
        t.timeout = Duration::from_millis(500);
        let h = LiveProber::new().probe(&t).await;
        assert!(!h.live);
        assert!(!h.ready);
        assert_eq!(h.checks.len(), 2);
    }
}
