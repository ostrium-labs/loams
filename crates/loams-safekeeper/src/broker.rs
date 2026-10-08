//! Storage broker publication and discovery (§28 §6.7, PG2 Task 32).
//!
//! The pageserver finds the WAL service of a timeline only through Neon's
//! storage broker. Every second, `loams-wal` publishes a
//! `SafekeeperTimelineInfo` for each timeline it serves, as the fork's
//! safekeeper does (`safekeeper/src/broker.rs` `push_loop`). It also answers
//! `SafekeeperDiscoveryRequest`s for any timeline its store holds (the fork's
//! `discover_loop`), which a pageserver sends when it has no candidate, for
//! example after a restart of either side.
//!
//! - `safekeeper_id` is the node id walproposer sees: the acceptor's id, or
//!   the pool's logical id for a stateless TiKV pool, whose instances all
//!   advertise the pool's address.
//! - The published set is bounded ([`Published`]). A timeline is published
//!   while a proposer streams to this instance or a pageserver reads from it;
//!   for `staleness` after that while its pageserver has not caught up
//!   (`remote_consistent_lsn < commit_lsn`); and, from discovery, only while
//!   it is being asked about. It leaves the set when it has been inactive
//!   for `staleness`, so an Arm A acceptor the pageserver does not stream
//!   from stops publishing even though its `remote_consistent_lsn` never
//!   moves (ruling R32.5; Task 39 owns a peer pull of that LSN).
//! - Each round reads the heads of the set's timelines with bounded
//!   concurrency within a time budget, starting where the previous round
//!   stopped, so a slow store delays publication without stalling it.
//!
//! The client is generated from the vendored `proto/storage_broker/
//! broker.proto` (Apache-2.0, Neon; see the repository's `NOTICE`).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, info, warn};

use crate::Error;
use crate::service::WalService;
use crate::store::WalStore;
use crate::types::{Id, Lsn, NodeId, TimelineId};

/// The generated messages and the client (and the server, for tests).
#[allow(clippy::all, missing_docs, unreachable_pub)]
pub mod proto {
    tonic::include_proto!("storage_broker");
}

use proto::broker_service_client::BrokerServiceClient;
use proto::{
    FilterTenantTimelineId, MessageType, SafekeeperDiscoveryResponse, SafekeeperTimelineInfo,
    SubscribeByFilterRequest, TenantTimelineId as ProtoTtid, TypeSubscription, TypedMessage,
};

/// How `loams-wal` reaches the broker and what it advertises.
#[derive(Clone, Debug)]
pub struct BrokerConfig {
    /// The broker's gRPC endpoint, for example `http://127.0.0.1:50051`.
    pub endpoint: String,
    /// The node id published as `safekeeper_id`.
    pub node_id: NodeId,
    /// The Postgres address the pageserver connects to (`host:port`).
    pub advertise_pg: String,
    /// The HTTP API's address.
    pub advertise_http: String,
    pub availability_zone: Option<String>,
    /// How often every served timeline is published (the fork: 1 s).
    pub interval: Duration,
    /// The pause before reconnecting after an error.
    pub retry: Duration,
    /// How long an inactive timeline stays in the published set (and is
    /// published while its pageserver lags).
    pub staleness: Duration,
    /// How long a discovery request keeps its timeline published.
    pub discovery_window: Duration,
    /// Heads read at once per round.
    pub concurrency: usize,
    /// How long a round may take; what is not read by then waits for the
    /// next round.
    pub round_budget: Duration,
}

