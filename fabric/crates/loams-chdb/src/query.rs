//! Streaming results: [`QueryStream`], its [`QueryStats`], and the registry
//! [`crate::Session::cancel`] needs to reach a running statement.
//!
//! chDB streams a result in blocks: `chdb_stream_query_n` starts the statement and
//! hands back a handle, `chdb_stream_fetch_result` returns one block at a time,
//! and a block of length zero ends the stream. Task 1 measured three blocks for
//! 100 000 rows in `RowBinary`, so a client that wants the last block early has to
//! ask for it — which is exactly what a streaming HTTP body does.
//!
//! The statistics are the ones ClickHouse puts in `X-ClickHouse-Summary`, which
//! Task 2 will write verbatim, and the header's naming is confusing enough to be
//! worth spelling out: `chdb_result_rows_read` counts the rows **in the result**,
//! while `chdb_result_storage_rows_read` counts the rows read **from storage**. So
//! [`QueryStats::result_rows`] and [`QueryStats::rows_read`] come from the two
//! different calls.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use bytes::Bytes;
use loams_chdb_sys::ffi::{self, Stream};

use crate::engine::engine_error;
use crate::error::ChdbError;

/// What a run reports about itself.
///
/// The names are ClickHouse's own, so Task 2 can put them in
/// `X-ClickHouse-Summary` without renaming anything.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QueryStats {
    /// Rows read from storage (`read_rows`).
    pub rows_read: u64,
    /// Bytes read from storage (`read_bytes`).
    pub bytes_read: u64,
    /// Rows produced into the result so far (`result_rows`).
    pub result_rows: u64,
    /// Bytes produced into the result so far (`result_bytes`).
    pub result_bytes: u64,
    /// How long the query has been running, in seconds.
    pub elapsed: f64,
}

impl QueryStats {
    /// Folds one block's counters in.
    ///
    /// The storage counters are progress and take the newest block's value; the
    /// result counters accumulate, because a result arrives in pieces and the
    /// summary wants the total.
    fn absorb(&mut self, block: &ffi::Block) {
        self.result_rows += block.rows_read;
        self.result_bytes += block.bytes_read;
        self.rows_read = block.storage_rows_read;
        self.bytes_read = block.storage_bytes_read;
        self.elapsed = block.elapsed;
    }
}

/// The live statements of one session, by query id.
///
/// A `KILL QUERY` arrives while the statement it names holds the session, so the
/// handle has to be findable from somewhere else: this is that place. The map
/// holds [`Weak`] references, so a forgotten statement is collected rather than
/// leaked.
#[derive(Debug, Default)]
pub(crate) struct Registry {
    live: Mutex<HashMap<String, Weak<Stream>>>,
    /// Cancellations that arrived before the statement they name: the caller
    /// chooses the query id, so it can know it before `execute` has returned.
    pending: Mutex<HashSet<String>>,
    /// Statements this session has run and finished. An id here is one a client can
    /// be told about: the statement is over, so there is nothing left to cancel.
    finished: Mutex<HashSet<String>>,
}

