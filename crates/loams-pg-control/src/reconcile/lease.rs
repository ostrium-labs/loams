//! The fenced project lease (design §46 §6.3; PG2 Task 7): one reconciler
//! acts on a project at a time, and a reconciler that lost the lease can
//! write nothing more for it.
//!
//! [`Leases`] keeps the fence this instance holds for each project. A pass
//! [`take`](Leases::take)s it: the held fence is renewed (`renew_lease`,
//! the same epoch), and only a lost one is acquired again, at a new epoch
//! (R3.12), so a holder never gets an old fence back. Before each Neon call
//! and each write the pass [`renew`](Leases::renew)s it again, and stops
//! when the lease is lost. A write after a lapse that the renewal did not
//! see (the holder paused in between) is refused by the store itself:
//! every write carries the fence, and the store checks the lease's epoch in
//! the write's own transaction (R3.2), so it fails `Fenced`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::model::project_lease;
use crate::store::{Fence, PgControlStore, StoreError};

use super::ReconcileError;

/// This instance's fences, by project id.
#[derive(Debug)]
pub(crate) struct Leases {
    holder: String,
    ttl: Duration,
    held: Mutex<HashMap<String, Fence>>,
}

/// What [`Leases::take`] found.
pub(crate) enum Taken {
    /// This instance holds the lease: act under the fence.
    Held(Fence),
    /// Another holder has it.
    Other(String),
}

impl Leases {
    pub fn new(holder: String, ttl: Duration) -> Self {
        Leases {
            holder,
            ttl,
            held: Mutex::new(HashMap::new()),
        }
    }

    fn held(&self) -> std::sync::MutexGuard<'_, HashMap<String, Fence>> {
        // A panic while holding the map leaves it consistent (single
        // inserts and removes), so a poisoned lock is still usable.
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The project's lease for this instance: the held fence renewed, or
    /// (none held, or lost) a newly acquired one.
    pub async fn take<S: PgControlStore>(
        &self,
        store: &S,
        project: &str,
    ) -> Result<Taken, StoreError> {
        let cached = self.held().get(project).cloned();
        if let Some(fence) = cached {
            match store.renew_lease(&fence, self.ttl).await {
                Ok(fence) => return Ok(Taken::Held(fence)),
                Err(StoreError::LeaseLost) => self.forget(project),
                Err(e) => return Err(e),
            }
        }
        match store
            .acquire_lease(&project_lease(project), &self.holder, self.ttl)
            .await
        {
            Ok(fence) => {
                self.held().insert(project.to_string(), fence.clone());
                Ok(Taken::Held(fence))
            }
            Err(StoreError::Held { holder, .. }) => Ok(Taken::Other(holder)),
            Err(e) => Err(e),
        }
    }

    /// Extends `fence` before a step; a lost lease ends the pass
    /// ([`ReconcileError::Fenced`]).
    pub async fn renew<S: PgControlStore>(
        &self,
        store: &S,
        project: &str,
        fence: &Fence,
    ) -> Result<(), ReconcileError> {
        match store.renew_lease(fence, self.ttl).await {
            Ok(_) => Ok(()),
            Err(StoreError::LeaseLost) => {
                self.forget(project);
                Err(ReconcileError::Fenced)
            }
            Err(e) => Err(ReconcileError::Store(e)),
        }
    }

    /// Drops the project's fence: lost, or the project is gone.
    pub fn forget(&self, project: &str) {
        self.held().remove(project);
    }
}
