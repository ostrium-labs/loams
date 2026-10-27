// Schema propagation, port type checks, validation, graph edits and the pipeline spec,
// without a DOM. The spec test feeds the serialized graph to @loams-core/flows' own
// compiler, so "compatible with flows" is checked by flows, not by a copy of its rules.

import { compileGraph } from '@loams-core/flows/src/compiler.ts';
import { describe, expect, it } from 'vite-plus/test';
import { FIXTURE_CATALOG } from '../src/fixtures.js';
import { emptyGraph, type PipelineGraph } from '../src/model.js';
import { addNode, connect, disconnect, moveNode, removeNodes, updateNode } from '../src/ops.js';
import {
  canConnect,
  castable,
  checkEdges,
  checkSchemas,
  convertible,
  propagate,
  schemaText,
} from '../src/schema.js';
import { autoLayout, fromPipelineSpec, secretRef, toPipelineSpec } from '../src/spec.js';
import { validateGraph } from '../src/validate.js';

const byId = new Map(FIXTURE_CATALOG.map((c) => [c.id, c]));
const lookup = (ref: string) => byId.get(ref);

function build(...steps: ((g: PipelineGraph) => PipelineGraph)[]): PipelineGraph {
  return steps.reduce((g, f) => f(g), emptyGraph('orders'));
}
const add = (kind: 'source' | 'sink' | 'transform', ref: string) => (g: PipelineGraph) =>
  addNode(g, { kind, ref, label: ref } as never).graph;
const link = (a: string, b: string) => (g: PipelineGraph) => {
  const r = connect(g, a, b);
  if (r.error) throw new Error(r.error);
  return r.graph;
};

describe('type rules', () => {
  it('widens without loss and refuses narrowing', () => {
    expect(convertible('int', 'float')).toBe(true);
    expect(convertible('int', 'string')).toBe(true);
    expect(convertible('timestamp', 'json')).toBe(true);
    expect(convertible('float', 'int')).toBe(false);
    expect(convertible('bytes', 'string')).toBe(false);
    expect(convertible('string', 'int')).toBe(false);
  });

  it('lets a schema mapping cast what a plain edge cannot', () => {
    expect(castable('string', 'int')).toBe(true);
    expect(castable('float', 'int')).toBe(true);
    expect(castable('bytes', 'int')).toBe(false);
  });

  it('reports a missing required field and a bad type, one problem each', () => {
    const check = checkSchemas(
      { fields: [{ name: 'amount', type: 'string' }] },
      {
        fields: [
          { name: 'order_id', type: 'int', required: true },
          { name: 'amount', type: 'float' },
          { name: 'note', type: 'string' },
        ],
      },
    );
    expect(check.ok).toBe(false);
    expect(check.problems).toEqual([
      'Missing required field order_id (int).',
      'Field amount is string; the sink needs float.',
    ]);
    expect(check.message).toBe('2 schema mismatches');
  });

  it('refuses an untyped stream into a typed sink, and accepts anything into an open one', () => {
    expect(
      checkSchemas({ open: true }, { fields: [{ name: 'x', type: 'int', required: true }] }).ok,
    ).toBe(false);
    expect(checkSchemas({ open: true }, { open: true }).ok).toBe(true);
  });
});

describe('ports', () => {
  const g = build(
    add('source', 'postgres-cdc'),
    add('transform', 'filter'),
    add('sink', 'clickhouse'),
  );

  it('has no input on a source, no output on a sink, and no self loop', () => {
    expect(canConnect(g, 'clickhouse-1', 'filter-1')).toEqual({
      ok: false,
      reason: 'A sink has no output.',
    });
    expect(canConnect(g, 'filter-1', 'postgres-cdc-1')).toEqual({
      ok: false,
      reason: 'A source has no input.',
    });
    expect(canConnect(g, 'filter-1', 'filter-1').ok).toBe(false);
  });

  it('refuses a cycle and a duplicate', () => {
    const two = build(
      add('transform', 'filter'),
      add('transform', 'map'),
      link('filter-1', 'map-1'),
    );
    expect(canConnect(two, 'map-1', 'filter-1')).toEqual({
      ok: false,
      reason: 'That connection would make a cycle.',
    });
    expect(canConnect(two, 'filter-1', 'map-1').ok).toBe(false);
  });

  it('draws a mismatched connection as an error edge rather than refusing it', () => {
    const bad = build(
      add('source', 'postgres-cdc'),
      add('sink', 'nats'),
      link('postgres-cdc-1', 'nats-1'),
    );
    const check = checkEdges(bad, lookup).get('postgres-cdc-1->nats-1');
    expect(check?.ok).toBe(false);
    expect(check?.problems).toContain('Missing required field data (bytes).');
  });
});