/// Check `--broker-endpoint`: `http://host:port`. TLS to the broker
/// (`https://` with a CA, as the fork's `make_tls_config`) is PG2 Task 38's.
pub fn parse_endpoint(s: &str) -> Result<String, Error> {
    let bad = |why: &str| Error::Protocol(format!("--broker-endpoint {s:?}: {why}"));
    let uri: tonic::transport::Uri = s.parse().map_err(|e| bad(&format!("{e}")))?;
    match uri.scheme_str() {
        Some("http") => {}
        Some("https") => {
            return Err(bad(
                "TLS to the storage broker is not supported yet (PG2 Task 38); \
                 use http:// on a trusted network",
            ));
        }
        Some(other) => return Err(bad(&format!("unsupported scheme {other:?}; use http://"))),
        None => return Err(bad("no scheme; use http://host:port")),
    }
    if uri.host().is_none_or(str::is_empty) {
        return Err(bad("no host"));
    }
    Ok(s.to_string())
}

/// Check an advertised address (`flag` names it in errors): `host:port`,
/// and, when the listener is not on loopback, an address other hosts can
/// reach (not unspecified, not loopback).
pub fn parse_advertise(flag: &str, s: &str, listener: SocketAddr) -> Result<String, Error> {
    let bad = |why: &str| Error::Protocol(format!("{flag} {s:?}: {why}"));
    let (host, port) = s.rsplit_once(':').ok_or_else(|| bad("not host:port"))?;
    let bracketed = host.strip_prefix('[').and_then(|h| h.strip_suffix(']'));
    if bracketed.is_none() && host.contains(':') {
        return Err(bad("an IPv6 address must be in brackets: [addr]:port"));
    }
    let host = bracketed.unwrap_or(host);
    if host.is_empty() {
        return Err(bad("no host"));
    }
    match port.parse::<u16>() {
        Ok(p) if p > 0 => {}
        _ => return Err(bad("not host:port (bad port)")),
    }
    let ip = host.parse::<IpAddr>().ok();
    if !listener.ip().is_loopback() {
        let unreachable = match ip {
            Some(ip) => ip.is_unspecified() || ip.is_loopback(),
            None => host.eq_ignore_ascii_case("localhost"),
        };
        if unreachable {
            return Err(bad(
                "not an address the pageserver can reach (unspecified or loopback) while \
                 the listener is not on loopback; advertise this host's address or the \
                 pool's Service",
            ));
        }
    }
    Ok(s.to_string())
}

impl BrokerConfig {
    /// The configuration from `loams-wal`'s options, checked: the endpoint
    /// and both advertised addresses (which default to the listeners).
    pub fn from_options(
        endpoint: &str,
        node_id: NodeId,
        advertise_pg: Option<&str>,
        advertise_http: Option<&str>,
        listen_pg: SocketAddr,
        listen_http: SocketAddr,
    ) -> Result<Self, Error> {
        let pg = advertise_pg.map_or_else(|| listen_pg.to_string(), str::to_string);
        let http = advertise_http.map_or_else(|| listen_http.to_string(), str::to_string);
        Ok(Self::new(
            parse_endpoint(endpoint)?,
            node_id,
            parse_advertise("--advertise-pg", &pg, listen_pg)?,
            parse_advertise("--advertise-http", &http, listen_http)?,
        ))
    }

    pub fn new(
        endpoint: String,
        node_id: NodeId,
        advertise_pg: String,
        advertise_http: String,
    ) -> Self {
        Self {
            endpoint,
            node_id,
            advertise_pg,
            advertise_http,
            availability_zone: None,
            interval: Duration::from_secs(1),
            retry: Duration::from_secs(1),
            staleness: Duration::from_secs(300),
            discovery_window: Duration::from_secs(30),
            concurrency: 16,
            round_budget: Duration::from_millis(500),
        }
    }
}

fn proto_ttid(tl: TimelineId) -> ProtoTtid {
    ProtoTtid {
        tenant_id: tl.tenant.0.to_vec(),
        timeline_id: tl.timeline.0.to_vec(),
    }
}

