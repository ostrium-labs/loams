//! One-shot operations (design §77 §6, D1642; KV1 plan KV0 Task 2).
//!
//! Each is one [`Store::run`] with the default options and the store's
//! commit mode (async commit and one-phase commit on TiKV by default), so a
//! gateway's single-key command costs one transaction and no body of its
//! own. Every key is relative to the store's root, as in [`Txn`](crate::Txn).
//!
//! | Method | `op` | |
//! |---|---|---|
//! | [`Store::get`] | `kv.get` | a read-only transaction |
//! | [`Store::put`] | `kv.put` | a blind write |
//! | [`Store::insert`] | `kv.insert` | [`TxnError::AlreadyExists`] when present |
//! | [`Store::delete`] | `kv.delete` | a blind delete |
//! | [`Store::compare_and_put`] | `kv.cas` | a read and a conditional write in one transaction |
//!
//! [`Store::compare_and_put`] is atomic because its read and its write are
//! in one optimistic transaction: a write of the key committed after the
//! read makes its commit conflict (first committer wins), and the runner
//! then rereads and compares again. A mismatch writes nothing and reports
//! the value the transaction read, linearized at its start timestamp.

use std::sync::Arc;

use crate::{Store, Ts, TxnError, TxnOptions};

/// The `op` names of the one-shot operations.
pub mod op {
    pub const GET: &str = "kv.get";
    pub const PUT: &str = "kv.put";
    pub const INSERT: &str = "kv.insert";
    pub const DELETE: &str = "kv.delete";
    pub const CAS: &str = "kv.cas";
}

impl Store {
    /// Reads `key` in a read-only transaction.
    pub async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError> {
        let key: Arc<[u8]> = key.into();
        self.run(TxnOptions::new(op::GET), move |txn| {
            let key = key.clone();
            Box::pin(async move { txn.get(&key).await })
        })
        .await
        .map(|c| c.value)
    }

    /// Writes `value` at `key` and returns the commit timestamp. The 2 MiB
    /// bound of [`Txn::put`](crate::Txn::put) applies.
    pub async fn put(&self, key: &[u8], value: impl Into<Vec<u8>>) -> Result<Ts, TxnError> {
        let (key, value): (Arc<[u8]>, Arc<[u8]>) = (key.into(), value.into().into());
        self.run(TxnOptions::new(op::PUT), move |txn| {
            let (key, value) = (key.clone(), value.clone());
            Box::pin(async move { txn.put(&key, value.to_vec()).await })
        })
        .await
        .map(|c| c.commit_ts)
    }

    /// Writes `value` at `key` if it is absent; fails with
    /// [`TxnError::AlreadyExists`] if it is present.
    pub async fn insert(&self, key: &[u8], value: impl Into<Vec<u8>>) -> Result<Ts, TxnError> {
        let (key, value): (Arc<[u8]>, Arc<[u8]>) = (key.into(), value.into().into());
        self.run(TxnOptions::new(op::INSERT), move |txn| {
            let (key, value) = (key.clone(), value.clone());
            Box::pin(async move { txn.insert(&key, value.to_vec()).await })
        })
        .await
        .map(|c| c.commit_ts)
    }

    /// Deletes `key` (present or not) and returns the commit timestamp.
    pub async fn delete(&self, key: &[u8]) -> Result<Ts, TxnError> {
        let key: Arc<[u8]> = key.into();
        self.run(TxnOptions::new(op::DELETE), move |txn| {
            let key = key.clone();
            Box::pin(async move { txn.delete(&key).await })
        })
        .await
        .map(|c| c.commit_ts)
    }

    /// Writes `new` (or deletes, for `None`) when the current value equals `expected`
    /// (`None`: absent). `Ok(Ok(ts))` on success, `Ok(Err(current))` on a mismatch.
    pub async fn compare_and_put(
        &self,
        key: &[u8],
        expected: Option<&[u8]>,
        new: Option<&[u8]>,
    ) -> Result<Result<Ts, Option<Vec<u8>>>, TxnError> {
        let key: Arc<[u8]> = key.into();
        let expected: Option<Arc<[u8]>> = expected.map(Into::into);
        let new: Option<Arc<[u8]>> = new.map(Into::into);
        let committed = self
            .run(TxnOptions::new(op::CAS), move |txn| {
                let (key, expected, new) = (key.clone(), expected.clone(), new.clone());
                Box::pin(async move {
                    let current = txn.get(&key).await?;
                    if current.as_deref() != expected.as_deref() {
                        return Ok(Err(current));
                    }
                    match new {
                        Some(value) => txn.put(&key, value.to_vec()).await?,
                        None => txn.delete(&key).await?,
                    }
                    Ok(Ok(()))
                })
            })
            .await?;
        Ok(committed.value.map(|()| committed.commit_ts))
    }
}