impl Registry {
    fn live(&self) -> MutexGuard<'_, HashMap<String, Weak<Stream>>> {
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn pending(&self) -> MutexGuard<'_, HashSet<String>> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn finished(&self) -> MutexGuard<'_, HashSet<String>> {
        self.finished
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether a statement of this id has run and finished.
    pub(crate) fn has_finished(&self, query_id: &str) -> bool {
        self.finished().contains(query_id)
    }

    /// Records a started statement.
    pub(crate) fn insert(&self, query_id: &str, stream: &Arc<Stream>) {
        self.live()
            .insert(query_id.to_string(), Arc::downgrade(stream));
    }

    /// Forgets a finished statement, remembering that it has run: a later `cancel`
    /// for that id is refused rather than silently doing nothing.
    pub(crate) fn remove(&self, query_id: &str) {
        self.live().remove(query_id);
        self.pending().remove(query_id);
        self.finished().insert(query_id.to_string());
    }

    /// The stream of a started statement, if it is still running.
    pub(crate) fn get(&self, query_id: &str) -> Option<Arc<Stream>> {
        let entry = self.live().get(query_id)?.clone();
        entry.upgrade()
    }

    /// Records a cancellation that arrived first, and says whether this call is
    /// the one that recorded it.
    pub(crate) fn mark_pending(&self, query_id: &str) -> bool {
        self.pending().insert(query_id.to_string())
    }

    /// Takes a pending cancellation for this statement, if one is waiting.
    pub(crate) fn take_pending(&self, query_id: &str) -> bool {
        self.pending().remove(query_id)
    }
}

/// Asks the engine to stop a statement, on its own thread.
///
/// Task 1 measured what `chdb_stream_cancel_query` is: it does **not** interrupt a
/// running statement, it blocks until the statement has finished on its own
/// (`SELECT count() FROM numbers(1e12)` answered with its result after 275 s, and
/// the cancel call returned a second earlier) and only then tears the stream down,
/// after which a fetch says `"No active streaming query"`.
///
/// There is no cheaper handle, either: chDB 26.9.0 has no `query_id` setting — `SET
/// query_id` answers 115 `UNKNOWN_SETTING` — so a statement cannot even be named
/// for a cross-connection `KILL QUERY`, and `system.processes` lists only ids the
/// engine generated itself.
///
/// So the cancellation a client sees is Loams', not the engine's:
/// [`crate::Session::cancel`] marks the statement, the next
/// [`QueryStream::next_chunk`] answers `394 CANCELLED` at once, and this thread
/// asks the engine to stop in the background.
pub(crate) fn spawn_cancel(query_id: &str, stream: Arc<Stream>) -> Result<(), ChdbError> {
    stream.cancel();
    std::thread::Builder::new()
        .name(format!("loams-chdb-cancel-{query_id}"))
        .spawn(move || {
            if let Some(token) = stream.claim_for_cancel() {
                token.cancel_and_release();
            }
        })
        .map(|_| ())
        .map_err(|err| {
            ChdbError::loams(
                "CANCEL_NOT_STARTED",
                format!("the cancellation thread for {query_id} did not start: {err}"),
            )
        })
}

/// A streaming result.
///
/// Dropping it destroys the chDB handle, which is the only way that handle is
/// released; a statement that is still running when its stream goes away is torn
/// down with it.
#[derive(Debug)]
pub struct QueryStream {
    query_id: String,
    stream: Arc<Stream>,
    registry: Arc<Registry>,
    /// Behind a mutex so an async caller can hand the pull to a blocking thread,
    /// while the synchronous API keeps its `&mut self`.
    state: Arc<Mutex<State>>,
}

#[derive(Debug, Default)]
struct State {
    stats: QueryStats,
    done: bool,
}

impl QueryStream {
    /// Wraps a started statement.
    pub(crate) fn new(query_id: String, stream: Arc<Stream>, registry: Arc<Registry>) -> Self {
        registry.insert(&query_id, &stream);
        let query_stream = Self {
            query_id: query_id.clone(),
            stream,
            registry,
            state: Arc::new(Mutex::new(State::default())),
        };
        // A cancellation that arrived while the statement was being set up: the
        // caller chose the id, so it could name the statement before `execute`
        // returned.
        if query_stream.registry.take_pending(&query_id) {
            let _ = spawn_cancel(&query_id, Arc::clone(&query_stream.stream));
        }
        query_stream
    }

    /// The query id, which Task 2 echoes in `X-ClickHouse-Query-Id` and which
    /// [`crate::Session::cancel`] takes.
    pub fn query_id(&self) -> &str {
        &self.query_id
    }

    /// The counters so far.
    pub fn stats(&self) -> QueryStats {
        self.locked().stats
    }

