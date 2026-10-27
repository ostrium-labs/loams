//! Observer adapters' shared parts (design §63 §6.1, ME1 Task 3).
//!
//! Each product keeps its observer trait (`UsageObserver` in the House,
//! `ConnectionObserver` in the gateway, `ComputeLifecycleObserver` in
//! pg-control) and implements it in its own `meter` module on top of what is
//! here, so the dependency runs product → gate only (D202, D551):
//! - [`Emitter`]: attributes an event (subject and `tenantid` from the
//!   [`SubjectResolver`], never from tenant input, §63 §19) and sends it;
//! - [`ids`]: the deterministic ids of §63 §6.1, so a re-emission is a
//!   duplicate;
//! - [`ComputeTicker`]: Postgres compute time in UTC-minute ticks, whose ids
//!   depend only on the endpoint, the compute and the minute, so ticks
//!   re-emitted after a restart dedupe (D2182).

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::event::UsageEvent;
use crate::sink::MeterSink;

/// Who pays for an event: `subject` (`org_<id>`) and `tenantid`
/// (`<org>/<namespace>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribution {
    pub subject: String,
    pub tenant: String,
}

impl Attribution {
    /// The org is the billing unit (D65).
    pub fn for_org(org: &str, namespace: &str) -> Self {
        Self {
            subject: format!("org_{org}"),
            tenant: format!("{org}/{namespace}"),
        }
    }
}

/// Maps what a product knows (an org, when it keeps one, and a namespace) to
/// an [`Attribution`]. `None` means the event cannot be billed to anyone: it
/// is counted and not sent.
pub trait SubjectResolver: Send + Sync + fmt::Debug {
    fn resolve(&self, org: Option<&str>, namespace: &str) -> Option<Attribution>;
}

/// The org a product names, else one org for the whole deployment (a
/// single-org `loams dev` or standalone stack). With neither, nothing is
/// attributed.
#[derive(Clone, Debug, Default)]
pub struct DeploymentOrg {
    pub org: Option<String>,
}

impl DeploymentOrg {
    pub fn new(org: impl Into<String>) -> Self {
        Self {
            org: Some(org.into()),
        }
    }
}

impl SubjectResolver for DeploymentOrg {
    fn resolve(&self, org: Option<&str>, namespace: &str) -> Option<Attribution> {
        let org = org
            .filter(|o| !o.is_empty())
            .or(self.org.as_deref().filter(|o| !o.is_empty()))?;
        Some(Attribution::for_org(org, namespace))
    }
}

/// `loams://<cluster>/<role>/<node>` (§63 §6.1).
pub fn source_uri(cluster: &str, role: &str, node: &str) -> String {
    format!("loams://{cluster}/{role}/{node}")
}

/// Fills `source`, `subject` and `tenantid` and sends the event.
#[derive(Debug)]
pub struct Emitter {
    sink: Arc<dyn MeterSink>,
    resolver: Arc<dyn SubjectResolver>,
    source: String,
    unattributed: AtomicU64,
}

impl Emitter {
    pub fn new(
        sink: Arc<dyn MeterSink>,
        resolver: Arc<dyn SubjectResolver>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            sink,
            resolver,
            source: source.into(),
            unattributed: AtomicU64::new(0),
        }
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Events dropped because no subject could be resolved
    /// (`loams_events_dropped_total{reason="unattributed"}`).
    pub fn unattributed(&self) -> u64 {
        self.unattributed.load(Ordering::Relaxed)
    }

    /// The attribution of `namespace` (and `org` when the product knows it);
    /// `None` is counted as unattributed.
    pub fn resolve(&self, org: Option<&str>, namespace: &str) -> Option<Attribution> {
        let a = self.resolver.resolve(org, namespace);
        if a.is_none() {
            self.unattributed.fetch_add(1, Ordering::Relaxed);
        }
        a
    }

    /// Attributes `event` and sends it; any subject or tenant already on it
    /// is replaced (§63 §19).
    pub fn emit(&self, org: Option<&str>, namespace: &str, event: UsageEvent) {
        if let Some(a) = self.resolve(org, namespace) {
            self.emit_attributed(&a, event);
        }
    }

