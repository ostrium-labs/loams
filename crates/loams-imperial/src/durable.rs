//! Atomic lease and checkpoint storage on the existing Loams KV transaction seam.
use crate::{Lease, LeaseToken};
use loams_kv::{Store, TxnError, TxnOptions};
use std::sync::Arc;

/// Persists fenced leases and checkpoints in a tenant-scoped KV store.
#[derive(Clone)]
pub struct DurableLeases {
    store: Store,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}
impl std::fmt::Debug for DurableLeases {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableLeases").finish_non_exhaustive()
    }
}
/// A fencing token bound to its named task or leader lease.
#[derive(Clone, Debug)]
pub struct TaskToken {
    pub(crate) task: String,
    pub(crate) token: LeaseToken,
}
impl TaskToken {
    /// The monotonically increasing epoch of this acquisition.
    pub fn epoch(&self) -> u64 {
        self.token.epoch()
    }
}
pub(crate) fn key(task: &str, tag: u8) -> Vec<u8> {
    let mut key = vec![tag];
    key.extend_from_slice(task.as_bytes());
    key
}
pub(crate) fn decode(bytes: Option<Vec<u8>>) -> Result<Lease, TxnError> {
    bytes
        .map(|v| {
            serde_json::from_slice(&v)
                .map_err(|_| TxnError::Fatal("invalid persisted lease".into()))
        })
        .unwrap_or_else(|| Ok(Lease::new()))
}
pub(crate) fn encode(lease: &Lease) -> Result<Vec<u8>, TxnError> {
    serde_json::to_vec(lease).map_err(|_| TxnError::Fatal("lease encoding failed".into()))
}
impl DurableLeases {
    /// Uses a trusted runtime clock, sampled after acquiring the transaction lock.
    pub fn new(store: Store, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        Self { store, clock }
    }
    /// Acquires a task or leader lease atomically, retaining fencing tombstones.
    pub async fn claim(
        &self,
        task: &str,
        holder: &str,
        ttl_ms: u64,
    ) -> Result<TaskToken, TxnError> {
        let task_name = task.to_owned();
        let (lease_key, holder, clock) = (key(task, b'L'), holder.to_owned(), self.clock.clone());
        self.store
            .run(TxnOptions::pessimistic("imperial.claim"), move |txn| {
                let (lease_key, holder, clock) = (lease_key.clone(), holder.clone(), clock.clone());
                Box::pin(async move {
                    let mut lease = decode(txn.get_for_update(&lease_key).await?)?;
                    let token = lease
                        .claim(&holder, clock(), ttl_ms)
                        .map_err(|e| TxnError::Fatal(format!("lease refused: {e:?}")))?;
                    txn.put(&lease_key, encode(&lease)?).await?;
                    Ok(token)
                })
            })
            .await
            .map(|v| TaskToken {
                task: task_name,
                token: v.value,
            })
    }
    /// Commits a changed checkpoint and progress-gated renewal in one transaction.
    pub async fn checkpoint(
        &self,
        task: &str,
        token: &TaskToken,
        ttl_ms: u64,
        progress: u64,
        value: &[u8],
    ) -> Result<(), TxnError> {
        anyhow_task(task, token)?;
        let (lk, tk, token, value, clock) = (
            key(task, b'L'),
            key(task, b'T'),
            token.token.clone(),
            value.to_vec(),
            self.clock.clone(),
        );
        self.store
            .run(TxnOptions::pessimistic("imperial.checkpoint"), move |txn| {
                let (lk, tk, token, value, clock) = (
                    lk.clone(),
                    tk.clone(),
                    token.clone(),
                    value.clone(),
                    clock.clone(),
                );
                Box::pin(async move {
                    let mut lease = decode(txn.get_for_update(&lk).await?)?;
                    let now = clock();
                    lease
                        .check(&token, now)
                        .map_err(|e| TxnError::Fatal(format!("lease refused: {e:?}")))?;
                    if txn.get(&tk).await?.as_deref() == Some(value.as_slice()) {
                        return Err(TxnError::Fatal("unchanged checkpoint".into()));
                    }
                    lease
                        .renew(&token, now, ttl_ms, progress)
                        .map_err(|e| TxnError::Fatal(format!("lease refused: {e:?}")))?;
                    txn.put(&tk, value).await?;
                    txn.put(&lk, encode(&lease)?).await
                })
            })
            .await
            .map(|_| ())
    }
    /// Revokes ownership and persists the advanced fencing epoch without deleting it.
    pub async fn revoke(&self, task: &str, token: &TaskToken) -> Result<(), TxnError> {
        anyhow_task(task, token)?;
        let (lk, token, clock) = (key(task, b'L'), token.token.clone(), self.clock.clone());
        self.store
            .run(TxnOptions::pessimistic("imperial.revoke"), move |txn| {
                let (lk, token, clock) = (lk.clone(), token.clone(), clock.clone());
                Box::pin(async move {
                    let mut lease = decode(txn.get_for_update(&lk).await?)?;
                    lease
                        .revoke(&token, clock())
                        .map_err(|e| TxnError::Fatal(format!("lease refused: {e:?}")))?;
                    txn.put(&lk, encode(&lease)?).await
                })
            })
            .await
            .map(|_| ())
    }
    /// Reads the last atomically committed checkpoint.
    pub async fn read_checkpoint(&self, task: &str) -> Result<Option<Vec<u8>>, TxnError> {
        self.store.get(&key(task, b'T')).await
    }
}

fn anyhow_task(task: &str, token: &TaskToken) -> Result<(), TxnError> {
    if task != token.task {
        return Err(TxnError::Fatal("token belongs to another task".into()));
    }
    Ok(())
}
