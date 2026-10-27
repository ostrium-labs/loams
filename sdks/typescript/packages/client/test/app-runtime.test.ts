// SDKP Part A for TypeScript: the runtime clauses R11–R14, the collection
// handle and `search.queryAll`, each pinned against a stub so no server is
// needed. The canonical names are what `check-languages.mjs --suite app` looks
// for:
//
// - `typescript_default_namespace` (R11)
// - `typescript_timeout_deadline` (R12)
// - `typescript_write_batch_chunks` (R13)
// - `typescript_streams_subscribe_resumes` (R14)
// - `typescript_collection_handle_delegates` (SDKP §2.8)
// - `typescript_search_query_all_requires_tiebreak` (SDKP §2.4)

import { Code, ConnectError } from '@connectrpc/connect';
import { DocumentService } from '@loams/proto/documents';
import { InstanceService } from '@loams/proto/instance';
import { describe, expect, it } from 'vite-plus/test';
import { documentCalls, searchCalls, type UnaryCall } from '../src/app.js';
import { Filter } from '../src/builders/filter.js';
import { Search, Sort } from '../src/builders/search.js';
import { MODULES, SERVICE_DESCRIPTORS } from '../src/gen/facade.js';
import { CollectionHandle } from '../src/handle.js';
import { Loams } from '../src/loams.js';
import { CallInvoker } from '../src/runtime/call.js';
import {
  BatchError,
  DeadlineExceededError,
  InvalidArgumentError,
} from '../src/runtime/errors.js';
import { subscribe } from '../src/runtime/subscribe.js';
import { GET, WRITE, messageTransport } from './helpers/app.js';

const GET_INSTANCE = MODULES.find((module) => module.name === 'instance')!.calls[0]!;

function documentsInvoker(
  transport: unknown,
  defaults: ConstructorParameters<typeof CallInvoker>[4] = {},
  maxRetries = 0,
): CallInvoker {
  const invoker = new CallInvoker(transport as never, undefined, maxRetries, undefined, defaults);
  invoker.register('loams.collection.v1.DocumentService', DocumentService);
  invoker.register(GET_INSTANCE.service, InstanceService);
  return invoker;
}

describe('typescript_default_namespace', () => {
  it('fills an empty namespace from the client default', async () => {
    const { sent, transport } = messageTransport(() => ({}));
    const invoker = documentsInvoker(transport, { namespace: 'app' });
    await invoker.unary(GET, { collection: 'notes', ids: [] });
    expect(sent[0]?.input.namespace).toBe('app');
  });

  it('lets an explicit namespace win', async () => {
    const { sent, transport } = messageTransport(() => ({}));
    const invoker = documentsInvoker(transport, { namespace: 'app' });
    await invoker.unary(GET, { namespace: 'other', collection: 'notes', ids: [] });
    expect(sent[0]?.input.namespace).toBe('other');
  });

  it('refuses a namespaced call with neither, before any network call', async () => {
    const { sent, transport } = messageTransport(() => ({}));
    const invoker = documentsInvoker(transport);
    const refused = await invoker.unary(GET, { collection: 'notes', ids: [] }).catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(InvalidArgumentError);
    expect((refused as InvalidArgumentError).reason).toBe('namespace_required');
    expect(sent).toHaveLength(0);
  });

  it('leaves a request without a namespace field alone', async () => {
    const { sent, transport } = messageTransport(() => ({}));
    const invoker = documentsInvoker(transport);
    await invoker.unary(GET_INSTANCE, {});
    expect(sent[0]?.input).toEqual({});
  });

  it('leaves a namespace field outside the app packages alone', async () => {
    // `loams.live.v1` `CreateAppRequest.namespace` is free text naming an
    // app's environment, not a Loams namespace: never filled, never refused.
    const createApp = MODULES.find((module) => module.name === 'live.admin')!.calls.find(
      (call) => call.method === 'CreateApp',
    )!;
    const { sent, transport } = messageTransport(() => ({}));
    const invoker = new CallInvoker(transport as never, undefined, 0, undefined, { namespace: 'app' });
    invoker.register(createApp.service, SERVICE_DESCRIPTORS[createApp.service]!);
    await invoker.unary(createApp, { name: 'blog' });
    expect(sent[0]?.input).toEqual({ name: 'blog' });
    const bare = new CallInvoker(transport as never, undefined, 0, undefined);
    bare.register(createApp.service, SERVICE_DESCRIPTORS[createApp.service]!);
    await expect(bare.unary(createApp, { name: 'blog' })).resolves.toBeDefined();
  });

  it('is the client option and LOAMS_NAMESPACE', () => {
    expect(new Loams({ endpoint: 'http://127.0.0.1:1', namespace: 'app' }).namespace).toBe('app');
    const fromEnv = Loams.fromEnv({}, { LOAMS_ENDPOINT: 'http://127.0.0.1:1', LOAMS_NAMESPACE: 'env' });
    expect(fromEnv.namespace).toBe('env');
    expect(() => Loams.fromEnv({}, {})).toThrow(/LOAMS_ENDPOINT/);
  });
});

