//! The gate's lease client (design §63 §11.2; ME1 follow-up (b), D2209):
//! ME-gate's [`LeaseSource`] over the counter owners' `LeaseService`.
//!
//! - A request is routed by the subject's partition ([`partition_of`]). When
//!   this process owns the partition, the local [`LeaseServer`] answers it
//!   with no network hop (every partition on a single node).
//! - Otherwise it goes to the partition's owner: the holder of the
//!   partition's lease, whose holder name carries its cluster address
//!   ([`crate::run::holder_addr`]). The call is `LeaseService.AcquireLease`
//!   in the Connect protocol's JSON form, over the owner's cluster listener.
//!   The transport is a [`LeaseTransport`], so it can move (design 83: HTTP/3
//!   with mTLS) without touching the routing.
//! - Every grant passes the [`EpochFence`]: a grant from an owner epoch older
//!   than one already seen for the partition comes from a fenced owner and
//!   is dropped (`stale_epoch_grant_ignored`).
//! - [`LeaseSource::acquire`] is synchronous and bounded by its deadline.
//!   The client runs its own single-threaded runtime on its own thread, so
//!   it may be called from a gate's refresh thread or from inside another
//!   runtime alike.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_meter_core::{Amount, FeatureKey, LeaseId, LeaseReport, SubjectKey, UtcNanos};
use loams_meter_gate::gate::{
    self as g, LeaseGrant as GateGrant, LeaseRequest as GateRequest, LeaseSource, SourceError,
};
use serde_json::{Value, json};

use crate::authz::Caller;
use crate::customers::Clock;
use crate::lease_service::{EpochFence, Granted, LeaseServer};
use crate::owner::{PartitionLeases, partition_of};
use crate::run::{CLUSTER_SERVICE, holder_addr};

/// The Connect path of `AcquireLease`.
pub const ACQUIRE_PATH: &str = "/loams.meter.internal.v1.LeaseService/AcquireLease";

/// A lease as the owner answered it, before the gate's conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireLease {
    pub lease_id: String,
    pub amount: Amount,
    pub has_access: bool,
    pub expires_at: UtcNanos,
    pub refresh_after: UtcNanos,
    pub owner_epoch: u64,
    pub refusal_reason: String,
    pub counting: bool,
    pub overage: bool,
    pub fail_open: Amount,
}

impl From<&Granted> for WireLease {
    fn from(g: &Granted) -> Self {
        WireLease {
            lease_id: g
                .grant
                .id
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default(),
            amount: g.grant.amount,
            has_access: g.grant.has_access,
            expires_at: g.grant.expires_at,
            refresh_after: g.grant.refresh_after,
            owner_epoch: g.grant.owner_epoch,
            refusal_reason: g
                .grant
                .refusal
                .map(|r| r.reason().to_string())
                .unwrap_or_default(),
            counting: g.counting,
            overage: g.overage,
            fail_open: g.fail_open,
        }
    }
}

/// How a request reaches a remote owner.
#[async_trait]
pub trait LeaseTransport: Send + Sync + fmt::Debug {
    /// `AcquireLease` on the owner at `addr` with the Connect JSON `body`;
    /// answers the response body (`AcquireLeaseResponse` JSON), or an error
    /// (a Connect error included).
    async fn acquire(&self, addr: &str, body: Value) -> Result<Value, String>;
}

/// [`LeaseTransport`] over HTTP/1.1 with the Connect protocol's JSON
/// encoding (the cluster listener of M1).
#[derive(Debug, Clone)]
pub struct HttpTransport(reqwest::Client);

impl HttpTransport {
    pub fn new(timeout: Duration) -> Result<Self, String> {
        reqwest::Client::builder()
            .connect_timeout(timeout)
            .timeout(timeout)
            .build()
            .map(HttpTransport)
            .map_err(|e| e.to_string())
    }
}

#[async_trait]
impl LeaseTransport for HttpTransport {
    async fn acquire(&self, addr: &str, body: Value) -> Result<Value, String> {
        let response = self
            .0
            .post(format!("http://{addr}{ACQUIRE_PATH}"))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = response.status();
        let body: Value = response.json().await.map_err(|e| e.to_string())?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(format!("{status}: {body}"))
        }
    }
}

/// Integer base units of a decimal amount, floored, never negative
/// (D2180).
fn units(a: Amount) -> u64 {
    crate::run::gate_units(a)
}

fn refusal(reason: &str) -> Option<g::RefusalReason> {
    Some(match reason {
        "" => return None,
        "entitlement_exhausted" => g::RefusalReason::EntitlementExhausted,
        "payment_method_required" => g::RefusalReason::PaymentMethodRequired,
        "spend_cap_reached" => g::RefusalReason::SpendCapReached,
        "not_entitled" => g::RefusalReason::NotEntitled,
        _ => g::RefusalReason::MeterUnavailable,
    })
}

fn until(t: UtcNanos, now: UtcNanos) -> Duration {
    Duration::from_nanos(u64::try_from(t.0.saturating_sub(now.0)).unwrap_or(0))
}

