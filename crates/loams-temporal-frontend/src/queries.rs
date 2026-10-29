//! Queries: handed to a live worker through the matcher's in-memory items,
//! answered through `RespondQueryTaskCompleted`. Never durable, as in
//! Temporal.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use loams_temporal_proto::temporal::api::common::v1::Payloads;
use loams_temporal_proto::temporal::api::query::v1::WorkflowQuery;
use loams_temporal_store::RunKey;
use tokio::sync::oneshot;

/// How a worker answered.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryAnswer {
    /// The query's result.
    Answered(Option<Payloads>),
    /// The worker's error message and failure (boxed: a `Failure` is large,
    /// and most answers are `Answered`).
    Failed(
        String,
        Option<Box<loams_temporal_proto::temporal::api::failure::v1::Failure>>,
    ),
}

/// A query waiting for a worker.
#[derive(Debug, Clone)]
pub struct QueryTask {
    /// The run it asks.
    pub key: RunKey,
    /// The Loams namespace of the run.
    pub loams_namespace_id: String,
    /// The query.
    pub query: WorkflowQuery,
}

#[derive(Default)]
struct Entry {
    task: Option<QueryTask>,
    tx: Option<oneshot::Sender<QueryAnswer>>,
}

/// The queries in flight.
#[derive(Default)]
pub struct Queries {
    next: AtomicU64,
    pending: Mutex<HashMap<u64, Entry>>,
}

impl std::fmt::Debug for Queries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queries").finish_non_exhaustive()
    }
}

impl Queries {
    /// Registers a query; returns its ID and where its answer arrives.
    pub fn register(&self, task: QueryTask) -> (u64, oneshot::Receiver<QueryAnswer>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                id,
                Entry {
                    task: Some(task),
                    tx: Some(tx),
                },
            );
        (id, rx)
    }

    /// The query a poll got, if it is still wanted.
    pub fn task(&self, id: u64) -> Option<QueryTask> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .and_then(|e| e.task.clone())
    }

    /// Delivers a worker's answer; false if nobody waits any more.
    pub fn complete(&self, id: u64, answer: QueryAnswer) -> bool {
        let entry = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        entry
            .and_then(|e| e.tx)
            .is_some_and(|tx| tx.send(answer).is_ok())
    }

    /// Forgets a query (its caller gave up).
    pub fn remove(&self, id: u64) {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
}
