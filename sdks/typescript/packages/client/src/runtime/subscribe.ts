// `streams.subscribe` (runtime contract R14, SDKP §2.5 and SDKP-2).
//
//   const sub = loams.streams.subscribe({ stream: 'events', partition: 0, fromOffset: 1n });
//   for await (const record of sub) {
//     handle(record);
//     if (done) break;            // cancels the loop
//   }
//   resumeLater(sub.offset);       // the next offset to read
//
// A client loop over unary `Fetch` rather than a server stream, so it works on
// every transport (Connect-unary included) and through every proxy:
//
// - The next offset is the last record's offset + 1.
// - An empty fetch backs off 100 ms, doubling, up to 1 s, and the backoff
//   resets when a record arrives. When the caller passes `maxWaitMs`, the
//   server long-polls instead and the backoff only applies when it answers
//   empty at its deadline.
// - `offset` is always the next offset to read, so a caller can stop and
//   resume without losing or repeating a record.
// - Breaking out of `for await`, calling `return()`, or aborting `signal`
//   stops the loop; no further fetch is sent.
//
// A fetch that fails is retried by the call path as any `safe` call is; a
// failure that outlasts the retries ends the iteration with that error, and
// `offset` still says where to resume.

import { sleep } from './retry.js';

/** The first and the largest pause after an empty fetch. */
export const SUBSCRIBE_MIN_BACKOFF_MS = 100;
export const SUBSCRIBE_MAX_BACKOFF_MS = 1_000;

/** Where a subscription starts and how much each fetch asks for. */
export interface SubscribeRequest {
  namespace?: string;
  stream: string;
  partition?: number;
  /** The first offset to read; default 0. */
  fromOffset?: bigint | number;
  maxRecords?: number;
  maxBytes?: number;
  /** A server long poll per fetch, when the server offers it (P0c). */
  maxWaitMs?: number;
}

/** One record, as far as the loop needs to know it. */
export interface OffsetRecord {
  readonly offset: bigint | number | string;
}

/** One `Fetch`. */
export type RecordFetcher<R extends OffsetRecord> = (
  request: Record<string, unknown>,
  options?: { signal?: AbortSignal },
) => Promise<{ records?: readonly R[] } | undefined>;

/** A running subscription: an async iterable that knows where it is. */
export interface Subscription<R> extends AsyncIterableIterator<R> {
  /** The next offset to read. */
  readonly offset: bigint;
  /** Stops the loop; the same as `return()`. */
  cancel(): void;
}

function asBigInt(value: bigint | number | string | undefined): bigint {
  if (value === undefined) {
    return 0n;
  }
  return typeof value === 'bigint' ? value : BigInt(value);
}

/** Starts a subscription over `fetch`. Nothing is sent until the first
 * `next()`. */
export function subscribe<R extends OffsetRecord>(
  fetch: RecordFetcher<R>,
  request: SubscribeRequest,
  options: { signal?: AbortSignal } = {},
): Subscription<R> {
  let offset = asBigInt(request.fromOffset);
  let buffer: R[] = [];
  let backoff = SUBSCRIBE_MIN_BACKOFF_MS;
  let done = false;
  const controller = new AbortController();
  const onAbort = () => {
    done = true;
    controller.abort(options.signal?.reason);
  };
  if (options.signal?.aborted === true) {
    onAbort();
  } else {
    options.signal?.addEventListener('abort', onAbort, { once: true });
  }
  const finish = (): IteratorResult<R> => {
    done = true;
    options.signal?.removeEventListener('abort', onAbort);
    if (!controller.signal.aborted) {
      controller.abort();
    }
    return { done: true, value: undefined };
  };

  const fetchRequest = () => {
    const out: Record<string, unknown> = {
      stream: request.stream,
      partition: request.partition ?? 0,
      offset,
    };
    for (const key of ['namespace', 'maxRecords', 'maxBytes', 'maxWaitMs'] as const) {
      if (request[key] !== undefined) {
        out[key] = request[key];
      }
    }
    return out;
  };

  const subscription: Subscription<R> = {
    get offset() {
      return offset;
    },
    cancel() {
      finish();
    },
    async next(): Promise<IteratorResult<R>> {
      while (!done) {
        const record = buffer.shift();
        if (record !== undefined) {
          offset = asBigInt(record.offset) + 1n;
          return { done: false, value: record };
        }
        let answer: { records?: readonly R[] } | undefined;
        try {
          answer = await fetch(fetchRequest(), { signal: controller.signal });
        } catch (error) {
          if (done) {
            return finish();
          }
          finish();
          throw error;
        }
        if (done) {
          return finish();
        }
        // A record below the next offset is one already yielded (a server that
        // answers from a segment start); skipping it keeps resume exact.
        buffer = (answer?.records ?? []).filter((r) => asBigInt(r.offset) >= offset);
        if (buffer.length > 0) {
          backoff = SUBSCRIBE_MIN_BACKOFF_MS;
          continue;
        }
        try {
          await sleep(backoff, controller.signal);
        } catch {
          return finish();
        }
        backoff = Math.min(backoff * 2, SUBSCRIBE_MAX_BACKOFF_MS);
      }
      return finish();
    },
    return(): Promise<IteratorResult<R>> {
      return Promise.resolve(finish());
    },
    [Symbol.asyncIterator]() {
      return this;
    },
  };
  return subscription;
}
