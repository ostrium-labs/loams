//! The matching fast path (D1505): when the relay creates a workflow or
//! activity task, the matcher hears of it at once instead of waiting for
//! Resonate's delivery. Resonate still grants every claim.

use std::sync::Arc;

use async_trait::async_trait;
use loams_temporal_matching::{Matcher, Ready};
use loams_temporal_resonate::{
    Claim, CreatePromise, DelaySupport, Delivery, Durability, DurabilityError, PromiseStatus,
    PromiseView, SettleAs,
};

/// A [`Durability`] that tells the matcher about the tasks it creates.
pub struct FastPath {
    inner: Arc<dyn Durability>,
    matcher: std::sync::Weak<Matcher>,
}

impl std::fmt::Debug for FastPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FastPath").finish_non_exhaustive()
    }
}

impl FastPath {
    /// Wraps `inner`, offering created tasks to `matcher`.
    pub fn new(inner: Arc<dyn Durability>, matcher: &Arc<Matcher>) -> Self {
        FastPath {
            inner,
            matcher: Arc::downgrade(matcher),
        }
    }
}

#[async_trait]
impl Durability for FastPath {
    async fn promise_create(&self, req: &CreatePromise) -> Result<PromiseView, DurabilityError> {
        let view = self.inner.promise_create(req).await?;
        let due = req.delay_until_ms.is_none_or(|d| d <= self.inner.now_ms());
        if let (Some(group), PromiseStatus::Pending, true) = (&req.target_group, view.status, due)
            && let Some(m) = self.matcher.upgrade()
        {
            // A new task is pending at version 0; if it has moved on, the
            // claim is refused and the matcher skips it.
            m.offer_group(
                group,
                Ready {
                    task_id: req.id.clone(),
                    version: 0,
                },
            )
            .await;
        }
        Ok(view)
    }
    async fn promise_settle(
        &self,
        id: &str,
        state: SettleAs,
    ) -> Result<PromiseView, DurabilityError> {
        self.inner.promise_settle(id, state).await
    }
    async fn promise_register_listener(
        &self,
        id: &str,
        group: &str,
    ) -> Result<PromiseView, DurabilityError> {
        self.inner.promise_register_listener(id, group).await
    }
    async fn promise_get(&self, id: &str) -> Result<PromiseView, DurabilityError> {
        self.inner.promise_get(id).await
    }
    async fn task_acquire(
        &self,
        task_id: &str,
        version: i64,
        pid: &str,
        ttl_ms: i64,
    ) -> Result<Claim, DurabilityError> {
        self.inner.task_acquire(task_id, version, pid, ttl_ms).await
    }
    async fn task_heartbeat(
        &self,
        pid: &str,
        tasks: &[(&str, i64)],
    ) -> Result<(), DurabilityError> {
        self.inner.task_heartbeat(pid, tasks).await
    }
    async fn task_fulfill(
        &self,
        task_id: &str,
        version: i64,
        state: SettleAs,
    ) -> Result<(), DurabilityError> {
        self.inner.task_fulfill(task_id, version, state).await
    }
    async fn task_release(&self, task_id: &str, version: i64) -> Result<(), DurabilityError> {
        self.inner.task_release(task_id, version).await
    }
    async fn subscribe(
        &self,
        group: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<Delivery>, DurabilityError> {
        self.inner.subscribe(group).await
    }
    fn delay_support(&self) -> DelaySupport {
        self.inner.delay_support()
    }
    fn now_ms(&self) -> i64 {
        self.inner.now_ms()
    }
}
