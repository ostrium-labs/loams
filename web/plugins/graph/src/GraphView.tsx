// The graph view (§48 §18.2): the Node, Relationship and Path values of a result drawn
// as a node-link diagram. At most 500 nodes are drawn; the rest stay in the table. The
// layout is a force-directed one from a fixed seed, so the same result always draws the
// same picture (stable screenshots). Nodes are buttons: Tab reaches them, Enter or a
// click shows their labels and properties beside the drawing.

import { create } from '@bufbuild/protobuf';
import { graph } from '@loams/proto';
import { useMemo, useState } from 'react';
import { type Elements, nodeText, relationshipText, valueText } from './values.js';

export const MAX_DRAWN_NODES = 500;
/** The layout's seed. Changing it changes every screenshot. */
export const LAYOUT_SEED = 0x6c6f616d;

const W = 1000;
const H = 640;

export interface DrawnNode {
  key: string;
  node: graph.Node;
  /** An endpoint the result did not return; drawn from its id alone. */
  stub: boolean;
  x: number;
  y: number;
}

export interface DrawnEdge {
  key: string;
  rel: graph.Relationship;
  from: DrawnNode;
  to: DrawnNode;
}

export interface Drawing {
  nodes: DrawnNode[];
  edges: DrawnEdge[];
  /** Nodes in the result, stubs not counted. */
  totalNodes: number;
}

/** mulberry32: a small seeded PRNG, the same sequence on every engine. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Picks the nodes to draw (capped), adds stub endpoints, and lays them out. Pure. */
export function layoutGraph(elements: Elements, seed = LAYOUT_SEED): Drawing {
  const picked = new Map<string, DrawnNode>();
  for (const [key, node] of elements.nodes) {
    if (picked.size >= MAX_DRAWN_NODES) break;
    picked.set(key, { key, node, stub: false, x: 0, y: 0 });
  }
  const edges: DrawnEdge[] = [];
  const endpoint = (id: bigint): DrawnNode | undefined => {
    const key = id.toString();
    const have = picked.get(key);
    if (have) return have;
    // An endpoint the result has but the cap left out is not drawn, nor is its edge.
    if (elements.nodes.has(key) || picked.size >= MAX_DRAWN_NODES) return undefined;
    const stub: DrawnNode = {
      key,
      node: create(graph.NodeSchema, { id }),
      stub: true,
      x: 0,
      y: 0,
    };
    picked.set(key, stub);
    return stub;
  };
  for (const [key, rel] of elements.relationships) {
    const from = endpoint(rel.src);
    const to = endpoint(rel.dst);
    if (from && to) edges.push({ key, rel, from, to });
  }

  const nodes = [...picked.values()];
  const n = nodes.length;
  const random = rng(seed);
  const px = new Float64Array(n);
  const py = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    px[i] = random() * W;
    py[i] = random() * H;
  }
  if (n > 1) relax(px, py, edgeIndexes(nodes, edges));
  fitToCanvas(px, py);
  nodes.forEach((d, i) => {
    d.x = at(px, i);
    d.y = at(py, i);
  });
  return { nodes, edges, totalNodes: elements.nodes.size };
}

/** A typed-array read; every index the layout uses is in range. */
const at = (a: Float64Array, i: number): number => a[i] ?? 0;

function edgeIndexes(nodes: DrawnNode[], edges: DrawnEdge[]): [number, number][] {
  const index = new Map(nodes.map((d, i) => [d, i]));
  return edges
    .map((e): [number, number] => [index.get(e.from) ?? 0, index.get(e.to) ?? 0])
    .filter(([a, b]) => a !== b);
}

