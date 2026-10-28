// The mock client: the metric stream emits a backfill, then one point per interval,
// and ends when the deployment stops; deploy records secret refs, never values.

import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { createMockPipelinesClient } from '../src/mock.js';
import { emptyGraph } from '../src/model.js';
import { addNode, connect } from '../src/ops.js';
import { secretRef, toPipelineSpec } from '../src/spec.js';

afterEach(() => {
  vi.useRealTimers();
});

function spec() {
  let g = emptyGraph('m');
  g = addNode(g, { kind: 'source', ref: 'kafka', label: 'Kafka' }).graph;
  g = addNode(g, { kind: 'sink', ref: 's3', label: 'S3' }).graph;
  g = connect(g, 'kafka-1', 's3-1').graph;
  g = {
    ...g,
    nodes: g.nodes.map((n) =>
      n.id === 's3-1' ? { ...n, secrets: { secret_access_key: true } } : n,
    ),
  };
  return toPipelineSpec(g);
}

describe('mock pipelines client', () => {
  it('serves the fixture catalog with every CN2 family represented', async () => {
    const client = createMockPipelinesClient();
    const families = new Set((await client.catalog()).map((c) => c.family));
    for (const f of [
      'kafka',
      'postgres',
      'mysql',
      'objects',
      'analytics',
      'brokers',
      'cloud',
      'http',
    ]) {
      expect(families.has(f as never)).toBe(true);
    }
  });

  it('emits a backfill, then one point per interval, and ends on stop', async () => {
    vi.useFakeTimers();
    let t = 1_000_000;
    const client = createMockPipelinesClient({
      intervalMs: 500,
      deployMs: 0,
      now: () => t,
      random: () => 0.5,
    });
    const deploying = client.deploy(spec(), new Map());
    await vi.advanceTimersByTimeAsync(0);
    const d = await deploying;

    const it = client.metrics(d.id, 'kafka-1', 'throughput')[Symbol.asyncIterator]();
    const first = await it.next();
    if (first.done) throw new Error('the metric stream ended before its first batch');
    expect(first.value).toHaveLength(10);
    expect(first.value.every((p) => p.series === 'throughput' && p.v >= 0)).toBe(true);

    const second = it.next();
    t += 500;
    await vi.advanceTimersByTimeAsync(500);
    expect((await second).value).toEqual([{ t, v: expect.any(Number), series: 'throughput' }]);

    await client.stop(d.id);
    const third = it.next();
    await vi.advanceTimersByTimeAsync(500);
    expect((await third).done).toBe(true);
  });

  it('a redeploy stops the deployment it supersedes and bumps the revision', async () => {
    const client = createMockPipelinesClient({ deployMs: 0 });
    const first = await client.deploy(spec(), new Map());
    const second = await client.deploy(spec(), new Map());
    const third = await client.deploy(spec(), new Map());
    expect([first.revision, second.revision, third.revision]).toEqual([1, 2, 3]);
    expect((await client.stop(first.id)).state).toBe('stopped');
    const it = client.metrics(second.id, 'kafka-1', 'lag')[Symbol.asyncIterator]();
    expect((await it.next()).done).toBe(true);
  });

  it('yields nothing for a node the deployment does not have', async () => {
    const client = createMockPipelinesClient({ deployMs: 0 });
    const d = await client.deploy(spec(), new Map());
    const it = client.metrics(d.id, 'nope', 'lag')[Symbol.asyncIterator]();
    expect((await it.next()).done).toBe(true);
    await client.stop(d.id);
  });

  it('records secret refs on deploy, never the values', async () => {
    const client = createMockPipelinesClient({ deployMs: 0 });
    const ref = secretRef('m', 's3-1', 'secret_access_key');
    const d = await client.deploy(spec(), new Map([[ref, 'value-that-must-not-leak']]));
    expect(d.state).toBe('running');
    expect(client.calls.find((c) => c.method === 'deploy')?.arg).toEqual({
      name: 'm',
      secretRefs: [ref],
    });
    expect(JSON.stringify(client.calls)).not.toContain('value-that-must-not-leak');
    await client.stop(d.id);
  });

  it('fails a deploy on request', async () => {
    const client = createMockPipelinesClient({ deployMs: 0, failDeploy: 'quota exceeded' });
    await expect(client.deploy(spec(), new Map())).rejects.toThrow('quota exceeded');
  });
});