fn parse_ttid(p: &ProtoTtid) -> Result<TimelineId, Error> {
    let id = |b: &[u8], what: &str| -> Result<Id, Error> {
        <[u8; 16]>::try_from(b)
            .map(Id)
            .map_err(|_| Error::Protocol(format!("broker: a {what} of {} bytes", b.len())))
    };
    Ok(TimelineId::new(
        id(&p.tenant_id, "tenant id")?,
        id(&p.timeline_id, "timeline id")?,
    ))
}

/// What this instance publishes for `tl`, or `None` when its store does not
/// hold the timeline.
pub async fn timeline_info<S: WalStore>(
    svc: &WalService<S>,
    tl: TimelineId,
    cfg: &BrokerConfig,
) -> Result<Option<SafekeeperTimelineInfo>, Error> {
    let Some(st) = svc.store().load(&tl).await? else {
        return Ok(None);
    };
    let p = svc.progress(tl);
    // A proposer streaming here has the freshest view; the store's head is
    // otherwise current.
    let (term, flush, commit) = if p.active && p.term >= st.term {
        (p.term, p.flush_lsn, p.commit_lsn.max(st.commit_lsn))
    } else {
        (st.term, st.wal_end(), st.commit_lsn)
    };
    let commit = commit.min(flush);
    Ok(Some(SafekeeperTimelineInfo {
        safekeeper_id: cfg.node_id,
        tenant_timeline_id: Some(proto_ttid(tl)),
        term,
        last_log_term: st.last_log_term(),
        flush_lsn: flush.0,
        commit_lsn: commit.0,
        backup_lsn: st.backup_lsn.0,
        remote_consistent_lsn: st.remote_consistent_lsn.0,
        peer_horizon_lsn: p.peer_horizon_lsn.max(st.peer_horizon_lsn).0,
        local_start_lsn: st.local_start_lsn.0,
        standby_horizon: 0,
        safekeeper_connstr: cfg.advertise_pg.clone(),
        http_connstr: cfg.advertise_http.clone(),
        https_connstr: None,
        availability_zone: cfg.availability_zone.clone(),
    }))
}

/// The timelines this instance publishes (bounded; see the module docs).
#[derive(Debug, Default)]
pub struct Published {
    entries: Mutex<HashMap<TimelineId, Entry>>,
    /// Where the next round starts (rounds that run out of budget rotate).
    cursor: AtomicUsize,
}

#[derive(Clone, Copy, Debug, Default)]
struct Entry {
    /// When a proposer or a reader was last seen on this instance.
    last_active: Option<Instant>,
    /// Asked about through discovery: published until then.
    asked_until: Option<Instant>,
}

/// A timeline a round considers, and why.
#[derive(Clone, Copy, Debug)]
struct Candidate {
    tl: TimelineId,
    /// A proposer or a reader is on this instance now, or it is asked about.
    always: bool,
}

impl Published {
    /// The timelines in the set.
    pub fn timelines(&self) -> Vec<TimelineId> {
        let m = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let mut out: Vec<TimelineId> = m.keys().copied().collect();
        out.sort_unstable_by_key(|t| (t.tenant.0, t.timeline.0));
        out
    }

    /// A discovery request for `tl`: publish it until `until`.
    fn ask(&self, tl: TimelineId, until: Instant) {
        let mut m = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        let e = m.entry(tl).or_default();
        e.asked_until = Some(e.asked_until.map_or(until, |u| u.max(until)));
    }