    pub fn emit_attributed(&self, a: &Attribution, event: UsageEvent) {
        self.sink.emit(
            event
                .source(self.source.as_str())
                .subject(a.subject.as_str())
                .tenant(a.tenant.as_str()),
        );
    }
}

/// Deterministic event ids (§63 §6.1).
pub mod ids {
    /// `house.query`: one event per worker lease, named by its first
    /// statement's query id and the lease's sequence under it.
    pub fn house_query(query_id: &str, lease_seq: u32) -> String {
        format!("{query_id}/{lease_seq}")
    }

    /// `house.insert`: one per insert and table.
    pub fn house_insert(query_id: &str, database: &str, table: &str) -> String {
        format!("{query_id}/insert/{database}/{table}")
    }

    /// `house.storage`: one per table and UTC day.
    pub fn house_storage(namespace: &str, database: &str, table: &str, day: u64) -> String {
        format!("{namespace}/{database}/{table}/{day}")
    }

    /// `house.compaction`: one per table and the compaction's end.
    pub fn house_compaction(namespace: &str, database: &str, table: &str, end_ns: i64) -> String {
        format!("{namespace}/{database}/{table}/compaction/{end_ns}")
    }

    /// A Postgres compute tick: endpoint, compute and UTC minute.
    pub fn compute_tick(endpoint: &str, compute: &str, minute: i64) -> String {
        format!("{endpoint}/{compute}/{minute}")
    }

    /// A gateway connection: the gateway process's boot (ns at start) and
    /// the connection's sequence number in it.
    pub fn connection(boot_ns: i64, seq: u64) -> String {
        format!("{boot_ns}/{seq}")
    }
}

/// Assigns connection sequence numbers within one process.
#[derive(Debug)]
pub struct Sequence {
    boot_ns: i64,
    next: AtomicU64,
}

impl Sequence {
    pub fn new(boot_ns: i64) -> Self {
        Self {
            boot_ns,
            next: AtomicU64::new(1),
        }
    }

    /// The process's boot time.
    pub fn starting_now() -> Self {
        Self::new(crate::event::system_time_ns(std::time::SystemTime::now()))
    }

    pub fn next_id(&self) -> String {
        ids::connection(self.boot_ns, self.next.fetch_add(1, Ordering::Relaxed))
    }
}

const MINUTE_MS: i64 = 60_000;

/// One minute (or the last part of one) of a compute's time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComputeTick {
    pub endpoint: String,
    pub compute: String,
    /// UTC minutes since the epoch.
    pub minute: i64,
    /// Compute units × ms in that minute (`cu_ms`).
    pub cu_ms: u64,
    /// The end of the measured interval, ms since the epoch.
    pub end_ms: i64,
    /// The compute ended in this minute.
    pub last: bool,
    pub attribution: Attribution,
    /// Extra dimensions the product passed at start (project, branch …).
    pub dims: Vec<(String, String)>,
}

impl ComputeTick {
    pub fn id(&self) -> String {
        ids::compute_tick(&self.endpoint, &self.compute, self.minute)
    }
}

#[derive(Clone, Debug)]
struct Running {
    attribution: Attribution,
    dims: Vec<(String, String)>,
    /// Quarter compute units (pg-control's `Cu`, §46 §7.4).
    quarters: u64,
    /// Measured up to here.
    since_ms: i64,
    /// Quarter-CU × ms measured in the current minute.
    acc: u64,
}

/// Turns compute lifecycle events into per-minute `postgres.compute` ticks
/// (§63 §6.3: every 60 s per running compute and at suspend). Sans-I/O: the
/// product calls [`ComputeTicker::tick`] from its own timer.
#[derive(Debug, Default)]
pub struct ComputeTicker {
    running: HashMap<(String, String), Running>,
}