/// The gate's grant for `w`, with times relative to `now`.
pub fn to_gate(w: &WireLease, now: UtcNanos) -> GateGrant {
    GateGrant {
        lease: g::LeaseId::parse(&w.lease_id).unwrap_or(g::LeaseId::NONE),
        amount: units(w.amount),
        has_access: w.has_access,
        refusal: if w.has_access {
            None
        } else {
            refusal(&w.refusal_reason).or(Some(g::RefusalReason::MeterUnavailable))
        },
        counting: w.counting,
        overage: w.overage,
        ttl: until(w.expires_at, now),
        refresh_after: until(w.refresh_after, now),
        owner_epoch: w.owner_epoch,
        fail_open: units(w.fail_open),
    }
}

/// The Connect JSON of an `AcquireLeaseRequest`.
pub fn request_json(r: &OwnedRequest, known_epoch: u64) -> Value {
    let held: Vec<Value> = r
        .held
        .iter()
        .map(|h| {
            json!({
                "leaseId": h.lease.to_string(),
                "granted": h.granted.to_string(),
                "spent": h.spent.to_string(),
            })
        })
        .collect();
    json!({
        "subject": r.subject,
        "feature": r.feature,
        "node": r.node,
        "want": r.want.to_string(),
        "held": held,
        "knownEpoch": known_epoch.to_string(),
    })
}

fn str_field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn u64_field(v: &Value, k: &str) -> u64 {
    match v.get(k) {
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

fn amount_field(v: &Value, k: &str) -> Result<Amount, String> {
    match str_field(v, k) {
        "" => Ok(Amount::ZERO),
        s => Amount::parse(s).map_err(|e| format!("{k}: {e}")),
    }
}

fn time_field(v: &Value, k: &str) -> Result<UtcNanos, String> {
    match str_field(v, k) {
        "" => Ok(UtcNanos(0)),
        s => UtcNanos::from_rfc3339(s).map_err(|e| format!("{k}: {e}")),
    }
}

/// Parses an `AcquireLeaseResponse` in Connect JSON.
pub fn parse_response(body: &Value) -> Result<WireLease, String> {
    let l = body.get("lease").ok_or("no lease in the answer")?;
    Ok(WireLease {
        lease_id: str_field(l, "leaseId").to_string(),
        amount: amount_field(l, "amount")?,
        has_access: l.get("hasAccess").and_then(Value::as_bool).unwrap_or(false),
        expires_at: time_field(l, "expireTime")?,
        refresh_after: time_field(l, "refreshTime")?,
        owner_epoch: u64_field(l, "ownerEpoch"),
        refusal_reason: str_field(l, "refusalReason").to_string(),
        counting: l.get("counting").and_then(Value::as_bool).unwrap_or(false),
        overage: l.get("overage").and_then(Value::as_bool).unwrap_or(false),
        fail_open: amount_field(l, "failOpen")?,
    })
}

/// A gate's request, owned, for the client's thread.
#[derive(Clone, Debug)]
pub struct OwnedRequest {
    pub subject: String,
    pub feature: String,
    pub node: String,
    pub want: Amount,
    pub held: Vec<LeaseReport>,
}

impl OwnedRequest {
    fn of(r: &GateRequest<'_>) -> Result<Self, String> {
        let held = match &r.held {
            Some(h) if h.lease != g::LeaseId::NONE => vec![LeaseReport {
                lease: LeaseId::new(h.lease.to_string()).map_err(|e| e.to_string())?,
                granted: Amount::from_units(i64::try_from(h.granted).unwrap_or(i64::MAX)),
                spent: Amount::from_units(i64::try_from(h.spent).unwrap_or(i64::MAX)),
            }],
            _ => Vec::new(),
        };
        Ok(OwnedRequest {
            subject: r.subject.to_string(),
            feature: r.feature.to_string(),
            node: r.node.to_string(),
            want: Amount::from_units(i64::try_from(r.want).unwrap_or(i64::MAX)),
            held,
        })
    }
}

/// Routes and sends lease requests (the async half of [`LeaseClient`]).
pub struct LeaseRouter {
    pub partitions: u32,
    /// This process's lease server, when it runs one.
    pub local: Option<Arc<LeaseServer>>,
    /// The partition leases (who owns what).
    pub owners: Arc<dyn PartitionLeases>,
    pub transport: Arc<dyn LeaseTransport>,
    pub fence: Arc<EpochFence>,
    pub clock: Clock,
    /// Where the meter has a policy (D2400): a pair outside it gets
    /// [`unmetered_grant`] without asking any owner. `None`: every pair asks.
    pub policies: Option<Arc<loams_meter_gate::scope::Policies>>,
}

/// How long an unmetered grant is good for before the gate asks again (so
/// a new entitlement takes effect within about this long).
pub const UNMETERED_REFRESH: Duration = Duration::from_secs(10);

/// The grant for a pair the meter has no policy for: allowed, counting
/// only, no amount limit, never fails closed. The gate caches it like any
/// lease, so such a request costs a lease hit.
pub fn unmetered_grant() -> GateGrant {
    GateGrant {
        lease: g::LeaseId::NONE,
        amount: u64::MAX / 4,
        has_access: true,
        refusal: None,
        counting: true,
        overage: false,
        ttl: UNMETERED_REFRESH * 3,
        refresh_after: UNMETERED_REFRESH,
        owner_epoch: 0,
        fail_open: u64::MAX / 4,
    }
}

impl fmt::Debug for LeaseRouter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseRouter")
            .field("partitions", &self.partitions)
            .finish_non_exhaustive()
    }
}