describe('schema propagation', () => {
  it('a map renames and projects, and a schema mapping casts', () => {
    let g = build(
      add('source', 'postgres-cdc'),
      add('transform', 'map'),
      add('transform', 'schema'),
      add('sink', 'clickhouse'),
      link('postgres-cdc-1', 'map-1'),
      link('map-1', 'schema-1'),
      link('schema-1', 'clickhouse-1'),
    );
    g = updateNode(g, 'map-1', {
      mappings: [
        { from: 'order_id', to: 'order_id' },
        { from: 'amount', to: 'amount' },
        { from: 'created_at', to: 'created_at' },
        { from: 'status', to: 'state' },
      ],
    });
    expect(schemaText(propagate(g, lookup).get('map-1'))).toBe(
      'order_id:int, amount:float, created_at:timestamp, state:string',
    );
    g = updateNode(g, 'schema-1', { casts: { amount: 'int' } });
    // float -> int narrows; ClickHouse wants float, and int widens back, so it fits.
    expect(checkEdges(g, lookup).get('schema-1->clickhouse-1')?.ok).toBe(true);
    g = updateNode(g, 'schema-1', { casts: { created_at: 'string' } });
    expect(checkEdges(g, lookup).get('schema-1->clickhouse-1')?.problems).toEqual([
      'Field created_at is string; the sink needs timestamp.',
    ]);
  });

  it('fails every input of a transform whose inputs disagree on a field type', () => {
    let g = build(
      add('source', 'postgres-cdc'),
      add('source', 'otlp'),
      add('transform', 'map'),
      add('transform', 'map'),
      add('transform', 'filter'),
      link('postgres-cdc-1', 'map-1'),
      link('otlp-1', 'map-2'),
      link('map-1', 'filter-1'),
      link('map-2', 'filter-1'),
    );
    g = updateNode(g, 'map-1', { mappings: [{ from: 'order_id', to: 'key' }] });
    g = updateNode(g, 'map-2', { mappings: [{ from: 'body', to: 'key' }] });
    const checks = checkEdges(g, lookup);
    for (const id of ['map-1->filter-1', 'map-2->filter-1']) {
      expect(checks.get(id)).toEqual({
        ok: false,
        message: 'Conflicting inputs',
        problems: ['Field key is int on one input and string on another.'],
      });
    }
    g = updateNode(g, 'map-2', { mappings: [{ from: 'body', to: 'text' }] });
    expect(checkEdges(g, lookup).get('map-2->filter-1')?.ok).toBe(true);
  });

  it('a filter passes its input through unchanged', () => {
    const g = build(add('source', 'otlp'), add('transform', 'filter'), link('otlp-1', 'filter-1'));
    const s = propagate(g, lookup);
    expect(s.get('filter-1')).toEqual(s.get('otlp-1'));
  });
});

describe('validation', () => {
  it('asks for a source and a sink on an empty graph', () => {
    const msgs = validateGraph(emptyGraph('x'), lookup).map((i) => i.message);
    expect(msgs).toEqual(expect.arrayContaining(['Add a source.', 'Add a sink.']));
  });

  it('names required config and unset required secrets', () => {
    const g = build(
      add('source', 'webhook'),
      add('sink', 'iceberg'),
      link('webhook-1', 'iceberg-1'),
    );
    const msgs = validateGraph(g, lookup).map((i) => i.message);
    expect(msgs).toContain('webhook: Path is required.');
    expect(msgs).toContain('webhook: Signing secret is required.');
    expect(msgs).toContain('iceberg: Catalog is required.');
    // Optional secrets are not required.
    expect(msgs.some((m) => m.includes('Catalog token'))).toBe(false);
  });

  it('passes a complete graph', () => {
    let g = build(add('source', 'webhook'), add('sink', 'iceberg'), link('webhook-1', 'iceberg-1'));
    g = updateNode(g, 'webhook-1', {
      config: { path: '/hooks/a' },
      secrets: { signing_secret: true },
    });
    g = updateNode(g, 'iceberg-1', { config: { catalog: 'main', table: 'events' } });
    expect(validateGraph(g, lookup)).toEqual([]);
  });

  it('refuses a bad pipeline name and dangling nodes', () => {
    const g = { ...build(add('source', 'webhook'), add('transform', 'filter')), name: 'Bad Name' };
    const msgs = validateGraph(g, lookup).map((i) => i.message);
    expect(msgs[0]).toMatch(/pipeline name/);
    expect(msgs).toContain('webhook goes nowhere; connect it to a sink.');
    expect(msgs).toContain('filter has no input.');
  });
});