impl ComputeTicker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn running(&self) -> usize {
        self.running.len()
    }

    /// A compute started (or, after a restart, is re-learned with its start
    /// time) at `at_ms` with `quarters` quarter-CUs.
    pub fn start(
        &mut self,
        endpoint: &str,
        compute: &str,
        attribution: Attribution,
        dims: Vec<(String, String)>,
        quarters: u16,
        at_ms: i64,
    ) {
        self.running.insert(
            (endpoint.to_owned(), compute.to_owned()),
            Running {
                attribution,
                dims,
                quarters: u64::from(quarters),
                since_ms: at_ms,
                acc: 0,
            },
        );
    }

    /// The compute was resized at `at_ms`: minutes completed before it are
    /// returned, and time after it counts at the new size.
    pub fn resize(
        &mut self,
        endpoint: &str,
        compute: &str,
        quarters: u16,
        at_ms: i64,
    ) -> Vec<ComputeTick> {
        let key = (endpoint.to_owned(), compute.to_owned());
        let mut out = Vec::new();
        if let Some(r) = self.running.get_mut(&key) {
            advance(&key, r, at_ms, &mut out);
            r.quarters = u64::from(quarters);
        }
        out
    }

    /// The compute ended at `at_ms`: every remaining minute, the partial one
    /// last (marked `last`).
    pub fn stop(&mut self, endpoint: &str, compute: &str, at_ms: i64) -> Vec<ComputeTick> {
        let key = (endpoint.to_owned(), compute.to_owned());
        let mut out = Vec::new();
        if let Some(mut r) = self.running.remove(&key) {
            advance(&key, &mut r, at_ms, &mut out);
            if r.acc > 0 || r.since_ms.rem_euclid(MINUTE_MS) != 0 {
                out.push(tick(
                    &key,
                    &r,
                    r.since_ms.div_euclid(MINUTE_MS),
                    r.since_ms,
                    true,
                ));
            } else if let Some(t) = out.last_mut() {
                t.last = true;
            }
        }
        out
    }

    /// Every minute that ended by `now_ms`, for every running compute.
    pub fn tick(&mut self, now_ms: i64) -> Vec<ComputeTick> {
        let mut out = Vec::new();
        for (key, r) in &mut self.running {
            advance(key, r, now_ms, &mut out);
        }
        out
    }
}

/// Measures `r` up to `to_ms`, closing each minute it completes.
fn advance(key: &(String, String), r: &mut Running, to_ms: i64, out: &mut Vec<ComputeTick>) {
    while r.since_ms < to_ms {
        let minute = r.since_ms.div_euclid(MINUTE_MS);
        let minute_end = (minute + 1) * MINUTE_MS;
        let seg_end = to_ms.min(minute_end);
        let ms = u64::try_from(seg_end - r.since_ms).unwrap_or(0);
        r.acc = r.acc.saturating_add(r.quarters.saturating_mul(ms));
        r.since_ms = seg_end;
        if seg_end == minute_end {
            out.push(tick(key, r, minute, minute_end, false));
            r.acc = 0;
        }
    }
}

