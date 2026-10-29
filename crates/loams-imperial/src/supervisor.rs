//! Leader election and task cancellation with atomic fencing.
//!
//! The store must be tenant-scoped. The API layer must authorize leader
//! candidates and bind each token's holder to the authenticated caller.
//! Cancellation fences writes and marks state; cancelling the actual agent
//! stream remains the runtime's responsibility.
use crate::{
    LeaseToken,
    durable::{decode, encode, key},
    task_store::{decode_task, encode_task},
};
use loams_kv::{Store, TxnError, TxnOptions};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const LEADER_KEY: &[u8] = b"I";
const LEADER_CHECKPOINT_KEY: &[u8] = b"J";

/// A leader acquisition. The caller authenticates the token holder separately.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LeaderToken(LeaseToken);
impl LeaderToken {
    /// Returns the acquired fencing epoch.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.0.epoch()
    }
    /// Returns the principal the API must authenticate.
    #[must_use]
    pub fn holder(&self) -> &str {
        self.0.holder()
    }
}

/// Coordinates a single leader lease per tenant-scoped store.
#[derive(Clone)]
pub struct Supervisor {
    store: Store,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}
impl std::fmt::Debug for Supervisor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Supervisor").finish_non_exhaustive()
    }
}
impl Supervisor {
    /// Uses the same tenant-scoped store and trusted clock as the task store.
    #[must_use]
    pub fn new(store: Store, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        Self { store, clock }
    }

    /// Atomically acquires an absent or expired leader lease.
    pub async fn elect(&self, holder: &str, ttl_ms: u64) -> Result<LeaderToken, TxnError> {
        let (holder, clock) = (holder.to_owned(), self.clock.clone());
        self.store
            .run(
                TxnOptions::pessimistic("imperial.leader.elect"),
                move |txn| {
                    let (holder, clock) = (holder.clone(), clock.clone());
                    Box::pin(async move {
                        let mut lease = decode(txn.get_for_update(LEADER_KEY).await?)?;
                        let token = lease
                            .claim(&holder, clock(), ttl_ms)
                            .map_err(|e| TxnError::Fatal(format!("leader lease refused: {e:?}")))?;
                        txn.put(LEADER_KEY, encode(&lease)?).await?;
                        Ok(LeaderToken(token))
                    })
                },
            )
            .await
            .map(|result| result.value)
    }

    /// Renews a leader lease only with an advanced, changed durable checkpoint.
    pub async fn checkpoint(
        &self,
        token: &LeaderToken,
        ttl_ms: u64,
        progress: u64,
        checkpoint: &[u8],
    ) -> Result<(), TxnError> {
        let (token, checkpoint, clock) = (token.0.clone(), checkpoint.to_vec(), self.clock.clone());
        self.store
            .run(
                TxnOptions::pessimistic("imperial.leader.checkpoint"),
                move |txn| {
                    let (token, checkpoint, clock) =
                        (token.clone(), checkpoint.clone(), clock.clone());
                    Box::pin(async move {
                        let mut lease = decode(txn.get_for_update(LEADER_KEY).await?)?;
                        let now = clock();
                        lease
                            .check(&token, now)
                            .map_err(|e| TxnError::Fatal(format!("leader lease refused: {e:?}")))?;
                        if txn.get(LEADER_CHECKPOINT_KEY).await?.as_deref()
                            == Some(checkpoint.as_slice())
                        {
                            return Err(TxnError::Fatal("unchanged leader checkpoint".into()));
                        }
                        lease
                            .renew(&token, now, ttl_ms, progress)
                            .map_err(|e| TxnError::Fatal(format!("leader lease refused: {e:?}")))?;
                        txn.put(LEADER_CHECKPOINT_KEY, checkpoint).await?;
                        txn.put(LEADER_KEY, encode(&lease)?).await
                    })
                },
            )
            .await
            .map(|_| ())
    }

    /// Fences a task's lease and marks it cancelled in one transaction.
    ///
    /// The supervisor token is checked under lock in the same transaction.
    /// The runtime must also cancel the worker's execution stream after commit.
    pub async fn cancel(&self, task: &str, leader: &LeaderToken) -> Result<(), TxnError> {
        let (lk, sk, leader, clock) = (
            key(task, b'L'),
            key(task, b'S'),
            leader.0.clone(),
            self.clock.clone(),
        );
        self.store
            .run(
                TxnOptions::pessimistic("imperial.task.cancel"),
                move |txn| {
                    let (lk, sk, leader, clock) =
                        (lk.clone(), sk.clone(), leader.clone(), clock.clone());
                    Box::pin(async move {
                        let leader_lease = decode(txn.get_for_update(LEADER_KEY).await?)?;
                        let mut task_lease = decode(txn.get_for_update(&lk).await?)?;
                        let mut state = decode_task(txn.get_for_update(&sk).await?)?;
                        let now = clock();
                        leader_lease
                            .check(&leader, now)
                            .map_err(|e| TxnError::Fatal(format!("leader lease refused: {e:?}")))?;
                        state
                            .cancel_by_supervisor()
                            .map_err(|e| TxnError::Fatal(format!("task refused: {e:?}")))?;
                        task_lease
                            .revoke_by_supervisor(now)
                            .map_err(|e| TxnError::Fatal(format!("task lease refused: {e:?}")))?;
                        txn.put(&lk, encode(&task_lease)?).await?;
                        txn.put(&sk, encode_task(&state)?).await
                    })
                },
            )
            .await
            .map(|_| ())
    }
}