describe('graph edits', () => {
  it('mints distinct ids and labels for the same connector', () => {
    const g = build(add('source', 'kafka'), add('source', 'kafka'));
    expect(g.nodes.map((n) => n.id)).toEqual(['kafka-1', 'kafka-2']);
    expect(g.nodes.map((n) => n.label)).toEqual(['kafka', 'kafka 2']);
  });

  it('removing a node removes its edges; a patch never changes identity', () => {
    let g = build(add('source', 'kafka'), add('sink', 's3'), link('kafka-1', 's3-1'));
    g = updateNode(g, 'kafka-1', { id: 'evil', kind: 'sink', label: 'Orders in' } as never);
    expect(g.nodes[0]).toMatchObject({ id: 'kafka-1', kind: 'source', label: 'Orders in' });
    expect(removeNodes(g, ['s3-1']).edges).toEqual([]);
    expect(disconnect(g, ['kafka-1->s3-1']).edges).toEqual([]);
  });

  it('ignores a non-finite move', () => {
    const g = build(add('source', 'kafka'));
    expect(moveNode(g, 'kafka-1', { x: Number.NaN, y: 1 })).toBe(g);
  });
});

describe('pipeline spec', () => {
  const complete = () => {
    let g = build(
      add('source', 'postgres-cdc'),
      add('transform', 'filter'),
      add('sink', 'clickhouse'),
      link('postgres-cdc-1', 'filter-1'),
      link('filter-1', 'clickhouse-1'),
    );
    g = updateNode(g, 'postgres-cdc-1', {
      config: { host: 'db', port: 5432 },
      secrets: { password: true },
    });
    g = updateNode(g, 'filter-1', { config: { expression: 'amount > 10' } });
    return g;
  };

  it('compiles with @loams-core/flows: every node and edge, nothing dropped', () => {
    const spec = toPipelineSpec(complete());
    const compiled = compileGraph(spec.graph, []);
    expect(compiled.diagnostics).toEqual({ droppedNodes: [], droppedEdges: [] });
    expect(compiled.nodes.map((n) => n.id)).toEqual(['postgres-cdc-1', 'filter-1', 'clickhouse-1']);
    expect(compiled.edges.map((e) => [e.source, e.target])).toEqual([
      ['postgres-cdc-1', 'filter-1'],
      ['filter-1', 'clickhouse-1'],
    ]);
    expect(compiled.nodes[0]?.className).toBe('pipeline-source');
  });

  it('carries secret refs, never values, and steps name their inputs', () => {
    const spec = toPipelineSpec(complete());
    const pg = spec.steps.find((s) => s.id === 'postgres-cdc-1');
    expect(pg?.secrets).toEqual({
      password: { secretRef: secretRef('orders', 'postgres-cdc-1', 'password') },
    });
    expect(pg?.uses).toBe('connector:postgres-cdc');
    expect(spec.steps.find((s) => s.id === 'filter-1')).toMatchObject({
      uses: 'transform:filter',
      inputs: ['postgres-cdc-1'],
    });
  });

  it('round-trips through fromPipelineSpec', () => {
    const g = complete();
    expect(fromPipelineSpec(toPipelineSpec(g))).toEqual(g);
  });

  it('animates edges only for a running pipeline', () => {
    expect(toPipelineSpec(complete()).graph.edges.every((e) => e.animated === undefined)).toBe(
      true,
    );
    expect(toPipelineSpec(complete(), { running: true }).graph.edges.every((e) => e.animated)).toBe(
      true,
    );
  });

  it('auto layout ranks left to right with flows ranker', () => {
    const laid = autoLayout(complete());
    const x = (id: string) => {
      const node = laid.nodes.find((n) => n.id === id);
      if (!node) throw new Error(`auto layout dropped ${id}`);
      return node.position.x;
    };
    expect(x('postgres-cdc-1')).toBe(0);
    expect(x('filter-1')).toBeGreaterThan(x('postgres-cdc-1'));
    expect(x('clickhouse-1')).toBeGreaterThan(x('filter-1'));
  });
});
