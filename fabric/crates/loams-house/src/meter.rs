//! Usage events from the House (design §63 §6.3, ME1 Task 3): a
//! [`UsageObserver`] that turns each [`UsageEvent`] into a
//! `io.loams.dev.usage.house.*.v1` event for the meter, then hands it on to
//! the observer it wraps.
//!
//! Ids follow §63 §6.1 ([`ids`]): `house.query` is `<query_id>/<lease_seq>`
//! (one worker lease per first-statement query id, so the sequence is 1),
//! `house.insert` `<query_id>/insert/<db>/<table>`, `house.storage`
//! `<ns>/<db>/<table>/<utc_day>`, `house.compaction`
//! `<ns>/<db>/<table>/compaction/<end_ns>`. The House keeps no org per
//! namespace: the [`Emitter`]'s resolver supplies it.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use loams_meter_gate::adapters::{Emitter, ids};
use loams_meter_gate::event::{self as mevent, types};

use crate::usage::{NoUsageObserver, UsageEvent, UsageObserver};

/// Emits a meter event per House usage event.
pub struct MeterUsageObserver {
    emitter: Arc<Emitter>,
    inner: Arc<dyn UsageObserver>,
}

impl MeterUsageObserver {
    pub fn new(emitter: Arc<Emitter>) -> Self {
        Self::wrapping(emitter, Arc::new(NoUsageObserver))
    }

    pub fn wrapping(emitter: Arc<Emitter>, inner: Arc<dyn UsageObserver>) -> Self {
        Self { emitter, inner }
    }
}

impl fmt::Debug for MeterUsageObserver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MeterUsageObserver")
            .field("source", &self.emitter.source())
            .field("inner", &self.inner)
            .finish()
    }
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The meter event for one House usage event.
pub fn to_meter_event(event: &UsageEvent) -> mevent::UsageEvent {
    match event {
        UsageEvent::Query(q) => {
            let mut e =
                mevent::UsageEvent::new(types::HOUSE_QUERY, ids::house_query(&q.query_id, 1))
                    .at(q.at)
                    .text("namespace", &q.namespace)
                    .num("bytes_scanned", q.bytes_scanned)
                    .num("rows_read", q.rows_read)
                    .num("result_bytes", q.result_bytes)
                    .num("result_rows", q.result_rows)
                    .num("worker_ms", millis(q.worker_time))
                    .num("statements", u64::from(q.statements))
                    .int("code", i64::from(q.code));
            // An in-process worker is not measured: no `cpu_ms` rather than 0.
            if let Some(cpu) = q.cpu {
                e = e.num("cpu_ms", millis(cpu));
            }
            e
        }
        UsageEvent::Insert(i) => mevent::UsageEvent::new(
            types::HOUSE_INSERT,
            ids::house_insert(&i.query_id, &i.database, &i.table),
        )
        .at(i.at)
        .text("namespace", &i.namespace)
        .text("database", &i.database)
        .text("table", &i.table)
        .num("rows", i.rows)
        .num("bytes", i.bytes),
        UsageEvent::Storage(s) => mevent::UsageEvent::new(
            types::HOUSE_STORAGE,
            ids::house_storage(&s.namespace, &s.database, &s.table, s.day),
        )
        .at(s.at)
        .text("namespace", &s.namespace)
        .text("database", &s.database)
        .text("table", &s.table)
        .num("bytes", s.bytes)
        .num("day", s.day),
        UsageEvent::Compaction(c) => {
            let end_ns = mevent::system_time_ns(c.at);
            let mut e = mevent::UsageEvent::new(
                types::HOUSE_COMPACTION,
                ids::house_compaction(&c.namespace, &c.database, &c.table, end_ns),
            )
            .at_ns(end_ns)
            .text("namespace", &c.namespace)
            .text("database", &c.database)
            .text("table", &c.table)
            .num("bytes_scanned", c.bytes_scanned)
            .num("worker_ms", millis(c.worker_time));
            if let Some(cpu) = c.cpu {
                e = e.num("cpu_ms", millis(cpu));
            }
            e
        }
    }
}

impl UsageObserver for MeterUsageObserver {
    fn on_usage(&self, event: &UsageEvent) {
        self.emitter
            .emit(None, event.namespace(), to_meter_event(event));
        self.inner.on_usage(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::{InsertUsage, QueryUsage, StorageUsage};
    use loams_meter_gate::adapters::{DeploymentOrg, source_uri};
    use loams_meter_gate::sink::MemorySink;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn observer() -> (Arc<MemorySink>, MeterUsageObserver) {
        let sink = Arc::new(MemorySink::default());
        let emitter = Arc::new(Emitter::new(
            sink.clone(),
            Arc::new(DeploymentOrg::new("42")),
            source_uri("local", "house", "n1"),
        ));
        (sink, MeterUsageObserver::new(emitter))
    }

    #[test]
    fn house_query_event_id_is_stable() {
        let q = UsageEvent::Query(QueryUsage {
            namespace: "7".into(),
            query_id: "q-7f3a9c".into(),
            cpu: Some(Duration::from_millis(1840)),
            bytes_scanned: 734_003_200,
            worker_time: Duration::from_millis(2210),
            statements: 1,
            at: UNIX_EPOCH + Duration::from_secs(1_792_401_243),
            ..QueryUsage::default()
        });
        let a = to_meter_event(&q);
        let b = to_meter_event(&q);
        assert_eq!(a.id, "q-7f3a9c/1");
        assert_eq!(a.encode(), b.encode());
        assert_eq!(a.data["cpu_ms"], 1840);
        assert_eq!(a.data["worker_ms"], 2210);
    }

    #[test]
    fn unmeasured_cpu_is_omitted() {
        let q = UsageEvent::Query(QueryUsage {
            namespace: "7".into(),
            query_id: "q1".into(),
            ..QueryUsage::default()
        });
        assert!(to_meter_event(&q).data.get("cpu_ms").is_none());
    }

    #[test]
    fn every_kind_is_attributed_and_valid() {
        let (sink, obs) = observer();
        let now = SystemTime::now();
        obs.on_usage(&UsageEvent::Query(QueryUsage {
            namespace: "7".into(),
            query_id: "q1".into(),
            at: now,
            ..QueryUsage::default()
        }));
        obs.on_usage(&UsageEvent::Insert(InsertUsage {
            namespace: "7".into(),
            query_id: "q2".into(),
            database: "db".into(),
            table: "t".into(),
            rows: 10,
            bytes: 100,
            at: now,
        }));
        obs.on_usage(&UsageEvent::Storage(StorageUsage {
            namespace: "7".into(),
            database: "db".into(),
            table: "t".into(),
            bytes: 5,
            day: 20_388,
            at: now,
        }));
        let events = sink.events();
        assert_eq!(events.len(), 3);
        for e in &events {
            e.validate().unwrap();
            assert_eq!(e.subject, "org_42");
            assert_eq!(e.tenant, "42/7");
        }
        assert_eq!(events[1].id, "q2/insert/db/t");
        assert_eq!(events[2].id, "7/db/t/20388");
    }
}
