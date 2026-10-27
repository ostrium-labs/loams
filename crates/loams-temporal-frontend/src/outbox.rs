//! The store's outbox as the relay's source.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use loams_temporal_resonate::{OutboxRequest, OutboxSource};
use loams_temporal_store::Store;
use tokio::sync::{Mutex, watch};

/// Claims outbox rows of every shard for one relay.
pub struct StoreOutbox {
    store: Arc<dyn Store>,
    shards: Vec<u32>,
    owner: String,
    lease: Duration,
    wake: Mutex<watch::Receiver<u64>>,
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
        }
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
