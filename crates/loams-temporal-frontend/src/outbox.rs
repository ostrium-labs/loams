//! The store's outbox as the relay's source.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_temporal_resonate::{OutboxRequest, OutboxSource};
use loams_temporal_store::Store;
use tokio::sync::{Mutex, mpsc, watch};

use crate::effects::{FOLLOW_UP_KIND, FollowUpRow};

/// A claimed follow-up row for the executor; it marks the row dispatched
/// when it has carried it out (D2089).
#[derive(Debug, Clone)]
pub struct ClaimedFollowUp {
    /// The outbox row's ID.
    pub id: i64,
    /// Claims so far, this one included.
    pub attempts: i32,
    /// What it asks for.
    pub row: FollowUpRow,
}

/// Claims outbox rows of every shard for one relay.
pub struct StoreOutbox {
    store: Arc<dyn Store>,
    shards: Vec<u32>,
    owner: String,
    lease: Duration,
    wake: Mutex<watch::Receiver<u64>>,
    follow_ups: Option<mpsc::UnboundedSender<ClaimedFollowUp>>,
}

impl std::fmt::Debug for StoreOutbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreOutbox")
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

impl StoreOutbox {
    /// The outbox of `shards` shards, claimed as `owner` for `lease`.
    pub fn new(
        store: Arc<dyn Store>,
        shards: u32,
        owner: impl Into<String>,
        lease: Duration,
    ) -> Self {
        let wake = store.outbox_wakeups();
        StoreOutbox {
            store,
            shards: (0..shards).collect(),
            owner: owner.into(),
            lease,
            wake: Mutex::new(wake),
            follow_ups: None,
        }
    }

    /// Hands follow-up rows to `tx` instead of the relay. Without it they
    /// are claimed and left alone (their lease runs out and they wait).
    pub fn with_follow_ups(mut self, tx: mpsc::UnboundedSender<ClaimedFollowUp>) -> Self {
        self.follow_ups = Some(tx);
        self
    }
}

#[async_trait]
impl OutboxSource for StoreOutbox {
    async fn claim(&self, limit: usize) -> Result<Vec<(i64, OutboxRequest)>, String> {
        let rows = self
            .store
            .claim_outbox(&self.shards, limit, self.lease, &self.owner)
            .await
            .map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(rows.len());
        let mut bad = Vec::new();
        for r in rows {
            if r.kind == FOLLOW_UP_KIND {
                match serde_json::from_value::<FollowUpRow>(r.request) {
                    Ok(row) => {
                        if let Some(tx) = &self.follow_ups {
                            let _ = tx.send(ClaimedFollowUp {
                                id: r.id,
                                attempts: r.attempts,
                                row,
                            });
                        }
                    }
                    Err(e) => {
                        tracing::error!(id = r.id, error = %e, "unreadable follow-up row; dropping it");
                        bad.push(r.id);
                    }
                }
                continue;
            }
            match serde_json::from_value::<OutboxRequest>(r.request) {
                Ok(req) => out.push((r.id, req)),
                Err(e) => {
                    tracing::error!(id = r.id, error = %e, "unreadable outbox row; dropping it");
                    bad.push(r.id);
                }
            }
        }
        if !bad.is_empty() {
            self.store
                .mark_dispatched(&bad)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(out)
    }

    async fn mark_dispatched(&self, ids: &[i64]) -> Result<(), String> {
        self.store
            .mark_dispatched(ids)
            .await
            .map_err(|e| e.to_string())
    }

    async fn wait(&self) {
        let mut rx = self.wake.lock().await;
        let _ = tokio::time::timeout(Duration::from_millis(500), rx.changed()).await;
    }
}