/** Fruchterman-Reingold: all pairs repel, edges attract, a cooling step bounds moves. */
function relax(px: Float64Array, py: Float64Array, edges: [number, number][]) {
  const n = px.length;
  const k = Math.sqrt((W * H) / n) * 0.75;
  const iterations = n <= 100 ? 200 : Math.max(40, Math.floor(20_000 / n));
  const dx = new Float64Array(n);
  const dy = new Float64Array(n);
  const push = (i: number, x: number, y: number) => {
    dx[i] = at(dx, i) + x;
    dy[i] = at(dy, i) + y;
  };
  for (let it = 0; it < iterations; it++) {
    const temp = (W / 10) * (1 - it / iterations);
    dx.fill(0);
    dy.fill(0);
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        let ex = at(px, i) - at(px, j);
        let ey = at(py, i) - at(py, j);
        let dist = Math.hypot(ex, ey);
        if (dist < 0.01) {
          ex = 0.01 * (i - j);
          ey = 0.01;
          dist = Math.hypot(ex, ey);
        }
        const f = (k * k) / dist / dist;
        push(i, ex * f, ey * f);
        push(j, -ex * f, -ey * f);
      }
    }
    for (const [a, b] of edges) {
      const ex = at(px, a) - at(px, b);
      const ey = at(py, a) - at(py, b);
      const dist = Math.max(Math.hypot(ex, ey), 0.01);
      const f = dist / k;
      push(a, -ex * f, -ey * f);
      push(b, ex * f, ey * f);
    }
    for (let i = 0; i < n; i++) {
      const len = Math.hypot(at(dx, i), at(dy, i));
      let x = at(px, i);
      let y = at(py, i);
      if (len > 0) {
        const step = Math.min(len, temp);
        x += (at(dx, i) / len) * step;
        y += (at(dy, i) / len) * step;
      }
      // A gentle pull to the centre keeps disconnected parts on the canvas.
      px[i] = x + (W / 2 - x) * 0.01;
      py[i] = y + (H / 2 - y) * 0.01;
    }
  }
}

/** Scales the layout into the canvas with a margin, rounded so output is stable text. */
function fitToCanvas(px: Float64Array, py: Float64Array) {
  const n = px.length;
  if (n === 0) return;
  if (n === 1) {
    px[0] = W / 2;
    py[0] = H / 2;
    return;
  }
  const m = 48;
  const [x0, x1] = [Math.min(...px), Math.max(...px)];
  const [y0, y1] = [Math.min(...py), Math.max(...py)];
  const sx = (W - 2 * m) / Math.max(x1 - x0, 1);
  const sy = (H - 2 * m) / Math.max(y1 - y0, 1);
  for (let i = 0; i < n; i++) {
    px[i] = Math.round((m + (at(px, i) - x0) * sx) * 10) / 10;
    py[i] = Math.round((m + (at(py, i) - y0) * sy) * 10) / 10;
  }
}

const PALETTE = [
  'var(--op-accent)',
  'var(--op-grow)',
  'var(--op-info)',
  'var(--op-grow-deep)',
  'var(--op-danger)',
];

function colour(label: string | undefined): string {
  if (!label) return 'var(--op-faint)';
  let h = 0;
  for (const ch of label) h = (h * 31 + (ch.codePointAt(0) ?? 0)) >>> 0;
  return PALETTE[h % PALETTE.length] ?? 'var(--op-accent)';
}

/** The short caption under a node: its name or title, else its first label, else its id. */
export function caption(n: graph.Node): string {
  for (const key of ['name', 'title', 'label', 'id']) {
    const v = n.properties[key];
    if (v && v.kind.case === 'string') {
      return v.kind.value.length > 24 ? `${v.kind.value.slice(0, 23)}…` : v.kind.value;
    }
  }
  return n.labels[0] ?? `#${n.id}`;
}

