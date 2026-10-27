// The fixture-backed client: the seeded project, creating services, persisting positions,
// and the status and metric streams on an interval.

import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { validServiceName } from '../src/client.js';
import { KINDS } from '../src/kinds.js';
import { createMockCanvasClient, rng, SEED_PROJECT_ID, seedProject } from '../src/mock.js';

afterEach(() => {
  vi.useRealTimers();
});

describe('seedProject', () => {
  const p = seedProject();

  it('holds every kind of service, with unique ids', () => {
    const kinds = new Set(p.services.map((s) => s.kind));
    expect([...kinds].sort()).toEqual(Object.keys(KINDS).sort());
    expect(new Set(p.services.map((s) => s.id)).size).toBe(p.services.length);
  });

  it('only connects services that exist, including connector source -> sink and Live -> Postgres', () => {
    const ids = new Set(p.services.map((s) => s.id));
    for (const e of p.edges) {
      expect(ids.has(e.source)).toBe(true);
      expect(ids.has(e.target)).toBe(true);
    }
    expect(p.edges).toContainEqual(
      expect.objectContaining({ source: 'live-api', target: 'pg-main', kind: 'depends-on' }),
    );
    expect(p.edges).toContainEqual(
      expect.objectContaining({
        source: 'sql-orders',
        target: 'conn-orders-cdc',
        kind: 'data-flow',
      }),
    );
    expect(p.edges).toContainEqual(
      expect.objectContaining({ source: 'conn-orders-cdc', target: 'house-analytics' }),
    );
  });
});

describe('createMockCanvasClient', () => {
  it('returns a copy of the project, not its store', async () => {
    const c = createMockCanvasClient();
    const a = await c.getProject(SEED_PROJECT_ID);
    (a.services as unknown[]).length = 0;
    expect((await c.getProject(SEED_PROJECT_ID)).services.length).toBeGreaterThan(0);
  });

  it('refuses an unknown project', async () => {
    await expect(createMockCanvasClient().getProject('nope')).rejects.toThrow('nope');
  });

  it('creates a deploying service, connected when asked', async () => {
    const c = createMockCanvasClient();
    const { service, edges } = await c.createService(SEED_PROJECT_ID, {
      kind: 'connector',
      name: 'pg-to-house',
      connectFrom: 'pg-main',
    });
    expect(service).toMatchObject({ kind: 'connector', name: 'pg-to-house', status: 'deploying' });
    expect(edges).toEqual([
      expect.objectContaining({ source: 'pg-main', target: service.id, kind: 'data-flow' }),
    ]);
    const p = c.peek();
    expect(p.services.map((s) => s.id)).toContain(service.id);
    expect(p.edges.map((e) => e.id)).toContain(edges[0]?.id);
  });

  it('refuses bad names, duplicates and unknown sources', async () => {
    const c = createMockCanvasClient();
    await expect(c.createService(SEED_PROJECT_ID, { kind: 'graph', name: '  ' })).rejects.toThrow(
      'Give the service a name.',
    );
    await expect(
      c.createService(SEED_PROJECT_ID, { kind: 'postgres-branch', name: 'main' }),
    ).rejects.toThrow('already exists');
    await expect(
      c.createService(SEED_PROJECT_ID, { kind: 'graph', name: 'g', connectFrom: 'ghost' }),
    ).rejects.toThrow('no longer exists');
  });

  it('merges saved positions, rounding them, and ignores unknown services', async () => {
    const c = createMockCanvasClient();
    await c.savePositions(SEED_PROJECT_ID, { 'pg-main': { x: 10.4, y: 20.6 } });
    await c.savePositions(SEED_PROJECT_ID, { 'live-web': { x: 1, y: 2 }, ghost: { x: 0, y: 0 } });
    expect(c.peek().positions).toEqual({ 'pg-main': { x: 10, y: 21 }, 'live-web': { x: 1, y: 2 } });
  });

  it('streams a back-filled metric, then a point per interval', async () => {
    vi.useFakeTimers();
    let t = 1_000_000;
    const c = createMockCanvasClient({ intervalMs: 1000, backfill: 5, now: () => t });
    const it = c.metricStream(SEED_PROJECT_ID, 'pg-main')[Symbol.asyncIterator]();
    const first = await it.next();
    expect(first.value).toHaveLength(5);
    expect(first.value?.at(-1)?.t).toBe(t);
    expect(first.value?.[0]?.t).toBe(t - 4000);
    const second = it.next();
    t += 1000;
    await vi.advanceTimersByTimeAsync(1000);
    const got = await second;
    expect(got.value).toEqual([{ t, v: expect.any(Number) }]);
    await it.return?.();
  });

  it('streams the same metric for the same seed', async () => {
    const take = async (seed: number) => {
      const c = createMockCanvasClient({ seed, backfill: 8, now: () => 0 });
      const it = c.metricStream(SEED_PROJECT_ID, 'sql-orders')[Symbol.asyncIterator]();
      const r = await it.next();
      await it.return?.();
      if (r.done) throw new Error('the metric stream ended');
      return r.value.map((p) => p.v);
    };
    expect(await take(3)).toEqual(await take(3));
    expect(await take(3)).not.toEqual(await take(4));
  });

  it('reports a new service healthy on the next status tick, and stops on abort', async () => {
    vi.useFakeTimers();
    const c = createMockCanvasClient({ intervalMs: 500 });
    const { service } = await c.createService(SEED_PROJECT_ID, { kind: 'house', name: 'lake' });
    const abort = new AbortController();
    const it = c.watchStatus(SEED_PROJECT_ID, abort.signal)[Symbol.asyncIterator]();
    const next = it.next();
    await vi.advanceTimersByTimeAsync(500);
    const batch = (await next).value;
    expect(batch).toContainEqual({ serviceId: service.id, status: 'healthy' });
    expect(c.peek().services.find((s) => s.id === service.id)?.status).toBe('healthy');
    const after = it.next();
    abort.abort();
    await vi.advanceTimersByTimeAsync(500);
    expect((await after).done).toBe(true);
  });
});

describe('validServiceName', () => {
  it('accepts plain names and explains the rest', () => {
    expect(validServiceName('orders_v2')).toBeUndefined();
    expect(validServiceName('-x')).toMatch(/starting with/);
    expect(validServiceName('a'.repeat(64))).toMatch(/63/);
  });
});

describe('rng', () => {
  it('stays in [0, 1)', () => {
    const r = rng(1);
    for (let i = 0; i < 1000; i++) {
      const v = r();
      expect(v).toBeGreaterThanOrEqual(0);
      expect(v).toBeLessThan(1);
    }
  });
});
