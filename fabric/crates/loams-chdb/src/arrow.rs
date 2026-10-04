//! Arrow in and out through chDB's Arrow C stream interface.
//!
//! Both directions are the same ABI, and Task 1 measured its shape:
//!
//! * **Out** (`chdb_stream_query_arrow` and the `chdb_stream_fetch_arrow` that
//!   follows it) writes into a caller-allocated `chdb_arrow_stream_` cell and
//!   leaves a pointer to its own `ArrowArrayStream` in that cell's `internal_data`.
//!   Handing it a cell with `internal_data` already set is *not* the protocol: a
//!   probe that pre-filled the cell with its own stream found the storage
//!   untouched at zero while the call allocated a stream of its own. The engine
//!   owns that stream, and the handle that produced it owns the right to release
//!   it.
//! * **In** (`chdb_arrow_scan`) reads a caller-allocated `ArrowArrayStream` from
//!   the same cell. The reader behind it has to outlive the registration, because
//!   the engine pulls from it whenever a statement reads the table.
//!
//! So [`ArrowStream`] imports the engine's stream into arrow-rs and hands out record
//! batches, and [`ArrowHandle`] owns an exported stream for as long as the table can
//! be read.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use arrow::array::RecordBatch;
use arrow::datatypes::SchemaRef;
use arrow::ffi_stream::FFI_ArrowArrayStream;
use loams_chdb_sys::ffi;

use crate::engine::engine_error;
use crate::error::ChdbError;
use crate::query::QueryStats;

/// Re-exported so a caller can build a reader without naming arrow itself.
pub use arrow::array::RecordBatchReader;
pub use arrow::datatypes::{DataType, Field, Schema};

/// The part of an [`ArrowStream`] a blocking thread needs to pull a block.
struct Shared {
    stream: Arc<ffi::ArrowStream>,
    /// The block in hand, which holds chDB's C stream and releases it when it drops.
    block: Mutex<Option<ffi::ArrowBlockReader>>,
    stats: Mutex<QueryStats>,
}

impl Shared {
    fn locked_block(&self) -> MutexGuard<'_, Option<ffi::ArrowBlockReader>> {
        self.block
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The next batch of the block in hand, keeping an error instead of losing it.
    fn next_of_block(&self) -> Result<Option<RecordBatch>, ChdbError> {
        let mut guard = self.locked_block();
        let Some(block) = guard.as_ref() else {
            return Ok(None);
        };
        match block.next_batch() {
            Ok(Some(batch)) => Ok(Some(batch)),
            Ok(None) => {
                // The block is spent; its stream is released with it.
                *guard = None;
                Ok(None)
            }
            Err(err) => {
                *guard = None;
                Err(ChdbError::loams("ARROW_READ_FAILED", err.message))
            }
        }
    }

    /// The next batch, fetching blocks from the engine as needed.
    fn next_batch(&self) -> Result<Option<RecordBatch>, ChdbError> {
        loop {
            match self.next_of_block()? {
                Some(batch) => {
                    self.absorb(batch.num_rows() as u64);
                    return Ok(Some(batch));
                }
                None => match self.stream.fetch().map_err(engine_error)? {
                    Some(block) => *self.locked_block() = Some(block),
                    None => return Ok(None),
                },
            }
        }
    }

    /// Puts the engine's next block in hand.
    fn fetch_block(&self) -> Result<(), ChdbError> {
        match self.stream.fetch().map_err(engine_error)? {
            Some(reader) => {
                *self.locked_block() = Some(reader);
                Ok(())
            }
            None => {
                *self.locked_block() = None;
                Ok(())
            }
        }
    }

    /// Counts the rows a batch carries.
    ///
    /// The Arrow path answers a state rather than a result handle, so the block
    /// counters are only readable from the statement handle itself; the rows of the
    /// batches are the honest number here.
    fn absorb(&self, rows: u64) {
        let mut stats = self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        stats.result_rows += rows;
    }

    fn stats(&self) -> QueryStats {
        let mut stats = *self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // The statement's own counters are readable from the handle, unlike the
        // blocks'.
        let block = self.stream.stats();
        stats.rows_read = block.storage_rows_read;
        stats.bytes_read = block.storage_bytes_read;
        stats.elapsed = block.elapsed;
        stats
    }
}

/// An Arrow record batch stream produced by a query.
///
/// Dropping it releases the engine's `ArrowArrayStream` and destroys the handle it
/// came from.
pub struct ArrowStream {
    query_id: String,
    shared: Arc<Shared>,
    done: AtomicBool,
}

impl std::fmt::Debug for ArrowStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArrowStream")
            .field("query_id", &self.query_id)
            .field("done", &self.is_done())
            .finish_non_exhaustive()
    }
}