export function GraphView({ elements }: { elements: Elements }) {
  const drawing = useMemo(() => layoutGraph(elements), [elements]);
  const [selected, setSelected] = useState<string>();
  const sel = drawing.nodes.find((d) => d.key === selected);
  const drawnReal = drawing.nodes.filter((d) => !d.stub).length;
  const capped = drawing.totalNodes > drawnReal;
  const showEdgeTypes = drawing.edges.length <= 200;

  return (
    <div className="flex flex-col gap-2">
      <p className="m-0 text-sm text-muted" role="status">
        {capped
          ? `Showing ${drawnReal} of ${drawing.totalNodes} nodes. The rest are in the table.`
          : `${drawnReal} ${drawnReal === 1 ? 'node' : 'nodes'}, ${drawing.edges.length} ${
              drawing.edges.length === 1 ? 'relationship' : 'relationships'
            }.`}
      </p>
      <div className="grid gap-3 lg:grid-cols-[minmax(0,1fr)_280px]">
        {/* biome-ignore lint/a11y/useSemanticElements: an <svg> holding focusable nodes; a fieldset cannot draw */}
        <svg
          viewBox={`0 0 ${W} ${H}`}
          className="h-[460px] w-full rounded-md border border-solid border-rule bg-surface"
          role="group"
          aria-label={`Graph view: ${drawnReal} nodes and ${drawing.edges.length} relationships`}
          data-testid="graph-view"
        >
          <defs>
            <marker
              id="gr-arrow"
              viewBox="0 0 10 10"
              refX="22"
              refY="5"
              markerWidth="7"
              markerHeight="7"
              orient="auto-start-reverse"
            >
              <path d="M0,0 L10,5 L0,10 z" fill="var(--op-muted)" />
            </marker>
          </defs>
          <g>
            {drawing.edges.map((e) => (
              <g key={e.key} data-edge={e.key}>
                <line
                  x1={e.from.x}
                  y1={e.from.y}
                  x2={e.to.x}
                  y2={e.to.y}
                  stroke="var(--op-muted)"
                  strokeWidth={1.2}
                  markerEnd="url(#gr-arrow)"
                >
                  <title>{relationshipText(e.rel)}</title>
                </line>
                {showEdgeTypes && (
                  <text
                    x={(e.from.x + e.to.x) / 2}
                    y={(e.from.y + e.to.y) / 2 - 4}
                    textAnchor="middle"
                    fontSize={11}
                    fill="var(--op-muted)"
                  >
                    {e.rel.type}
                  </text>
                )}
              </g>
            ))}
          </g>
          {drawing.nodes.map((d) => {
            const name = d.stub ? `node #${d.node.id}` : nodeText(d.node);
            const on = d.key === selected;
            return (
              // biome-ignore lint/a11y/useSemanticElements: an SVG group cannot be a <button>
              <g
                key={d.key}
                data-node={d.key}
                role="button"
                tabIndex={0}
                aria-label={d.stub ? `node ${d.node.id} (not in the result)` : name}
                aria-pressed={on}
                className="cursor-pointer outline-none focus-visible:[&>circle]:stroke-ink"
                onClick={() => setSelected(on ? undefined : d.key)}
                onKeyDown={(ev) => {
                  if (ev.key === 'Enter' || ev.key === ' ') {
                    ev.preventDefault();
                    setSelected(on ? undefined : d.key);
                  }
                }}
              >
                <circle
                  cx={d.x}
                  cy={d.y}
                  r={14}
                  fill={d.stub ? 'var(--op-bg)' : colour(d.node.labels[0])}
                  stroke={on ? 'var(--op-ink)' : 'var(--op-rule)'}
                  strokeWidth={on ? 3 : 1.5}
                  strokeDasharray={d.stub ? '3 3' : undefined}
                />
                <text x={d.x} y={d.y + 28} textAnchor="middle" fontSize={12} fill="var(--op-ink)">
                  {d.stub ? `#${d.node.id}` : caption(d.node)}
                </text>
              </g>
            );
          })}
        </svg>
        <aside
          className="rounded-md border border-solid border-rule p-3 text-sm"
          aria-label="Selected node"
          aria-live="polite"
        >
          {sel ? (
            <NodeDetails node={sel.node} stub={sel.stub} />
          ) : (
            <p className="m-0 text-muted">Select a node to see its labels and properties.</p>
          )}
        </aside>
      </div>
    </div>
  );
}

function NodeDetails({ node, stub }: { node: graph.Node; stub: boolean }) {
  const keys = Object.keys(node.properties).sort();
  return (
    <div className="flex flex-col gap-2">
      <h3 className="m-0 text-base">
        Node <span className="font-mono">#{node.id.toString()}</span>
      </h3>
      {stub ? (
        <p className="m-0 text-muted">The result names this node only as an endpoint.</p>
      ) : (
        <>
          <p className="m-0">
            {node.labels.length ? node.labels.map((l) => `:${l}`).join(' ') : 'No labels'}
          </p>
          <dl className="m-0 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1">
            {keys.map((k) => (
              <div key={k} className="contents">
                <dt className="text-muted">{k}</dt>
                <dd className="m-0 break-words font-mono">{valueText(node.properties[k])}</dd>
              </div>
            ))}
          </dl>
        </>
      )}
    </div>
  );
}