    /// Refresh activity, drop what is stale, and list this round's
    /// candidates, rotated to start at the cursor.
    fn candidates(
        &self,
        active: &[TimelineId],
        cfg: &BrokerConfig,
        now: Instant,
    ) -> Vec<Candidate> {
        let mut m = self.entries.lock().unwrap_or_else(|p| p.into_inner());
        for tl in active {
            m.entry(*tl).or_default().last_active = Some(now);
        }
        let active: HashSet<TimelineId> = active.iter().copied().collect();
        m.retain(|tl, e| {
            let asked = e.asked_until.is_some_and(|u| u > now);
            let recent = e
                .last_active
                .is_some_and(|t| now.duration_since(t) < cfg.staleness);
            active.contains(tl) || asked || recent
        });
        let mut out: Vec<Candidate> = m
            .iter()
            .map(|(tl, e)| Candidate {
                tl: *tl,
                always: active.contains(tl) || e.asked_until.is_some_and(|u| u > now),
            })
            .collect();
        out.sort_unstable_by_key(|c| (c.tl.tenant.0, c.tl.timeline.0));
        if !out.is_empty() {
            let k = self.cursor.load(Ordering::Relaxed) % out.len();
            out.rotate_left(k);
        }
        out
    }
}

/// One round: the info to publish for every candidate whose head is read
/// within the budget, `cfg.concurrency` at a time.
pub async fn publish_round<S: WalStore>(
    svc: &Arc<WalService<S>>,
    cfg: &BrokerConfig,
    published: &Published,
) -> Vec<SafekeeperTimelineInfo> {
    let now = Instant::now();
    let cands = published.candidates(&svc.active_timelines(), cfg, now);
    let total = cands.len();
    let deadline = tokio::time::Instant::now() + cfg.round_budget;
    let mut pending = cands.into_iter();
    let mut tasks = tokio::task::JoinSet::new();
    let mut out = Vec::new();
    let mut done = 0usize;
    loop {
        while tasks.len() < cfg.concurrency.max(1) {
            let Some(c) = pending.next() else { break };
            let (svc, cfg) = (svc.clone(), cfg.clone());
            tasks.spawn(async move { (c, timeline_info(&svc, c.tl, &cfg).await) });
        }
        let next = match tokio::time::timeout_at(deadline, tasks.join_next()).await {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(_) => {
                warn!(
                    read = done,
                    total, budget = ?cfg.round_budget,
                    "broker: a round ran out of time; the rest waits for the next"
                );
                tasks.abort_all();
                break;
            }
        };
        done += 1;
        match next {
            Ok((c, Ok(Some(info)))) => {
                // Lagging timelines are published while recently active.
                if c.always || Lsn(info.remote_consistent_lsn) < Lsn(info.commit_lsn) {
                    out.push(info);
                }
            }
            Ok((_, Ok(None))) => {}
            Ok((c, Err(e))) => debug!(tl = %c.tl, error = %e, "broker: no info"),
            Err(e) => debug!(error = %e, "broker: a head read failed"),
        }
    }
    published.cursor.fetch_add(done, Ordering::Relaxed);
    out
}

async fn connect(
    cfg: &BrokerConfig,
) -> Result<BrokerServiceClient<tonic::transport::Channel>, Error> {
    let ep = tonic::transport::Endpoint::from_shared(cfg.endpoint.clone())
        .map_err(|e| Error::Io(format!("broker endpoint {}: {e}", cfg.endpoint)))?
        .connect_timeout(Duration::from_secs(5))
        .http2_keep_alive_interval(Duration::from_secs(5))
        .keep_alive_while_idle(true);
    let ch = ep
        .connect()
        .await
        .map_err(|e| Error::Io(format!("broker {}: {e}", cfg.endpoint)))?;
    Ok(BrokerServiceClient::new(ch))
}

/// Publish the set every `cfg.interval` over one `PublishSafekeeperInfo`
/// stream, until it fails.
async fn push_once<S: WalStore>(
    svc: &Arc<WalService<S>>,
    cfg: &BrokerConfig,
    published: &Arc<Published>,
) -> Result<(), Error> {
    let mut client = connect(cfg).await?;
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let publisher = {
        let (svc, cfg, published) = (svc.clone(), cfg.clone(), published.clone());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(cfg.interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                for info in publish_round(&svc, &cfg, &published).await {
                    if tx.send(info).await.is_err() {
                        return;
                    }
                }
            }
        })
    };
    let res = client
        .publish_safekeeper_info(tonic::Request::new(ReceiverStream::new(rx)))
        .await;
    publisher.abort();
    match res {
        Ok(_) => Err(Error::Io("broker: publish stream ended".into())),
        Err(e) => Err(Error::Io(format!("broker: publish: {e}"))),
    }
}

