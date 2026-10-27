// `documents.writeBatch` (runtime contract R13, SDKP §2.3; design §44 §7.5).
//
// A bulk load as a loop of ordinary `WriteDocuments` calls, chunked by rows
// and by bytes:
//
// - **One chunk in flight at a time.** Order is preserved and a failure stops
//   the load at a known point, which concurrency would blur.
// - **One idempotency key per chunk**, minted before the chunk's first attempt
//   and reused on every retry of that chunk, so a retried chunk is the same
//   write (D610) and two chunks are never mistaken for one.
// - **The tokens merge.** The result is the merge of every chunk's
//   consistency token, so a read held to it sees the whole load.
// - **The first failed chunk stops the load** with a `BatchError` that says
//   how many chunks (and documents) were applied before it, and carries their
//   merged token. Those chunks stay applied; nothing is rolled back.
//
// Browser-safe: sizes are measured with `TextEncoder`, not `Buffer`.

import { Code } from '@connectrpc/connect';
import type { CallOptions } from './call.js';
import { ConsistencyToken } from './consistency.js';
import { BatchError, toLoamsError } from './errors.js';
import { uuidv7 } from './uuidv7.js';

/** The default chunk bounds (§44 §7.5). */
export const DEFAULT_CHUNK_ROWS = 10_000;
export const DEFAULT_CHUNK_BYTES = 4 * 1024 * 1024;

/** How `writeBatch` chunks. */
export interface BatchOptions {
  /** The most operations in one chunk. Default 10 000. */
  chunkRows?: number;
  /** The most bytes of operation JSON in one chunk. Default 4 MiB. A single
   * operation larger than this goes in a chunk of its own. */
  chunkBytes?: number;
}

/** What a finished batch reports. */
export interface BatchResult {
  /** The merge of every chunk's consistency token. */
  readonly token: string | undefined;
  /** How many chunks were written. */
  readonly chunks: number;
  /** How many operations those chunks carried. */
  readonly rows: number;
}

/** Sends one chunk; `idempotencyKey` is that chunk's own key. */
export type ChunkWriter<Op> = (
  ops: Op[],
  options: CallOptions & { idempotencyKey: string },
) => Promise<{ token?: string } | undefined>;

const encoder = new TextEncoder();

function sizeOf(op: unknown): number {
  return encoder.encode(
    JSON.stringify(op, (_key, value: unknown) =>
      typeof value === 'bigint' ? value.toString() : value,
    ),
  ).length;
}

function mergeToken(current: ConsistencyToken | undefined, next: string | undefined) {
  if (next === undefined || next === '') {
    return current;
  }
  try {
    const parsed = ConsistencyToken.parse(next);
    return current === undefined ? parsed.normalized() : current.merge(parsed);
  } catch {
    return current;
  }
}

/** Splits `ops` into chunks and writes them one after another. */
export async function writeBatch<Op>(
  write: ChunkWriter<Op>,
  ops: Iterable<Op> | AsyncIterable<Op>,
  options: BatchOptions & CallOptions = {},
): Promise<BatchResult> {
  const { chunkRows = DEFAULT_CHUNK_ROWS, chunkBytes = DEFAULT_CHUNK_BYTES, ...call } = options;
  if (!(chunkRows > 0) || !(chunkBytes > 0)) {
    throw new RangeError('writeBatch: chunkRows and chunkBytes must be positive');
  }
  let token: ConsistencyToken | undefined;
  let chunks = 0;
  let rows = 0;
  let pending: Op[] = [];
  let pendingBytes = 0;

  const flush = async () => {
    if (pending.length === 0) {
      return;
    }
    const chunk = pending;
    pending = [];
    pendingBytes = 0;
    try {
      // The key is minted here, once per chunk, and the invoker reuses it on
      // every retry of this chunk.
      const answer = await write(chunk, { ...call, idempotencyKey: uuidv7() });
      token = mergeToken(token, answer?.token);
    } catch (thrown) {
      const cause = toLoamsError(thrown);
      throw new BatchError(
        `writeBatch stopped at chunk ${chunks + 1}: ${chunks} chunks (${rows} operations) were applied before it; ${cause.message}`,
        {
          code: cause.code === Code.Unknown ? Code.Aborted : cause.code,
          reason: 'batch_partially_applied',
          metadata: {
            chunks_applied: String(chunks),
            rows_applied: String(rows),
            ...(cause.reason === undefined ? {} : { cause_reason: cause.reason }),
          },
          rpc: cause.rpc,
          cause,
          chunksApplied: chunks,
          rowsApplied: rows,
          token: token?.toString(),
        },
      );
    }
    chunks += 1;
    rows += chunk.length;
  };

  for await (const op of ops as AsyncIterable<Op>) {
    const size = sizeOf(op);
    if (pending.length > 0 && (pending.length >= chunkRows || pendingBytes + size > chunkBytes)) {
      await flush();
    }
    pending.push(op);
    pendingBytes += size;
  }
  await flush();
  return { token: token?.toString(), chunks, rows };
}
