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
//! - A timeline is served while a proposer streams to this instance, and
//!   after that while its pageserver has not caught up
//!   (`remote_consistent_lsn < commit_lsn`).
//!
//! The client is generated from the vendored `proto/storage_broker/
//! broker.proto` (Apache-2.0, Neon; see the repository's `NOTICE`).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

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
}

impl BrokerConfig {
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

/// Whether `info` is worth publishing: a proposer streams here, or the
/// pageserver has WAL left to persist.
fn served<S: WalStore>(svc: &WalService<S>, tl: TimelineId, info: &SafekeeperTimelineInfo) -> bool {
    svc.progress(tl).active || Lsn(info.remote_consistent_lsn) < Lsn(info.commit_lsn)
}

/// One round: the info of every timeline this instance serves.
pub async fn served_infos<S: WalStore>(
    svc: &WalService<S>,
    cfg: &BrokerConfig,
) -> Vec<SafekeeperTimelineInfo> {
    let mut out = Vec::new();
    for tl in svc.known_timelines() {
        match timeline_info(svc, tl, cfg).await {
            Ok(Some(info)) if served(svc, tl, &info) => out.push(info),
            Ok(_) => {}
            Err(e) => debug!(%tl, error = %e, "broker: no info"),
        }
    }
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

/// Publish every served timeline every `cfg.interval` over one
/// `PublishSafekeeperInfo` stream, until it fails.
async fn push_once<S: WalStore>(svc: &Arc<WalService<S>>, cfg: &BrokerConfig) -> Result<(), Error> {
    let mut client = connect(cfg).await?;
    let (tx, rx) = tokio::sync::mpsc::channel(256);
    let feeder = {
        let (svc, cfg) = (svc.clone(), cfg.clone());
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(cfg.interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                for info in served_infos(&svc, &cfg).await {
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
    feeder.abort();
    match res {
        Ok(_) => Err(Error::Io("broker: publish stream ended".into())),
        Err(e) => Err(Error::Io(format!("broker: publish: {e}"))),
    }
}

/// Answer discovery requests for timelines the store holds, until the
/// subscription fails.
async fn discover_once<S: WalStore>(
    svc: &Arc<WalService<S>>,
    cfg: &BrokerConfig,
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
        let Some(info) = timeline_info(svc, tl, cfg).await? else {
            continue; // not a timeline of ours
        };
        // Serve it from now on: the pageserver connects next.
        svc.note_timeline(tl);
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

/// Run publication and discovery until the process stops, reconnecting
/// after every error.
pub fn spawn<S: WalStore>(
    svc: Arc<WalService<S>>,
    cfg: BrokerConfig,
) -> (JoinHandle<()>, JoinHandle<()>) {
    info!(endpoint = %cfg.endpoint, advertise = %cfg.advertise_pg, "broker publication on");
    let push = {
        let (svc, cfg) = (svc.clone(), cfg.clone());
        tokio::spawn(async move {
            loop {
                if let Err(e) = push_once(&svc, &cfg).await {
                    warn!(error = %e, "broker: reconnecting");
                }
                tokio::time::sleep(cfg.retry).await;
            }
        })
    };
    let discover = tokio::spawn(async move {
        loop {
            if let Err(e) = discover_once(&svc, &cfg).await {
                debug!(error = %e, "broker discovery: reconnecting");
            }
            tokio::time::sleep(cfg.retry).await;
        }
    });
    (push, discover)
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