/// Answer discovery requests for timelines the store holds, until the
/// subscription fails. A request that cannot be answered (a store error) is
/// logged and skipped; it does not end the subscription.
async fn discover_once<S: WalStore>(
    svc: &Arc<WalService<S>>,
    cfg: &BrokerConfig,
    published: &Published,
) -> Result<(), Error> {
    let mut client = connect(cfg).await?;
    let request = SubscribeByFilterRequest {
        types: vec![TypeSubscription {
            r#type: MessageType::SafekeeperDiscoveryRequest as i32,
        }],
        tenant_timeline_id: Some(FilterTenantTimelineId {
            enabled: false,
            tenant_timeline_id: None,
        }),
    };
    let mut stream = client
        .subscribe_by_filter(request)
        .await
        .map_err(|e| Error::Io(format!("broker: subscribe: {e}")))?
        .into_inner();
    while let Some(msg) = stream
        .message()
        .await
        .map_err(|e| Error::Io(format!("broker: subscription: {e}")))?
    {
        if msg.r#type() != MessageType::SafekeeperDiscoveryRequest {
            continue;
        }
        let Some(ttid) = msg
            .safekeeper_discovery_request
            .and_then(|r| r.tenant_timeline_id)
        else {
            continue;
        };
        let tl = match parse_ttid(&ttid) {
            Ok(tl) => tl,
            Err(e) => {
                warn!(error = %e, "broker: bad discovery request");
                continue;
            }
        };
        let info = match timeline_info(svc, tl, cfg).await {
            Ok(Some(info)) => info,
            Ok(None) => continue, // not a timeline of ours
            Err(e) => {
                warn!(%tl, error = %e, "broker: cannot answer a discovery request");
                continue;
            }
        };
        // Published while it is asked about; the pageserver connects next,
        // and its reader keeps it published.
        published.ask(tl, Instant::now() + cfg.discovery_window);
        info!(%tl, commit = %Lsn(info.commit_lsn), "broker: answering discovery");
        client
            .publish_one(TypedMessage {
                r#type: MessageType::SafekeeperDiscoveryResponse as i32,
                safekeeper_timeline_info: None,
                safekeeper_discovery_request: None,
                safekeeper_discovery_response: Some(SafekeeperDiscoveryResponse {
                    safekeeper_id: info.safekeeper_id,
                    tenant_timeline_id: info.tenant_timeline_id,
                    commit_lsn: info.commit_lsn,
                    safekeeper_connstr: info.safekeeper_connstr,
                    availability_zone: info.availability_zone,
                    standby_horizon: 0,
                }),
            })
            .await
            .map_err(|e| Error::Io(format!("broker: discovery response: {e}")))?;
    }
    Err(Error::Io("broker: subscription ended".into()))
}

/// The broker tasks of one `loams-wal`; dropping it leaves them running.
#[derive(Debug)]
pub struct BrokerTasks {
    pub push: JoinHandle<()>,
    pub discover: JoinHandle<()>,
    /// The published set, shared by both.
    pub published: Arc<Published>,
}

/// Run publication and discovery until the process stops, reconnecting
/// after every error.
pub fn spawn<S: WalStore>(svc: Arc<WalService<S>>, cfg: BrokerConfig) -> BrokerTasks {
    info!(endpoint = %cfg.endpoint, advertise = %cfg.advertise_pg, "broker publication on");
    let published = Arc::new(Published::default());
    let push = {
        let (svc, cfg, published) = (svc.clone(), cfg.clone(), published.clone());
        tokio::spawn(async move {
            loop {
                if let Err(e) = push_once(&svc, &cfg, &published).await {
                    warn!(error = %e, "broker publication: reconnecting");
                }
                tokio::time::sleep(cfg.retry).await;
            }
        })
    };
    let discover = {
        let published = published.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = discover_once(&svc, &cfg, &published).await {
                    warn!(error = %e, "broker discovery: reconnecting");
                }
                tokio::time::sleep(cfg.retry).await;
            }
        })
    };
    BrokerTasks {
        push,
        discover,
        published,
    }
}

