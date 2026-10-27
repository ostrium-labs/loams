//! The retention sweeper (D2063, Q1163).
//!
//! Closed runs are deleted once their namespace's retention has passed since
//! they closed, and dispatched outbox rows once their TTL has passed. The
//! sweeper calls [`Store::purge_expired`] until a call deletes less than its
//! limits allow, then sleeps for the interval. Every replica may run one:
//! the Postgres purge skips rows another sweeper holds.
//!
//! Resonate's promises of a purged run are not touched here. Until the
//! fork's FW5 retention lands they stay in Resonate (X1.R16): late
//! deliveries for a run that no longer exists are acknowledged and dropped.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::{PurgeLimits, PurgeReport, Store};

/// How often and how much the sweeper purges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionPolicy {
    /// The pause between rounds.
    pub interval: Duration,
    /// One purge call's limits; a round repeats calls while they saturate.
    pub limits: PurgeLimits,
    /// The most calls in one round, so a large backlog never starves the
    /// database (the next round continues it).
    pub max_calls_per_round: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(60),
            limits: PurgeLimits::default(),
            max_calls_per_round: 50,
        }
    }
}

/// Runs one round: purge calls until one is not saturated, an error, or the
/// call budget is spent. Returns the round's totals.
pub async fn sweep_once(store: &dyn Store, policy: &RetentionPolicy) -> PurgeReport {
    let mut total = PurgeReport::default();
    for _ in 0..policy.max_calls_per_round.max(1) {
        match store.purge_expired(&policy.limits).await {
            Ok(r) => {
                total.runs += r.runs;
                total.events += r.events;
                total.run_outbox_rows += r.run_outbox_rows;
                total.dispatched_outbox_rows += r.dispatched_outbox_rows;
                if !r.saturated(&policy.limits) {
                    break;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "loams-temporal-store: retention purge failed");
                break;
            }
        }
    }
    total
}

/// Spawns the sweeper. It stops when `stop` turns true or its sender is
/// dropped.
pub fn spawn_sweeper(
    store: Arc<dyn Store>,
    policy: RetentionPolicy,
    mut stop: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if *stop.borrow() {
                return;
            }
            let r = sweep_once(&*store, &policy).await;
            if r.runs > 0 || r.dispatched_outbox_rows > 0 {
                tracing::debug!(
                    runs = r.runs,
                    events = r.events,
                    dispatched_outbox_rows = r.dispatched_outbox_rows,
                    "loams-temporal-store: retention round"
                );
            }
            tokio::select! {
                () = tokio::time::sleep(policy.interval) => {}
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return;
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Commit, EventRow, MemStore, NamespaceRecord, RunKey, STATUS_RUNNING, StartExecution,
        StartOutcome,
    };

    fn commit(first: i64, status: i32) -> Commit {
        Commit {
            events: vec![EventRow {
                event_id: first,
                batch_first_event_id: first,
                event_type: 1,
                blob: vec![1],
            }],
            state_blob: vec![],
            status,
            next_event_id: first + 1,
            outbox: vec![],
            visibility: None,
        }
    }

    #[tokio::test]
    async fn sweeper_purges_closed_runs_in_rounds() {
        let s = Arc::new(MemStore::new(4).expect("store"));
        s.create_namespace(NamespaceRecord {
            namespace_id: "ns".into(),
            loams_namespace_id: "L".into(),
            name: "default".into(),
            retention_secs: 0,
            state: 1,
            config: serde_json::json!({}),
        })
        .await
        .expect("namespace");
        for i in 0..5 {
            let key = RunKey::new("ns", format!("wf-{i}"), "run");
            let out = s
                .start(StartExecution {
                    key: key.clone(),
                    request_id: format!("r{i}"),
                    commit: commit(1, STATUS_RUNNING),
                })
                .await
                .expect("start");
            assert_eq!(out, StartOutcome::Started { version: 1 });
            s.commit(&key, 1, commit(2, 2)).await.expect("close");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        let policy = RetentionPolicy {
            interval: Duration::from_secs(3600),
            limits: PurgeLimits {
                max_runs: 2,
                ..PurgeLimits::default()
            },
            max_calls_per_round: 10,
        };
        let r = sweep_once(&*s, &policy).await;
        assert_eq!(r.runs, 5);
        assert_eq!(r.events, 10);
        let (tx, rx) = watch::channel(false);
        let h = spawn_sweeper(s.clone(), policy, rx);
        tx.send(true).expect("stop");
        h.await.expect("sweeper stops");
    }
}
