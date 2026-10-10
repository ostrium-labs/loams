//! `ExecuteStream`: a statement's rows in chunks (GR1 Task 6; §48 §8.2, §13.1).
//!
//! A chunk holds at most [`MAX_CHUNK_ROWS`] rows (or the request's smaller `chunk_rows`) and at
//! most [`MAX_CHUNK_BYTES`] encoded bytes; a single row larger than that travels alone, and one
//! larger than a Connect message (4 MiB) is `graph_result_too_large` (review fix 1, M7). The
//! first chunk carries the columns, the last one says so and carries `truncated` and the
//! elapsed time. An empty result is one last chunk with the columns.
//!
//! The statement runs to its end before the first chunk (R6.3): Grafeo 0.5.43's own streaming
//! call refuses parameters, writes, `ORDER BY`, aggregates and `DISTINCT`, and its `QueryResult`
//! holds every row anyway. What the stream adds is that no answer is cut at the unary limits:
//! rows are converted to the wire a chunk at a time, and the request's `max_rows` (0: none) caps
//! the whole stream. `max_stream_bytes` caps it in bytes whatever `max_rows` says: past it the
//! stream ends `truncated`. The stream holds the statement's process and namespace slots until
//! its last chunk is taken or it is dropped (review fix 1, I4).

use loams_proto::loams::graph::v1 as pb;

use super::admin::Slots;
use connectrpc::ConnectError;

use super::data::{check_row, row_bytes, to_pb_row};
use super::errors::map_engine;
use crate::engine::GraphResult;

/// The most rows in one chunk.
pub const MAX_CHUNK_ROWS: u32 = 1_000;

/// The most encoded bytes in one chunk, unless one row alone is larger.
pub const MAX_CHUNK_BYTES: u64 = 1 << 20;

/// Room kept in every chunk for its other fields (`last`, `truncated`, the counters and the
/// elapsed time).
const CHUNK_OVERHEAD: u64 = 256;

/// The chunks of a finished statement, in order.
pub(crate) struct Chunks {
    result: GraphResult,
    next: usize,
    chunk_rows: usize,
    /// The most encoded bytes the whole stream may carry (`max_stream_bytes`, review fix 1, I4).
    max_bytes: u64,
    /// What the stream has carried so far.
    sent_bytes: u64,
    sent_first: bool,
    done: bool,
    /// A row converted for a chunk it did not fit in, for the next one.
    pending: Option<pb::Row>,
    /// The statement's slots, held until the stream ends or is dropped (review fix 1, I4).
    slots: Option<Slots>,
}

impl Chunks {
    /// `result`'s rows in chunks of at most `chunk_rows` (0, or more than [`MAX_CHUNK_ROWS`],
    /// takes [`MAX_CHUNK_ROWS`]), at most `max_bytes` in all.
    pub(crate) fn new(result: GraphResult, chunk_rows: u32, max_bytes: u64) -> Self {
        let chunk_rows = if chunk_rows == 0 {
            MAX_CHUNK_ROWS
        } else {
            chunk_rows.min(MAX_CHUNK_ROWS)
        };
        Self {
            result,
            next: 0,
            chunk_rows: chunk_rows as usize,
            max_bytes,
            sent_bytes: 0,
            sent_first: false,
            done: false,
            pending: None,
            slots: None,
        }
    }

    /// The same chunks, keeping `slots` until the last chunk is taken or the stream is dropped.
    pub(crate) fn holding(mut self, slots: Slots) -> Self {
        self.slots = Some(slots);
        self
    }

    fn finish(&mut self, chunk: &mut pb::ResultChunk, truncated: bool) {
        self.done = true;
        self.slots = None;
        chunk.last = true;
        chunk.truncated = truncated;
        chunk.elapsed_nanos = self.result.elapsed_nanos.unwrap_or_default();
    }
}

impl Iterator for Chunks {
    type Item = Result<pb::ResultChunk, ConnectError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut chunk = pb::ResultChunk::default();
        let mut bytes = CHUNK_OVERHEAD;
        if !self.sent_first {
            self.sent_first = true;
            chunk.columns = self.result.columns.clone();
            chunk.column_types = self.result.column_types.clone();
            bytes += self
                .result
                .columns
                .iter()
                .chain(&self.result.column_types)
                .map(|name| name.len() as u64 + 6)
                .sum::<u64>();
        }
        while chunk.rows.len() < self.chunk_rows {
            let row = match self.pending.take() {
                Some(row) => row,
                None if self.next < self.result.rows.len() => {
                    // Each engine row is freed once it is on the wire's side.
                    let values = std::mem::take(&mut self.result.rows[self.next].values);
                    self.next += 1;
                    to_pb_row(&crate::engine::GraphRow { values })
                }
                None => break,
            };
            let size = row_bytes(&row);
            if let Err(err) = check_row(size) {
                // No message can carry this row (M7). A statement that committed ends its
                // stream `truncated` rather than failing (I3); a read fails the stream, after
                // the rows already sent.
                if self.result.wrote {
                    self.finish(&mut chunk, true);
                    return Some(Ok(chunk));
                }
                self.done = true;
                self.slots = None;
                return Some(Err(map_engine(err)));
            }
            if self.sent_bytes + bytes + size > self.max_bytes {
                // The whole stream's byte limit: it ends here, `truncated` (I4).
                self.finish(&mut chunk, true);
                return Some(Ok(chunk));
            }
            if !chunk.rows.is_empty() && bytes + size > MAX_CHUNK_BYTES {
                self.pending = Some(row);
                break;
            }
            bytes += size;
            chunk.rows.push(row);
        }
        self.sent_bytes += bytes;
        if self.pending.is_none() && self.next >= self.result.rows.len() {
            let truncated = self.result.truncated;
            self.finish(&mut chunk, truncated);
        }
        Some(Ok(chunk))
    }
}
