//! Transactional task operations on the same named leases as `DurableLeases`.
//!
//! The caller must bind the token holder to an authenticated principal. Each
//! state mutation reads the lease and state under the same KV transaction;
//! checking the lease in a separate request does not provide fencing.
use std::sync::Arc;

use loams_kv::{Store, TxnError, TxnOptions};

use crate::{
    Task, TaskState,
    durable::{TaskToken, decode, encode, key},
};

/// Stores tenant-scoped tasks using the existing KV transaction backend.
#[derive(Clone)]
pub struct DurableTasks {
    store: Store,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl std::fmt::Debug for DurableTasks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableTasks").finish_non_exhaustive()
    }
}

#[derive(Clone, Copy)]
enum Action {
    Consume(u64, u64),
    Finish,
}

pub(crate) fn decode_task(bytes: Option<Vec<u8>>) -> Result<Task, TxnError> {
    let bytes = bytes.ok_or_else(|| TxnError::Fatal("task not found".into()))?;
    serde_json::from_slice(&bytes).map_err(|_| TxnError::Fatal("invalid persisted task".into()))
}

pub(crate) fn encode_task(task: &Task) -> Result<Vec<u8>, TxnError> {
    serde_json::to_vec(task).map_err(|_| TxnError::Fatal("task encoding failed".into()))
}

impl DurableTasks {
    /// Uses a tenant-scoped store and a trusted runtime clock.
    #[must_use]
    pub fn new(store: Store, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        Self { store, clock }
    }

    /// Creates a ready task, failing if its id already exists.
    pub async fn create(&self, task: &Task) -> Result<(), TxnError> {
        let sk = key(task.id(), b'S');
        let value = encode_task(task)?;
        self.store
            .run(
                TxnOptions::pessimistic("imperial.task.create"),
                move |txn| {
                    let (sk, value) = (sk.clone(), value.clone());
                    Box::pin(async move {
                        if txn.get_for_update(&sk).await?.is_some() {
                            return Err(TxnError::Fatal("task already exists".into()));
                        }
                        txn.put(&sk, value).await
                    })
                },
            )
            .await
            .map(|_| ())
    }

    /// Acquires the task's lease and starts or recovers it atomically.
    ///
    /// Blocked tasks remain blocked; resolving and requeuing them requires a
    /// separate authorized operation that this initial adapter does not offer.
    pub async fn claim(
        &self,
        task: &str,
        holder: &str,
        ttl_ms: u64,
    ) -> Result<TaskToken, TxnError> {
        let task_name = task.to_owned();
        let (lk, sk, holder, clock) = (
            key(task, b'L'),
            key(task, b'S'),
            holder.to_owned(),
            self.clock.clone(),
        );
        self.store
            .run(TxnOptions::pessimistic("imperial.task.claim"), move |txn| {
                let (lk, sk, holder, clock) =
                    (lk.clone(), sk.clone(), holder.clone(), clock.clone());
                Box::pin(async move {
                    let mut lease = decode(txn.get_for_update(&lk).await?)?;
                    let mut state = decode_task(txn.get_for_update(&sk).await?)?;
                    let now = clock();
                    let token = lease
                        .claim(&holder, now, ttl_ms)
                        .map_err(|e| TxnError::Fatal(format!("lease refused: {e:?}")))?;
                    match state.state() {
                        TaskState::Ready => state.start(&lease, &token, now),
                        TaskState::Running => state.recover(&lease, &token, now),
                        _ => Err(crate::TaskError::InvalidTransition),
                    }
                    .map_err(|e| TxnError::Fatal(format!("task refused: {e:?}")))?;
                    txn.put(&lk, encode(&lease)?).await?;
                    txn.put(&sk, encode_task(&state)?).await?;
                    Ok(token)
                })
            })
            .await
            .map(|result| TaskToken {
                task: task_name,
                token: result.value,
            })
    }

    async fn apply(&self, task: &str, token: &TaskToken, action: Action) -> Result<(), TxnError> {
        if task != token.task {
            return Err(TxnError::Fatal("token belongs to another task".into()));
        }
        let (lk, sk, token, clock) = (
            key(task, b'L'),
            key(task, b'S'),
            token.token.clone(),
            self.clock.clone(),
        );
        self.store
            .run(
                TxnOptions::pessimistic("imperial.task.mutate"),
                move |txn| {
                    let (lk, sk, token, clock) =
                        (lk.clone(), sk.clone(), token.clone(), clock.clone());
                    Box::pin(async move {
                        let lease = decode(txn.get_for_update(&lk).await?)?;
                        let mut state = decode_task(txn.get_for_update(&sk).await?)?;
                        let now = clock();
                        match action {
                            Action::Consume(tokens, steps) => {
                                state.consume(&lease, &token, now, tokens, steps)
                            }
                            Action::Finish => state.finish(&lease, &token, now),
                        }
                        .map_err(|e| TxnError::Fatal(format!("task refused: {e:?}")))?;
                        txn.put(&sk, encode_task(&state)?).await
                    })
                },
            )
            .await
            .map(|_| ())
    }

    /// Atomically accounts token and step use under the current task lease.
    pub async fn consume(
        &self,
        task: &str,
        token: &TaskToken,
        tokens: u64,
        steps: u64,
    ) -> Result<(), TxnError> {
        self.apply(task, token, Action::Consume(tokens, steps))
            .await
    }

    /// Atomically marks a running task completed under the current lease.
    pub async fn finish(&self, task: &str, token: &TaskToken) -> Result<(), TxnError> {
        self.apply(task, token, Action::Finish).await
    }

    /// Reads the committed state, or `None` if this task has not been created.
    pub async fn get(&self, task: &str) -> Result<Option<Task>, TxnError> {
        self.store
            .get(&key(task, b'S'))
            .await?
            .map(|bytes| decode_task(Some(bytes)))
            .transpose()
    }
}