impl LeaseRouter {
    async fn owner_addr(&self, partition: u32) -> Result<String, SourceError> {
        let wanted = crate::owner::assign::partition_key(partition);
        let now = self.owners.now_ms();
        let holder = self
            .owners
            .list(crate::owner::assign::LEASE_PREFIX)
            .await
            .map_err(|e| SourceError::Unavailable(e.to_string()))?
            .into_iter()
            .find(|(k, v)| *k == wanted && v.is_held_at(now))
            .and_then(|(_, v)| v.holder)
            .ok_or_else(|| {
                SourceError::Unavailable(format!("partition {partition} has no owner right now"))
            })?;
        holder_addr(&holder).map(str::to_string).ok_or_else(|| {
            SourceError::Unavailable(format!(
                "partition {partition}'s owner has no cluster address"
            ))
        })
    }

    /// One `AcquireLease`, routed, fenced and converted.
    pub async fn acquire(&self, r: &OwnedRequest) -> Result<GateGrant, SourceError> {
        if let Some(p) = &self.policies
            && !p.contains(&r.subject, &r.feature)
        {
            return Ok(unmetered_grant());
        }
        let partition = partition_of(&r.subject, self.partitions);
        let known = self.fence.known(partition);
        let local = self
            .local
            .as_ref()
            .filter(|l| l.owned().iter().any(|(p, _)| *p == partition));
        let wire = match local {
            Some(server) => {
                let subject = SubjectKey::new(r.subject.clone())
                    .map_err(|e| SourceError::Unavailable(e.to_string()))?;
                let feature = FeatureKey::new(r.feature.clone())
                    .map_err(|e| SourceError::Unavailable(e.to_string()))?;
                let granted = server
                    .acquire(
                        &Caller::Service(CLUSTER_SERVICE.to_string()),
                        subject,
                        feature,
                        r.node.clone(),
                        r.want,
                        r.held.clone(),
                        known,
                    )
                    .await
                    .map_err(|e| SourceError::Unavailable(e.to_string()))?;
                WireLease::from(&granted)
            }
            None => {
                let addr = self.owner_addr(partition).await?;
                let body = self
                    .transport
                    .acquire(&addr, request_json(r, known))
                    .await
                    .map_err(SourceError::Unavailable)?;
                parse_response(&body).map_err(SourceError::Unavailable)?
            }
        };
        if !self.fence.admit(partition, wire.owner_epoch) {
            return Err(SourceError::Unavailable(format!(
                "a grant from owner epoch {} of partition {partition}, older than one seen",
                wire.owner_epoch
            )));
        }
        Ok(to_gate(&wire, (self.clock)()))
    }
}

type Reply = std::sync::mpsc::Sender<Result<GateGrant, SourceError>>;

/// ME-gate's [`LeaseSource`] over [`LeaseRouter`], on its own thread.
pub struct LeaseClient {
    jobs: tokio::sync::mpsc::UnboundedSender<(OwnedRequest, Reply)>,
}

impl fmt::Debug for LeaseClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeaseClient").finish_non_exhaustive()
    }
}

impl LeaseClient {
    /// Starts the client's thread. It ends when the client is dropped.
    pub fn start(router: LeaseRouter) -> std::io::Result<Self> {
        let (jobs, mut rx) = tokio::sync::mpsc::unbounded_channel::<(OwnedRequest, Reply)>();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let router = Arc::new(router);
        std::thread::Builder::new()
            .name("meter-lease-client".into())
            .spawn(move || {
                runtime.block_on(async move {
                    while let Some((req, reply)) = rx.recv().await {
                        let router = router.clone();
                        tokio::spawn(async move {
                            // The caller may have given up (its deadline).
                            let _ = reply.send(router.acquire(&req).await);
                        });
                    }
                });
            })?;
        Ok(LeaseClient { jobs })
    }
}

impl LeaseSource for LeaseClient {
    fn acquire(&self, req: &GateRequest<'_>, deadline: Duration) -> Result<GateGrant, SourceError> {
        let owned = OwnedRequest::of(req).map_err(SourceError::Unavailable)?;
        let (tx, rx) = std::sync::mpsc::channel();
        self.jobs
            .send((owned, tx))
            .map_err(|_| SourceError::Unavailable("the lease client stopped".into()))?;
        match rx.recv_timeout(deadline) {
            Ok(answer) => answer,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(SourceError::Timeout),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err(SourceError::Unavailable("the lease client stopped".into()))
            }
        }
    }
}