/** A transport that answers only when its signal aborts, or after `ms`. */
function slowTransport(ms: number) {
  let calls = 0;
  const seenTimeouts: (number | undefined)[] = [];
  return {
    calls: () => calls,
    seenTimeouts,
    transport: {
      unary(_m: unknown, signal: AbortSignal | undefined, timeoutMs: number | undefined) {
        calls += 1;
        seenTimeouts.push(timeoutMs);
        return new Promise((resolve, reject) => {
          const timer = setTimeout(() => resolve({ message: {} }), ms);
          signal?.addEventListener('abort', () => {
            clearTimeout(timer);
            reject(new ConnectError('aborted', Code.Canceled));
          });
        }) as never;
      },
      stream() {
        throw new Error('no streams');
      },
    },
  };
}

describe('typescript_timeout_deadline', () => {
  it('raises client_deadline_exceeded and does not retry', async () => {
    const slow = slowTransport(10_000);
    const invoker = documentsInvoker(slow.transport, { namespace: 'app', timeoutMs: 40 }, 3);
    const started = Date.now();
    const error = await invoker.unary(GET, { collection: 'notes', ids: [] }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(DeadlineExceededError);
    expect((error as DeadlineExceededError).reason).toBe('client_deadline_exceeded');
    expect((error as DeadlineExceededError).code).toBe(Code.DeadlineExceeded);
    expect(slow.calls()).toBe(1);
    expect(Date.now() - started).toBeLessThan(2_000);
    // The budget goes out as the transport's timeout, so the server sees it as
    // `connect-timeout-ms`.
    expect(slow.seenTimeouts[0]).toBeGreaterThan(0);
    expect(slow.seenTimeouts[0]).toBeLessThanOrEqual(40);
  });

  it('takes a per-call override, and 0 means no deadline', async () => {
    const slow = slowTransport(60);
    const invoker = documentsInvoker(slow.transport, { namespace: 'app', timeoutMs: 20 });
    await expect(invoker.unary(GET, { collection: 'notes', ids: [] }, { timeoutMs: 0 })).resolves.toEqual({});
    await expect(
      invoker.unary(GET, { collection: 'notes', ids: [] }, { timeoutMs: 500 }),
    ).resolves.toEqual({});
    await expect(invoker.unary(GET, { collection: 'notes', ids: [] })).rejects.toBeInstanceOf(
      DeadlineExceededError,
    );
  });

  it('defaults unary calls to 30 s', async () => {
    const slow = slowTransport(0);
    const invoker = documentsInvoker(slow.transport, { namespace: 'app' });
    await invoker.unary(GET, { collection: 'notes', ids: [] });
    expect(slow.seenTimeouts[0]).toBeGreaterThan(29_000);
    expect(slow.seenTimeouts[0]).toBeLessThanOrEqual(30_000);
  });
});

describe('typescript_write_batch_chunks', () => {
  const docs = (n: number) =>
    Array.from({ length: n }, (_, i) => ({ id: i + 1, source: { title: `note ${i + 1}` } }));

  it('chunks by rows, one key per chunk, and merges the tokens', async () => {
    let n = 0;
    const { sent, transport } = messageTransport(() => {
      n += 1;
      return { token: `v1:s1/p${n % 2}@${n * 10}` };
    });
    const invoker = documentsInvoker(transport, { namespace: 'app' });
    const documents = documentCalls({ write: (r, o) => invoker.unary(WRITE, r, o) });

    const result = await documents.writeBatch('notes', docs(5), { chunkRows: 2 });
    expect(result.chunks).toBe(3);
    expect(result.rows).toBe(5);
    expect(sent.map((s) => (s.input.ops as unknown[]).length)).toEqual([2, 2, 1]);
    const keys = sent.map((s) => s.input.idempotencyKey);
    expect(new Set(keys).size).toBe(3);
    // p1@10, p0@20, p1@30: the max per partition.
    expect(result.token).toBe('v1:s1/p0@20,s1/p1@30');
  });

  it('chunks by bytes', async () => {
    const { sent, transport } = messageTransport(() => ({}));
    const invoker = documentsInvoker(transport, { namespace: 'app' });
    const documents = documentCalls({ write: (r, o) => invoker.unary(WRITE, r, o) });
    const big = (id: number) => ({ id, source: { body: 'x'.repeat(400) } });
    await documents.writeBatch('notes', [big(1), big(2), big(3)], { chunkBytes: 1_000 });
    expect(sent.map((s) => (s.input.ops as unknown[]).length)).toEqual([2, 1]);
  });

  it('reuses a chunk key on that chunk\'s retry', async () => {
    let n = 0;
    const keys: unknown[] = [];
    const transport = {
      unary(_m: unknown, _s: unknown, _t: unknown, _h: unknown, input: Record<string, unknown>) {
        n += 1;
        keys.push(input.idempotencyKey);
        if (n === 1) {
          return Promise.reject(new ConnectError('restarting', Code.Unavailable));
        }
        return Promise.resolve({ message: {} }) as never;
      },
      stream() {
        throw new Error('no streams');
      },
    };
    const invoker = documentsInvoker(transport, { namespace: 'app' }, 1);
    const documents = documentCalls({ write: (r, o) => invoker.unary(WRITE, r, o) });
    await documents.writeBatch('notes', docs(1));
    expect(keys).toHaveLength(2);
    expect(keys[0]).toBe(keys[1]);
  });

  it('stops at the first failed chunk and says how much was applied', async () => {
    let n = 0;
    const transport = {
      unary() {
        n += 1;
        if (n === 2) {
          return Promise.reject(new ConnectError('bad doc', Code.InvalidArgument));
        }
        return Promise.resolve({ message: { token: 'v1:s1/p0@5' } }) as never;
      },
      stream() {
        throw new Error('no streams');
      },
    };
    const invoker = documentsInvoker(transport, { namespace: 'app' });
    const documents = documentCalls({ write: (r, o) => invoker.unary(WRITE, r, o) });
    const error = await documents.writeBatch('notes', docs(6), { chunkRows: 2 }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(BatchError);
    const batch = error as BatchError;
    expect(batch.reason).toBe('batch_partially_applied');
    expect(batch.code).toBe(Code.InvalidArgument);
    expect(batch.chunksApplied).toBe(1);
    expect(batch.rowsApplied).toBe(2);
    expect(batch.token).toBe('v1:s1/p0@5');
    expect(n).toBe(2);
  });
});

describe('typescript_streams_subscribe_resumes', () => {
  /** A partition holding offsets 0..4, served two at a time. */
  const log = () => {
    const requests: Record<string, unknown>[] = [];
    const fetch = async (request: Record<string, unknown>) => {
      requests.push(request);
      const from = Number(request.offset as bigint);
      const records = [0, 1, 2, 3, 4]
        .filter((offset) => offset >= from)
        .slice(0, 2)
        .map((offset) => ({ offset: BigInt(offset), value: `r${offset}` }));
      return { records };
    };
    return { fetch, requests };
  };

  it('yields from the offset, and resumes where it stopped', async () => {
    const { fetch, requests } = log();
    const sub = subscribe(fetch, { stream: 'events', partition: 0, fromOffset: 1n });
    const seen: string[] = [];
    for await (const record of sub) {
      seen.push(record.value);
      if (seen.length === 3) {
        break;
      }
    }
    expect(seen).toEqual(['r1', 'r2', 'r3']);
    expect(sub.offset).toBe(4n);
    expect(requests.map((r) => r.offset)).toEqual([1n, 3n]);

    const resumed = subscribe(fetch, { stream: 'events', fromOffset: sub.offset });
    const next = await resumed.next();
    expect(next.value?.value).toBe('r4');
    resumed.cancel();
    expect((await resumed.next()).done).toBe(true);
  });

  it('backs off on an empty fetch and stops when cancelled', async () => {
    let fetches = 0;
    const fetch = async () => {
      fetches += 1;
      return { records: [] as { offset: bigint }[] };
    };
    const controller = new AbortController();
    const sub = subscribe(fetch, { stream: 'events' }, { signal: controller.signal });
    const pending = sub.next();
    await new Promise((resolve) => setTimeout(resolve, 250));
    controller.abort();
    expect((await pending).done).toBe(true);
    // 100 ms then 200 ms of backoff fit in 250 ms: two or three fetches, not a
    // busy loop.
    expect(fetches).toBeGreaterThanOrEqual(2);
    expect(fetches).toBeLessThanOrEqual(3);
  });
});

describe('typescript_collection_handle_delegates', () => {
  it('fills collection and namespace into every call', async () => {
    const calls: [string, unknown[]][] = [];
    const record =
      (name: string) =>
      (...args: unknown[]) => {
        calls.push([name, args]);
        return Promise.resolve({});
      };
    const modules = {
      documents: {
        upsert: record('upsert'),
        delete: record('delete'),
        patch: record('patch'),
        writeBatch: record('writeBatch'),
        get: record('get'),
        count: record('count'),
        scrollAll: (async function* (...args: unknown[]) {
          calls.push(['scrollAll', args]);
        }) as never,
      },
      search: { query: record('query') },
      collections: { get: record('info'), drop: record('drop') },
    };
    const notes = new CollectionHandle(modules as never, 'notes', { namespace: 'app' });
    const target = { collection: 'notes', namespace: 'app' };

    await notes.upsert([{ id: 1 }]);
    await notes.delete([1]);
    await notes.patch(1, { set: { a: 1 } });
    await notes.writeBatch([{ id: 2 }]);
    await notes.get([1]);
    await notes.count({ where: Filter.eq('tags', 'x') });
    for await (const _ of notes.scrollAll()) {
      // drained
    }
    await notes.search(Search.on('ignored').text('refund'));
    await notes.vectorSearch('embedding', [1, 0, 0, 0], { limit: 3 });
    await notes.info();
    await notes.drop();

    const byName = Object.fromEntries(calls.map(([name, args]) => [name, args]));
    for (const name of ['upsert', 'delete', 'patch', 'writeBatch', 'scrollAll']) {
      expect(byName[name]?.[0], name).toEqual(target);
    }
    for (const name of ['get', 'count', 'info', 'drop']) {
      expect(byName[name]?.[0], name).toMatchObject(target);
    }
    const searched = calls.filter(([name]) => name === 'query').map(([, args]) => args[0]);
    expect(searched).toHaveLength(2);
    for (const request of searched) {
      expect(request).toMatchObject(target);
    }
  });

  it('is what loams.collection returns', () => {
    const loams = new Loams({ endpoint: 'http://127.0.0.1:1', namespace: 'app' });
    const handle = loams.collection('notes');
    expect(handle).toBeInstanceOf(CollectionHandle);
    expect(handle.name).toBe('notes');
  });
});

describe('typescript_search_query_all_requires_tiebreak', () => {
  it('refuses a sort without a primary-key tiebreaker, before any call', async () => {
    let calls = 0;
    const search = searchCalls({ query: (() => { calls += 1; return Promise.resolve({ hits: [] }); }) as UnaryCall });
    const noSort = Search.on('notes').text('refund').limit(2);
    const refused = await search.queryAll(noSort).next().catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(InvalidArgumentError);
    expect((refused as InvalidArgumentError).reason).toBe('search_after_needs_tiebreak');
    const scoreOnly = noSort.sort(Sort.score());
    await expect(search.queryAll(scoreOnly).next()).rejects.toBeInstanceOf(InvalidArgumentError);
    expect(calls).toBe(0);
  });

  it('pages with search_after until a short page', async () => {
    const requests: { searchAfter: unknown[] }[] = [];
    const pages = [
      [1, 2],
      [3, 4],
      [5],
    ];
    const search = searchCalls({
      query: ((request: { searchAfter: unknown[] }) => {
        requests.push(request);
        const page = pages[requests.length - 1] ?? [];
        return Promise.resolve({
          hits: page.map((pk) => ({ pk, sortValues: [{ kind: { case: 'numberValue', value: pk } }] })),
        });
      }) as UnaryCall,
    });
    const builder = Search.on('notes').text('refund').sort(Sort.score(), Sort.pk()).limit(2);
    const seen: unknown[] = [];
    for await (const hit of search.queryAll(builder)) {
      seen.push((hit as { pk: number }).pk);
    }
    expect(seen).toEqual([1, 2, 3, 4, 5]);
    expect(requests).toHaveLength(3);
    expect(requests[0]?.searchAfter).toEqual([]);
    expect(requests[1]?.searchAfter.length).toBe(1);
  });
});