/// The distinct timelines in a batch of published infos (for tests and
/// logs).
pub fn timelines_of(infos: &[SafekeeperTimelineInfo]) -> HashSet<TimelineId> {
    infos
        .iter()
        .filter_map(|i| i.tenant_timeline_id.as_ref())
        .filter_map(|t| parse_ttid(t).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn broker_endpoint_must_be_plain_http_with_a_host() {
        assert!(parse_endpoint("http://127.0.0.1:50051").is_ok());
        assert!(parse_endpoint("http://broker.ns.svc:50051").is_ok());
        let no_scheme = parse_endpoint("127.0.0.1:50051").unwrap_err().to_string();
        assert!(no_scheme.contains("no scheme"), "{no_scheme}");
        let tls = parse_endpoint("https://broker:50051")
            .unwrap_err()
            .to_string();
        assert!(tls.contains("TLS") && tls.contains("Task 38"), "{tls}");
        assert!(parse_endpoint("grpc://broker:50051").is_err());
        assert!(parse_endpoint("http://").is_err());
        assert!(parse_endpoint("").is_err());
    }

    #[test]
    fn advertised_addresses_are_host_port() {
        let lo = sa("127.0.0.1:5454");
        for ok in [
            "127.0.0.1:5454",
            "wal.ns.svc:5454",
            "[::1]:5454",
            "10.0.0.5:1",
        ] {
            assert!(parse_advertise("--advertise-pg", ok, lo).is_ok(), "{ok}");
        }
        for bad in [
            "wal",
            "wal:",
            ":5454",
            "wal:0",
            "wal:70000",
            "::1:5454",
            "wal:x",
        ] {
            assert!(parse_advertise("--advertise-pg", bad, lo).is_err(), "{bad}");
        }
    }

    #[test]
    fn beyond_loopback_the_advertised_address_must_be_reachable() {
        let any = sa("0.0.0.0:5454");
        for bad in [
            "0.0.0.0:5454",
            "[::]:5454",
            "127.0.0.1:5454",
            "[::1]:5454",
            "localhost:5454",
        ] {
            let e = parse_advertise("--advertise-pg", bad, any)
                .unwrap_err()
                .to_string();
            assert!(
                e.contains("--advertise-pg") && e.contains("reach"),
                "{bad}: {e}"
            );
        }
        assert!(parse_advertise("--advertise-pg", "10.0.0.5:5454", any).is_ok());
        assert!(parse_advertise("--advertise-pg", "wal.ns.svc:5454", any).is_ok());
        // The default, the listener itself, is refused when it is 0.0.0.0.
        assert!(
            BrokerConfig::from_options("http://b:50051", 1, None, Some("10.0.0.5:7676"), any, any)
                .is_err()
        );
        assert!(
            BrokerConfig::from_options(
                "http://b:50051",
                1,
                Some("10.0.0.5:5454"),
                Some("10.0.0.5:7676"),
                any,
                any
            )
            .is_ok()
        );
    }

    #[test]
    fn ttids_round_trip_and_bad_lengths_are_refused() {
        let tl = TimelineId::new(Id([1; 16]), Id([2; 16]));
        assert_eq!(parse_ttid(&proto_ttid(tl)).unwrap(), tl);
        assert!(
            parse_ttid(&ProtoTtid {
                tenant_id: vec![1; 15],
                timeline_id: vec![2; 16]
            })
            .is_err()
        );
    }
}