impl ArrowStream {
    /// Starts the query and fetches the first block, so the schema is known before
    /// any batch is asked for.
    ///
    /// An Arrow statement is not in the session's cancellation registry: the ABI's
    /// `chdb_stream_cancel_query` takes a `chdb_stream_query_n` handle and the
    /// header offers nothing for the `chdb_stream_query_arrow_n` handle this uses.
    pub(crate) fn start(
        query_id: String,
        conn: &Arc<ffi::Connection>,
        sql: &str,
    ) -> Result<Self, ChdbError> {
        let stream = Arc::new(conn.arrow_stream(sql).map_err(engine_error)?);
        let shared = Arc::new(Shared {
            stream,
            block: Mutex::new(None),
            stats: Mutex::new(QueryStats::default()),
        });
        let result = Self {
            query_id,
            shared,
            done: AtomicBool::new(false),
        };
        // One block eagerly: `schema` answers from it, and a query that produces
        // nothing is discovered here rather than at the first `next_batch`.
        result.shared.fetch_block()?;
        Ok(result)
    }

    /// The query id, as [`crate::Session::execute_arrow`] was given it.
    pub fn query_id(&self) -> &str {
        &self.query_id
    }

    /// The schema of the result, which is the first block's schema.
    pub fn schema(&self) -> Result<SchemaRef, ChdbError> {
        self.shared
            .locked_block()
            .as_ref()
            .map(|block| block.schema())
            .ok_or_else(|| {
                ChdbError::engine("the query produced no Arrow block, so chDB sent no schema")
            })
    }

    /// The counters so far.
    pub fn stats(&self) -> QueryStats {
        self.shared.stats()
    }

    /// Whether the stream has ended.
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    /// The next record batch, or `None` at the end of the stream.
    pub fn next_batch(&mut self) -> Result<Option<RecordBatch>, ChdbError> {
        if self.is_done() {
            return Ok(None);
        }
        match self.shared.next_batch()? {
            Some(batch) => Ok(Some(batch)),
            None => {
                self.done.store(true, Ordering::SeqCst);
                Ok(None)
            }
        }
    }

    /// [`ArrowStream::next_batch`] on a blocking thread.
    ///
    /// A fetch can wait on the engine for as long as the query takes, so an async
    /// caller must not make it on its own task.
    pub async fn next_batch_async(&self) -> Result<Option<RecordBatch>, ChdbError> {
        if self.is_done() {
            return Ok(None);
        }
        let shared = Arc::clone(&self.shared);
        let pulled: Result<Option<RecordBatch>, ChdbError> =
            tokio::task::spawn_blocking(move || shared.next_batch())
                .await
                .unwrap_or_else(|err| {
                    Err(ChdbError::loams("BLOCKING_TASK_FAILED", err.to_string()))
                });
        if matches!(pulled, Ok(None)) {
            self.done.store(true, Ordering::SeqCst);
        }
        pulled
    }
}

/// A registered Arrow table.
///
/// Holds the exported reader for as long as the engine may pull from it, and
/// unregisters the table when it drops.
#[derive(Debug)]
pub struct ArrowHandle {
    name: String,
    conn: Arc<ffi::Connection>,
    /// The exported stream, which owns the boxed reader. It is kept alive here
    /// because a reader freed while the table is registered is a use-after-free the
    /// next statement reading that table would walk into.
    stream: Box<FFI_ArrowArrayStream>,
    registered: bool,
}

impl ArrowHandle {
    /// The name the table is registered under.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the exported reader is still held.
    ///
    /// It is for as long as this handle lives: the stream's release callback is
    /// what frees the reader, and the callback is gone once the stream has been
    /// released. A handle that had dropped its reader would leave the engine
    /// reading freed memory on the next statement that reads the table.
    pub fn reader_is_alive(&self) -> bool {
        self.stream.release.is_some()
    }

    /// Unregisters the table early, releasing the reader.
    ///
    /// The engine's registration goes first: a table that is still registered
    /// while its reader is freed is the use-after-free above.
    pub fn unregister(&mut self) -> Result<(), ChdbError> {
        if !self.registered {
            return Ok(());
        }
        self.conn
            .unregister_arrow(&self.name)
            .map_err(engine_error)?;
        self.registered = false;
        Ok(())
    }
}

impl Drop for ArrowHandle {
    fn drop(&mut self) {
        if self.registered {
            let _ = self.conn.unregister_arrow(&self.name);
        }
        // The stream is released here, by its own Drop, after the engine has been
        // told the table is gone.
    }
}

/// Exports `reader` to the engine as a table called `name`.
///
/// The reader is boxed into the exported stream, so the caller's binding is
/// consumed: the engine reads it through `get_next` for as long as the table is
/// registered.
pub(crate) fn scan(
    conn: Arc<ffi::Connection>,
    name: &str,
    reader: Box<dyn RecordBatchReader + Send>,
) -> Result<ArrowHandle, ChdbError> {
    let stream = ffi::export_reader(reader);
    conn.scan_arrow(name, &stream).map_err(engine_error)?;
    // The handle owns the stream itself: the engine reads the reader through it
    // until the table is unregistered, and dropping it is what releases the reader.
    let stream = Box::new(stream);
    Ok(ArrowHandle {
        name: name.to_string(),
        conn,
        stream,
        registered: true,
    })
}