    /// Whether the stream has ended, by an empty block or by an error.
    pub fn is_done(&self) -> bool {
        self.locked().done
    }

    fn locked(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The next block, or `None` at the end of the stream.
    ///
    /// This blocks: a fetch waits for the engine to produce the block, which for a
    /// long query is a long time. An async caller wants
    /// [`QueryStream::next_chunk_async`].
    pub fn next_chunk(&mut self) -> Result<Option<Bytes>, ChdbError> {
        self.next_block()
    }

    /// The next block, on a blocking thread.
    ///
    /// Task 1's Semantics paragraph: every FFI call an async caller makes runs on a
    /// blocking thread.
    pub async fn next_chunk_async(&self) -> Result<Option<Bytes>, ChdbError> {
        let stream = Arc::clone(&self.stream);
        let state = Arc::clone(&self.state);
        let query_id = self.query_id.clone();
        tokio::task::spawn_blocking(move || {
            let mut state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.next_block(&query_id, &stream)
        })
        .await
        .unwrap_or_else(|err| Err(ChdbError::loams("BLOCKING_TASK_FAILED", err.to_string())))
    }

    fn next_block(&mut self) -> Result<Option<Bytes>, ChdbError> {
        let mut state = self.locked();
        let query_id = self.query_id.clone();
        let stream = Arc::clone(&self.stream);
        state.next_block(&query_id, &stream)
    }
}

impl State {
    fn next_block(
        &mut self,
        query_id: &str,
        stream: &Arc<Stream>,
    ) -> Result<Option<Bytes>, ChdbError> {
        if self.done {
            return Ok(None);
        }
        if stream.is_cancelled() {
            // Answering now, rather than after the engine acknowledges, is what
            // makes a cancellation prompt; `spawn_cancel` says what the engine is
            // told and when.
            self.done = true;
            return Err(ChdbError::cancelled(
                query_id,
                "cancelled by loams-chdb before the next block",
            ));
        }

        match stream.fetch() {
            Ok(None) => {
                self.done = true;
                Ok(None)
            }
            Ok(Some(block)) => {
                self.absorb(&block);
                Ok(Some(Bytes::from(block.bytes)))
            }
            Err(err) => {
                self.done = true;
                Err(if stream.is_cancelled() {
                    // The engine's own message after a cancellation is
                    // "No active streaming query", which carries no code; 394 is
                    // what a client that killed a statement expects.
                    ChdbError::cancelled(query_id, err.message)
                } else {
                    engine_error(err)
                })
            }
        }
    }

    fn absorb(&mut self, block: &ffi::Block) {
        QueryStats::absorb(&mut self.stats, block);
    }
}

impl Drop for QueryStream {
    fn drop(&mut self) {
        self.registry.remove(&self.query_id);
    }
}

/// Cancels a statement by query id, if it is running or is about to be.
///
/// Returns as soon as the cancellation is registered: the next block of that
/// statement answers `394 CANCELLED` without waiting for the engine, and the ABI's
/// blocking cancel runs in the background. A `query_id` no statement is using is
/// refused with `QUERY_ID_UNKNOWN`.
pub(crate) fn cancel(registry: &Registry, query_id: &str) -> Result<(), ChdbError> {
    match registry.get(query_id) {
        Some(stream) => spawn_cancel(query_id, stream),
        None if registry.has_finished(query_id) => Err(ChdbError::loams(
            "QUERY_ID_UNKNOWN",
            format!("the statement {query_id} has already finished, so there is nothing to cancel"),
        )),
        None if registry.mark_pending(query_id) => {
            // The statement is being set up with this id and has not registered a
            // handle yet, which is a race the caller cannot avoid: it chose the id.
            // `QueryStream::new` picks the cancellation up.
            Ok(())
        }
        // A cancellation already recorded for a statement that has not started.
        None => Ok(()),
    }
}
