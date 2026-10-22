//! `documents.write_batch` (runtime contract R13, SDKP §2.3; design §44 §7.5).
//!
//! A bulk load as a loop of ordinary `WriteDocuments` calls, chunked by
//! operations and by encoded bytes:
//!
//! * **one chunk in flight at a time**, so order holds and a failure stops at
//!   a known point;
//! * **one idempotency key per chunk**, minted before the chunk's first attempt
//!   and reused on that chunk's retries, so a retried chunk is the same write;
//! * **the tokens merge**: the result is the merge of every chunk's token;
//! * **the first failed chunk stops the load** with a [`BatchError`] that says
//!   how many chunks and operations were applied, and carries their token.

use std::future::Future;

use crate::consistency::ConsistencyToken;
use crate::error::{BATCH_PARTIALLY_APPLIED, LoamsError};
use crate::uuidv7::uuidv7;

/// The default most operations in one chunk.
pub const DEFAULT_CHUNK_ROWS: usize = 10_000;
/// The default most encoded bytes in one chunk (4 MiB).
pub const DEFAULT_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// How [`write_batch`] chunks.
#[derive(Debug, Clone, Copy)]
pub struct BatchOptions {
    /// The most operations in one chunk.
    pub chunk_rows: usize,
    /// The most encoded bytes in one chunk. One operation larger than this
    /// goes in a chunk of its own.
    pub chunk_bytes: usize,
}

impl Default for BatchOptions {
    fn default() -> Self {
        BatchOptions {
            chunk_rows: DEFAULT_CHUNK_ROWS,
            chunk_bytes: DEFAULT_CHUNK_BYTES,
        }
    }
}

/// What a finished batch reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchResult {
    /// The merge of every chunk's consistency token.
    pub token: Option<ConsistencyToken>,
    /// Chunks written.
    pub chunks: usize,
    /// Operations those chunks carried.
    pub rows: usize,
}

/// A batch that stopped at a failed chunk. The chunks before it are applied
/// and stay applied.
#[derive(Debug, thiserror::Error)]
#[error("write_batch stopped at chunk {}: {chunks_applied} chunks ({rows_applied} operations) were applied; {source}", chunks_applied + 1)]
pub struct BatchError {
    /// Chunks applied before the failure.
    pub chunks_applied: usize,
    /// Operations those chunks carried.
    pub rows_applied: usize,
    /// The merged token of the applied chunks.
    pub token: Option<ConsistencyToken>,
    /// The failed chunk's own error.
    #[source]
    pub source: LoamsError,
}

impl BatchError {
    /// The client-side reason, `batch_partially_applied`.
    #[must_use]
    pub fn reason(&self) -> &'static str {
        BATCH_PARTIALLY_APPLIED
    }
}

impl From<BatchError> for LoamsError {
    fn from(error: BatchError) -> Self {
        let chunks = error.chunks_applied.to_string();
        let rows = error.rows_applied.to_string();
        let message = error.to_string();
        LoamsError::new(error.source.kind, error.source.code, message)
            .with_client_reason(BATCH_PARTIALLY_APPLIED)
            .with_metadata("chunks_applied", chunks)
            .with_metadata("rows_applied", rows)
    }
}

/// Splits `ops` into chunks and writes them one after another.
///
/// `size` measures one operation (its encoded length); `write` sends one
/// chunk with that chunk's idempotency key and answers its consistency token.
///
/// # Errors
///
/// The first chunk that fails, as a [`BatchError`].
pub async fn write_batch<Op, S, W, Fut>(
    ops: impl IntoIterator<Item = Op>,
    options: BatchOptions,
    size: S,
    mut write: W,
) -> Result<BatchResult, BatchError>
where
    S: Fn(&Op) -> usize,
    W: FnMut(Vec<Op>, String) -> Fut,
    Fut: Future<Output = Result<Option<String>, LoamsError>>,
{
    let chunk_rows = options.chunk_rows.max(1);
    let chunk_bytes = options.chunk_bytes.max(1);
    let mut result = BatchResult::default();
    let mut pending: Vec<Op> = Vec::new();
    let mut pending_bytes = 0usize;

    async fn flush<Op, W, Fut>(
        chunk: Vec<Op>,
        write: &mut W,
        result: &mut BatchResult,
    ) -> Result<(), BatchError>
    where
        W: FnMut(Vec<Op>, String) -> Fut,
        Fut: Future<Output = Result<Option<String>, LoamsError>>,
    {
        let rows = chunk.len();
        match write(chunk, uuidv7()).await {
            Ok(token) => {
                if let Some(token) = token.and_then(|t| t.parse::<ConsistencyToken>().ok()) {
                    result.token = Some(match result.token.take() {
                        None => token.normalized(),
                        Some(current) => current.merge(&token),
                    });
                }
                result.chunks += 1;
                result.rows += rows;
                Ok(())
            }
            Err(source) => Err(BatchError {
                chunks_applied: result.chunks,
                rows_applied: result.rows,
                token: result.token.clone(),
                source,
            }),
        }
    }

    for op in ops {
        let bytes = size(&op);
        if !pending.is_empty()
            && (pending.len() >= chunk_rows || pending_bytes + bytes > chunk_bytes)
        {
            flush(std::mem::take(&mut pending), &mut write, &mut result).await?;
            pending_bytes = 0;
        }
        pending.push(op);
        pending_bytes += bytes;
    }
    if !pending.is_empty() {
        flush(pending, &mut write, &mut result).await?;
    }
    Ok(result)
}
