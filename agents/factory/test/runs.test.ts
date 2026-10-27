// FT0 Task 9 (plan FT1): the run record (the Live app `factory`, `live/factory.js`, on a
// fake `loams:server`) and the `factory_events` CloudEvents, with no server.
import { describe, expect, it } from 'vite-plus/test';

import {
  createRunRecorder,
  fromLiveValue,
  liveRunStore,
  RUN_STAGES,
  type RunEvent,
  type RunStore,
  runEventType,
  streamProducer,
  type Transition,
  toLiveValue,
} from '../src/index.js';
import { call, FakeDb, type FunctionDef } from './support/loams-server.js';

const BUNDLE = '../live/factory.js';
const schema = (await import('../live/schema.json', { with: { type: 'json' } })).default as {
  tables: { name: string; indexes: { name: string; fields: string[] }[] }[];
};
const { runs } = (await import(BUNDLE)) as {
  runs: { record: FunctionDef; list: FunctionDef; get: FunctionDef };
};

/** Integers as Live hands them to a function: int64, a BigInt. */
const asLive = (x: unknown): unknown => {
  if (typeof x === 'number' && Number.isSafeInteger(x)) return BigInt(x);
  if (Array.isArray(x)) return x.map(asLive);
  if (x && typeof x === 'object') {
    return Object.fromEntries(Object.entries(x).map(([k, v]) => [k, asLive(v)]));
  }
  return x;
};
/** And back, as a client reads them. */
const asJs = (x: unknown): unknown => {
  if (typeof x === 'bigint') return Number(x);
  if (Array.isArray(x)) return x.map(asJs);
  if (x && typeof x === 'object') {
    return Object.fromEntries(Object.entries(x).map(([k, v]) => [k, asJs(v)]));
  }
  return x;
};

function liveApp() {
  const indexes = Object.fromEntries(
    schema.tables.map((t) => [
      t.name,
      Object.fromEntries(t.indexes.map((i) => [i.name, i.fields])),
    ]),
  );
  const db = new FakeDb(indexes);
  let clock = 1_000;
  const store: RunStore = {
    async record(t) {
      clock += 1;
      const realNow = Date.now;
      Date.now = () => clock;
      try {
        const out = (await call(runs.record, db, {
          run: t.run,
          seq: BigInt(t.seq),
          patch: asLive(t.patch),
        })) as { applied: boolean };
        return { applied: out.applied };
      } finally {
        Date.now = realNow;
      }
    },
  };
  const get = async (run: string) =>
    asJs(await call(runs.get, db, { run })) as Record<string, unknown> | null;
  const list = async () => asJs(await call(runs.list, db, {})) as Record<string, unknown>[];
  return { db, store, get, list };
}

/** A stream that dedupes by `source` + `id`, as `ProduceCloudEvents` does. */
function fakeStream() {
  const events: RunEvent[] = [];
  let duplicates = 0;
  const produce = async (event: RunEvent) => {
    if (events.some((e) => e.source === event.source && e.id === event.id)) duplicates += 1;
    else events.push(event);
  };
  return { events, produce, duplicates: () => duplicates };
}

/** One run through every stage, as the workflow records it. */
function transitions(run: string): Transition[] {
  const out: Transition[] = [
    {
      run,
      seq: 1,
      kind: 'opened',
      patch: { project: 'comet', intent: 'Rate limit signup', budget: { limit: 5, spent: 0 } },
    },
  ];
  for (const stage of RUN_STAGES.slice(1)) {
    out.push({
      run,
      seq: out.length + 1,
      kind: 'stage',
      patch: { stage, budget: { limit: 5, spent: out.length * 0.25 } },
    });
  }
  out.push({ run, seq: out.length + 1, kind: 'succeeded', patch: { state: 'succeeded' } });
  return out;
}