fn tick(key: &(String, String), r: &Running, minute: i64, end_ms: i64, last: bool) -> ComputeTick {
    ComputeTick {
        endpoint: key.0.clone(),
        compute: key.1.clone(),
        minute,
        cu_ms: r.acc / 4,
        end_ms,
        last,
        attribution: r.attribution.clone(),
        dims: r.dims.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::MemorySink;

    fn attr() -> Attribution {
        Attribution::for_org("42", "7")
    }

    #[test]
    fn deployment_org_prefers_the_products_org() {
        let r = DeploymentOrg::new("dep");
        assert_eq!(
            r.resolve(Some("9"), "ns"),
            Some(Attribution::for_org("9", "ns"))
        );
        assert_eq!(
            r.resolve(Some(""), "ns"),
            Some(Attribution::for_org("dep", "ns"))
        );
        assert_eq!(r.resolve(None, "ns").unwrap().subject, "org_dep");
        assert_eq!(DeploymentOrg::default().resolve(None, "ns"), None);
    }

    #[test]
    fn emitter_sets_subject_from_resolver_and_counts_unattributed() {
        let sink = Arc::new(MemorySink::default());
        let e = Emitter::new(
            sink.clone(),
            Arc::new(DeploymentOrg::default()),
            source_uri("c", "house", "n1"),
        );
        let ev = UsageEvent::new(crate::event::types::HOUSE_QUERY, "q/1").subject("org_forged");
        e.emit(Some("42"), "7", ev.clone());
        e.emit(None, "7", ev);
        let got = sink.events();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].subject, "org_42");
        assert_eq!(got[0].tenant, "42/7");
        assert_eq!(got[0].source, "loams://c/house/n1");
        assert_eq!(e.unattributed(), 1);
    }

    #[test]
    fn house_query_event_id_is_stable() {
        assert_eq!(ids::house_query("q-7f3a9c", 1), "q-7f3a9c/1");
        assert_eq!(
            ids::house_query("q-7f3a9c", 1),
            ids::house_query("q-7f3a9c", 1)
        );
        assert_ne!(
            ids::house_query("q-7f3a9c", 1),
            ids::house_query("q-7f3a9c", 2)
        );
        assert_eq!(ids::house_storage("7", "db", "t", 20_380), "7/db/t/20380");
    }

    #[test]
    fn ticks_close_each_minute() {
        let mut t = ComputeTicker::new();
        // 1 CU (4 quarters) from 30 s into minute 10.
        t.start("ep", "c1", attr(), vec![], 4, 10 * MINUTE_MS + 30_000);
        assert!(t.tick(10 * MINUTE_MS + 59_000).is_empty());
        let ticks = t.tick(12 * MINUTE_MS + 5_000);
        assert_eq!(ticks.len(), 2);
        assert_eq!((ticks[0].minute, ticks[0].cu_ms), (10, 30_000));
        assert_eq!((ticks[1].minute, ticks[1].cu_ms), (11, 60_000));
        assert_eq!(ticks[1].id(), "ep/c1/11");
        let end = t.stop("ep", "c1", 12 * MINUTE_MS + 20_000);
        assert_eq!(end.len(), 1);
        assert_eq!(
            (end[0].minute, end[0].cu_ms, end[0].last),
            (12, 20_000, true)
        );
        assert_eq!(t.running(), 0);
    }

    #[test]
    fn resize_splits_the_minute() {
        let mut t = ComputeTicker::new();
        t.start("ep", "c1", attr(), vec![], 4, 0);
        assert!(t.resize("ep", "c1", 8, 30_000).is_empty());
        let ticks = t.tick(MINUTE_MS);
        // 30 s at 1 CU + 30 s at 2 CU.
        assert_eq!(ticks[0].cu_ms, 30_000 + 60_000);
    }

    #[test]
    fn quarter_cu_is_exact() {
        let mut t = ComputeTicker::new();
        t.start("ep", "c1", attr(), vec![], 1, 0);
        assert_eq!(t.tick(MINUTE_MS)[0].cu_ms, 15_000);
    }

    #[test]
    fn stop_on_a_minute_boundary_marks_the_last_full_minute() {
        let mut t = ComputeTicker::new();
        t.start("ep", "c1", attr(), vec![], 4, 0);
        let end = t.stop("ep", "c1", 2 * MINUTE_MS);
        assert_eq!(end.len(), 2);
        assert!(!end[0].last && end[1].last);
    }

    #[test]
    fn compute_tick_ids_dedupe_after_restart() {
        // Before the crash: started at minute 5 + 10 s, ticked through minute 7.
        let start = 5 * MINUTE_MS + 10_000;
        let mut before = ComputeTicker::new();
        before.start("ep", "c1", attr(), vec![], 4, start);
        let sent = before.tick(8 * MINUTE_MS);

        // After the restart pg-control re-learns the compute from its record
        // (same start) and ticks again: the same minutes get the same ids and
        // amounts, so the stream's dedupe drops them.
        let mut after = ComputeTicker::new();
        after.start("ep", "c1", attr(), vec![], 4, start);
        let resent = after.tick(9 * MINUTE_MS);
        assert_eq!(&resent[..sent.len()], &sent[..]);
        let ids: Vec<String> = resent.iter().map(ComputeTick::id).collect();
        assert_eq!(ids, ["ep/c1/5", "ep/c1/6", "ep/c1/7", "ep/c1/8"]);
    }

    #[test]
    fn connection_ids_are_unique_in_a_process() {
        let s = Sequence::new(77);
        assert_eq!(s.next_id(), "77/1");
        assert_eq!(s.next_id(), "77/2");
    }
}