describe('the run record', () => {
  it('run_record_updates_each_stage', async () => {
    const app = liveApp();
    const stream = fakeStream();
    const recorder = createRunRecorder({
      store: app.store,
      produce: stream.produce,
      project: 'comet',
      now: () => new Date('2026-10-26T00:00:00Z'),
    });
    for (const t of transitions('run-1')) {
      expect(await recorder.transition(t)).toEqual({ applied: true });
      const doc = await app.get('run-1');
      expect(doc?.seq).toBe(t.seq);
      if (t.patch.stage) expect(doc?.stage).toBe(t.patch.stage);
    }
    const done = await app.get('run-1');
    expect(done).toMatchObject({
      run: 'run-1',
      project: 'comet',
      intent: 'Rate limit signup',
      stage: 'close',
      state: 'succeeded',
      budget: { limit: 5, spent: 1.75 },
    });
    expect((done?.updatedAt as number) > (done?.createdAt as number)).toBe(true);
    expect(stream.events.map((e) => e.type)).toEqual([
      runEventType('opened'),
      ...RUN_STAGES.slice(1).map(() => runEventType('stage')),
      runEventType('succeeded'),
    ]);
    expect(stream.events[0]).toMatchObject({
      specversion: '1.0',
      id: 'run-1/1',
      source: '/loams/factory/comet',
      subject: 'run-1',
      type: 'io.loams.dev.factory.run.opened.v1',
      datacontenttype: 'application/json',
    });
    // The newest run first.
    await recorder.transition({ run: 'run-2', seq: 1, kind: 'opened', patch: { intent: 'b' } });
    expect((await app.list()).map((r) => r.run)).toEqual(['run-2', 'run-1']);
  });

  it('events_are_idempotent', async () => {
    const app = liveApp();
    const stream = fakeStream();
    const recorder = createRunRecorder({
      store: app.store,
      produce: stream.produce,
      project: 'comet',
    });
    const all = transitions('run-3');
    for (const t of all.slice(0, 4)) await recorder.transition(t);
    const before = await app.get('run-3');
    // A crash and a replay from the start: every write is a no-op.
    for (const t of all.slice(0, 4)) {
      expect(await recorder.transition(t)).toEqual({ applied: false });
    }
    expect(await app.get('run-3')).toEqual(before);
    expect(stream.events).toHaveLength(4);
    expect(stream.duplicates()).toBe(4);
    expect(new Set(stream.events.map((e) => e.id)).size).toBe(4);
    // An older transition never overwrites a newer one.
    expect(
      await recorder.transition({
        run: 'run-3',
        seq: 2,
        kind: 'stage',
        patch: { stage: 'intent' },
      }),
    ).toEqual({ applied: false });
    expect((await app.get('run-3'))?.stage).toBe(before?.stage);
    // The rest of the run continues.
    for (const t of all.slice(4)) expect(await recorder.transition(t)).toEqual({ applied: true });
    expect(app.db.tables.get('factory_runs')).toHaveLength(1);
  });

  it('refuses unknown fields, stages and states', async () => {
    const app = liveApp();
    const bad = (patch: Record<string, unknown>) =>
      app.store.record({ run: 'r', seq: 1, kind: 'opened', patch: patch as never });
    await expect(bad({ token: 'x' })).rejects.toThrow(/unknown field token/);
    await expect(bad({ stage: 'ship' })).rejects.toThrow(/unknown stage/);
    await expect(bad({ state: 'done' })).rejects.toThrow(/unknown state/);
    const recorder = createRunRecorder({ store: app.store, produce: async () => {}, project: 'p' });
    await expect(
      recorder.transition({ run: 'r', seq: 0, kind: 'opened', patch: {} }),
    ).rejects.toThrow(/positive seq/);
  });
});

describe('the clients', () => {
  it('liveRunStore calls runs:record on the factory app over Connect JSON', async () => {
    const seen: { url: string; init: RequestInit }[] = [];
    const fetchFn = (async (url: string, init: RequestInit) => {
      seen.push({ url, init });
      return new Response(
        JSON.stringify({ commitTs: '9', result: toLiveValue({ applied: true, seq: 2 }) }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      );
    }) as unknown as typeof fetch;
    const store = liveRunStore('http://127.0.0.1:8080/', fetchFn);
    const out = await store.record({ run: 'r', seq: 2, kind: 'stage', patch: { stage: 'plan' } });
    expect(out).toEqual({ applied: true });
    expect(seen[0]?.url).toBe('http://127.0.0.1:8080/loams.live.v1.LiveService/Mutate');
    const headers = (seen[0]?.init.headers ?? {}) as Record<string, string>;
    expect(headers['loams-live-app']).toBe('factory');
    expect(headers.authorization).toBeUndefined();
    const body = JSON.parse(String(seen[0]?.init.body));
    expect(body.function).toBe('runs:record');
    expect(body.idempotencyKey).toBe('r/2');
    expect(fromLiveValue(body.args)).toEqual({ run: 'r', seq: 2, patch: { stage: 'plan' } });
    expect(body.args.objectValue.fields.seq).toEqual({ int64Value: '2' });
  });

  it('streamProducer posts a structured CloudEvent to factory_events', async () => {
    const seen: { url: string; init: RequestInit }[] = [];
    const fetchFn = (async (url: string, init: RequestInit) => {
      seen.push({ url, init });
      return new Response('{"events":[{"status":"EVENT_STATUS_APPENDED"}]}', { status: 200 });
    }) as unknown as typeof fetch;
    const produce = streamProducer('http://h:1', 'default', fetchFn);
    const recorder = createRunRecorder({
      store: { record: async () => ({ applied: true }) },
      produce,
      project: 'comet',
    });
    await recorder.transition({ run: 'r', seq: 1, kind: 'opened', patch: {} });
    expect(seen[0]?.url).toBe(
      'http://h:1/v1/namespaces/default/streams/factory_events/events?partition=0',
    );
    expect((seen[0]?.init.headers as Record<string, string> | undefined)?.['content-type']).toBe(
      'application/cloudevents+json',
    );
    expect(JSON.parse(String(seen[0]?.init.body)).id).toBe('r/1');
  });

  it('Live values round-trip', () => {
    const x = { a: 1, b: 1.5, c: 'x', d: [true, false], e: { f: -3 } };
    expect(fromLiveValue(toLiveValue(x))).toEqual(x);
  });
});
