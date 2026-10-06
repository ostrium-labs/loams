/**
 * GraphSpec + rows -> the `{ nodes, edges }` pair React Flow mounts.
 *
 * This module is the reason `graph` is a sibling `Widget.type` and not a new
 * `ChartSchema.kind`. `plugin-echarts-render`'s registry is keyed on chart kinds
 * and every entry's `compile` returns an ECharts option object; React Flow wants
 * a nodes/edges pair that it renders through its own React tree, with its own
 * drag, zoom and pan state. There is no function-returning-an-options-blob that
 * bridges those two shapes, so the graph widget gets its own terminal value and
 * its own package.
 *
 * The compiler is pure: no side effects, no network, no theme lookup. It is fed
 * a spec (already validated, or not -- see below) and the rows the widget's
 * query returned, and it returns the arrays.
 *
 * DEFENSIVE BY POSITION, not by defensive by nature. Every input here crosses a
 * trust boundary: a dashboard spec is hand-authored JSON, and a preview route
 * accepts whatever body it is posted. So this module treats its spec as
 * untrusted and re-checks what zod checks for it. That is not duplication to be
 * removed -- it is what makes the same function safe on both a validated spec
 * and a raw one, and it is why a dangling endpoint is dropped here rather than
 * mounting an edge React Flow silently renders as nothing.
 */
import type { GraphLayout, GraphNode } from "@loams-plugins/types";

/** One node, in the shape React Flow's `nodes` prop takes. */
export interface FlowNode {
  id: string;
  /** `default` is React Flow's stock node: a titled box with source/target handles. */
  type: "default";
  position: { x: number; y: number };
  data: { label: string; [key: string]: unknown };
  style?: Record<string, string | number>;
  className?: string;
  width?: number;
  height?: number;
}

/** One edge, in the shape React Flow's `edges` prop takes. */
export interface FlowEdge {
  id: string;
  source: string;
  target: string;
  type: "default" | "straight" | "smoothstep" | "simplebezier";
  label?: string;
  animated: boolean;
  style?: Record<string, string | number>;
  markerEnd?: { type: string };
}

/**
 * What the compiler refused to draw, and why it is not an error.
 *
 * A dropped edge is a spec that disagrees with itself. React Flow's failure mode
 * for that is to render a graph with a connection silently missing, which is
 * worse than a shorter list -- so the ids are reported instead, and the service
 * turns them into one log line.
 */
export interface GraphDiagnostics {
  droppedNodes: string[];
  droppedEdges: string[];
}

/** The compiler's terminal value. Mirrors React Flow's two props, plus the above. */
export interface CompiledGraph {
  nodes: FlowNode[];
  edges: FlowEdge[];
  diagnostics: GraphDiagnostics;
}

export interface CompileGraphOptions {
  /** Cycled over node index for the default fill. Omitted means "paint nothing". */
  palette?: string[];
}

/** The layout used when the spec names none, or names one with holes in it. */
export const DEFAULT_GRAPH_LAYOUT: GraphLayout = {
  direction: "LR",
  nodeWidth: 180,
  nodeHeight: 48,
  rankSep: 80,
  nodeSep: 32,
};

type Dict = Record<string, unknown>;

function isPlainObject(value: unknown): value is Dict {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function asPositive(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : fallback;
}

function asNonNegative(value: unknown, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : fallback;
}

/**
 * Fill in every layout field, whatever the spec managed to say.
 *
 * Written as a per-field merge rather than a spread over the parsed layout,
 * because on an untrusted spec any individual field may be a wrong type or a
 * non-finite number while its siblings are fine -- and one bad field should cost
 * that field's default, not the whole layout.
 */
function readLayout(value: unknown): GraphLayout {
  const layout = isPlainObject(value) ? value : {};
  const direction = layout.direction;
  return {
    direction:
      direction === "TB" || direction === "LR" || direction === "BT" || direction === "RL"
        ? direction
        : DEFAULT_GRAPH_LAYOUT.direction,
    nodeWidth: asPositive(layout.nodeWidth, DEFAULT_GRAPH_LAYOUT.nodeWidth),
    nodeHeight: asPositive(layout.nodeHeight, DEFAULT_GRAPH_LAYOUT.nodeHeight),
    rankSep: asNonNegative(layout.rankSep, DEFAULT_GRAPH_LAYOUT.rankSep),
    nodeSep: asNonNegative(layout.nodeSep, DEFAULT_GRAPH_LAYOUT.nodeSep),
  };
}

/** The two endpoints of every declared edge, as an ordered list of pairs. */
function readPairs(edges: unknown): Array<{ source: string; target: string }> {
  if (!Array.isArray(edges)) return [];
  const pairs: Array<{ source: string; target: string }> = [];
  for (const edge of edges) {
    if (!isPlainObject(edge)) continue;
    const source = asString(edge.source);
    const target = asString(edge.target);
    // A pair with a missing endpoint cannot be ranked. The endpoint check below
    // is what reports it, so the pair is kept whole here on purpose.
    if (source === undefined || target === undefined) continue;
    pairs.push({ source, target });
  }
  return pairs;
}

/**
 * Assign each node a depth: its longest path from any root.
 *
 * Longest-path rather than shortest, because a node that is both a root and
 * three hops downstream of something else must sit past all of them -- a
 * shortest-path rank would draw an edge running backwards through the ranks and
 * React Flow would route it across the whole graph.
 *
 * Relaxation runs to a fixed point with a hard pass bound rather than to
 * convergence. A cycle (a -> b -> a) never reaches a fixed point, and an
 * unbounded loop over author-supplied data is a hung request on the server; with
 * the bound the loop stops after at most one pass per node and the ranks it has
 * settled on are consistent for everything that is not in the cycle.
 */
export function layoutGraph(spec: unknown): Map<string, number> {
  const source = isPlainObject(spec) ? spec : {};
  const nodes = Array.isArray(source.nodes) ? (source.nodes as unknown[]) : [];
  const ranks = new Map<string, number>();
  const order: string[] = [];

  for (const node of nodes) {
    const id = isPlainObject(node) ? asString(node.id) : undefined;
    if (id === undefined || ranks.has(id)) continue;
    ranks.set(id, 0);
    order.push(id);
  }

  const pairs = readPairs(source.edges).filter(
    (pair) => ranks.has(pair.source) && ranks.has(pair.target),
  );

  for (let pass = 0; pass <= order.length; pass++) {
    let changed = false;
    for (const pair of pairs) {
      const next = (ranks.get(pair.source) as number) + 1;
      if (next > (ranks.get(pair.target) as number)) {
        ranks.set(pair.target, next);
        changed = true;
      }
    }
    if (!changed) break;
  }

  return ranks;
}

/**
 * Turn a validated graph spec into ranks and positions.
 *
 * Exported separately from {@link compileGraph} because the ranking is the only
 * part with real logic in it, and it is the part worth testing on its own.
 */
export function positionNodes(
  nodes: GraphNode[],
  edges: Array<{ source: string; target: string }>,
  layout: GraphLayout,
): Map<string, { x: number; y: number }> {
  const ranks = layoutGraph({ nodes, edges });
  const byRank = new Map<number, string[]>();
  for (const node of nodes) {
    const rank = ranks.get(node.id) ?? 0;
    const bucket = byRank.get(rank);
    if (bucket) bucket.push(node.id);
    else byRank.set(rank, [node.id]);
  }

  // Horizontal layouts separate ranks along x; vertical ones along y. Reversed
  // directions negate the rank axis rather than re-sorting, so the geometry is
  // identical and only its sign differs.
  const horizontal = layout.direction === "LR" || layout.direction === "RL";
  const reversed = layout.direction === "RL" || layout.direction === "BT";
  const rankStride = (horizontal ? layout.nodeWidth : layout.nodeHeight) + layout.rankSep;
  const siblingStride = (horizontal ? layout.nodeHeight : layout.nodeWidth) + layout.nodeSep;

  const positions = new Map<string, { x: number; y: number }>();
  for (const [rank, ids] of byRank) {
    ids.forEach((id, index) => {
      const along = (reversed ? -1 : 1) * rank * rankStride;
      const across = index * siblingStride;
      positions.set(id, horizontal ? { x: along, y: across } : { x: across, y: along });
    });
  }
  return positions;
}

/** A finite `{x, y}`, or undefined for anything that would poison a transform. */
function readPosition(value: unknown): { x: number; y: number } | undefined {
  if (!isPlainObject(value)) return undefined;
  const { x, y } = value;
  if (typeof x !== "number" || !Number.isFinite(x)) return undefined;
  if (typeof y !== "number" || !Number.isFinite(y)) return undefined;
  return { x, y };
}

/** The row whose `id` matches, if there is one. Rows need not be objects. */
function rowFor(rows: unknown[], id: string): Dict | undefined {
  for (const row of rows) {
    if (isPlainObject(row) && asString(row.id) === id) return row;
  }
  return undefined;
}

/**
 * A node's visible text.
 *
 * Precedence is: the spec's literal `label`, then `labelField` off the matching
 * row, then the row's own `label`, then the node id. The id is last and is
 * always present, because a node with no text renders as an empty box -- which
 * is indistinguishable from a node that failed to mount.
 */
function readLabel(node: GraphNode, row: Dict | undefined): string {
  const literal = asString(node.label);
  if (literal !== undefined) return literal;
  if (row !== undefined) {
    const field = asString(node.labelField);
    if (field !== undefined) {
      const named = asString(row[field]);
      if (named !== undefined) return named;
    }
    const fromRow = asString(row.label);
    if (fromRow !== undefined) return fromRow;
  }
  return node.id;
}

/**
 * Compile a graph spec plus its rows into React Flow's node and edge arrays.
 *
 * @throws when there is no node to draw at all. That is the one input a graph
 * cannot be rendered from, and it is an authoring error rather than a data
 * condition -- an empty *result* is a valid state and is reported through
 * `diagnostics` instead.
 */
export function compileGraph(
  spec: unknown,
  rows: unknown[],
  options: CompileGraphOptions = {},
): CompiledGraph {
  if (!isPlainObject(spec)) {
    throw new Error("GraphSpec must be an object; a graph needs at least one node.");
  }
  const rawNodes = Array.isArray(spec.nodes) ? spec.nodes : undefined;
  if (rawNodes === undefined || rawNodes.length === 0) {
    throw new Error("GraphSpec needs at least one node; there is nothing to draw.");
  }

  const safeRows = Array.isArray(rows) ? rows : [];
  const layout = readLayout(spec.layout);

  // ── nodes ────────────────────────────────────────────────────────────────
  const diagnostics: GraphDiagnostics = { droppedNodes: [], droppedEdges: [] };
  const seen = new Set<string>();
  const nodes: GraphNode[] = [];

  for (const raw of rawNodes) {
    if (!isPlainObject(raw)) {
      diagnostics.droppedNodes.push("(malformed node)");
      continue;
    }
    const id = asString(raw.id);
    if (id === undefined) {
      diagnostics.droppedNodes.push("(unnamed node)");
      continue;
    }
    // First wins. React Flow keys on `id`; a second node with the same id
    // replaces the first at mount time, so keeping both would mean the graph
    // quietly draws something other than what the spec lists.
    if (seen.has(id)) {
      diagnostics.droppedNodes.push(id);
      continue;
    }
    seen.add(id);
    nodes.push(raw as unknown as GraphNode);
  }

  if (nodes.length === 0) {
    throw new Error("GraphSpec needs at least one node; there is nothing to draw.");
  }

  const pairs = readPairs(spec.edges);
  const positioned = positionNodes(nodes, pairs, layout);

  // ── edges ────────────────────────────────────────────────────────────────
  const edges: FlowEdge[] = [];
  const rawEdges = Array.isArray(spec.edges) ? spec.edges : [];

  rawEdges.forEach((raw, index) => {
    if (!isPlainObject(raw)) return;
    const source = asString(raw.source);
    const target = asString(raw.target);
    // A dangling endpoint is the case React Flow hides: the edge mounts and
    // renders as nothing, so the graph comes up with a connection missing and
    // no error to explain it.
    if (source === undefined || target === undefined || !seen.has(source) || !seen.has(target)) {
      diagnostics.droppedEdges.push(
        asString(raw.id) ?? `${source ?? "?"}->${target ?? "?"}#${index}`,
      );
      return;
    }
    const type = raw.type;
    // Hoisted: `asString` returns `string | undefined`, and a conditional spread
    // over two separate calls is not something TS can narrow -- the second call
    // is a fresh call as far as the checker is concerned.
    const label = asString(raw.label);
    const color = asString(raw.color);
    edges.push({
      // The index is in the id so two parallel edges between the same pair stay
      // distinct; React Flow keys edges by id and would drop the second.
      id: asString(raw.id) ?? `e:${source}->${target}#${index}`,
      source,
      target,
      type:
        type === "straight" ||
        type === "smoothstep" ||
        type === "simplebezier" ||
        type === "default"
          ? type
          : layout.direction === "TB" || layout.direction === "BT"
            ? // A vertical flow reads better with an orthogonal elbow than with
              // a bezier, which bows sideways off a top-to-bottom chain.
              "smoothstep"
            : "default",
      ...(label === undefined ? {} : { label }),
      animated: raw.animated === true,
      ...(color === undefined ? {} : { style: { stroke: color } }),
      markerEnd: { type: "arrowclosed" },
    });
  });

  // ── nodes -> React Flow nodes ────────────────────────────────────────────
  const palette = Array.isArray(options.palette)
    ? options.palette.filter((c): c is string => asString(c) !== undefined)
    : [];
  const compiledNodes: FlowNode[] = nodes.map((node, index) => {
    const row = rowFor(safeRows, node.id);
    const explicit = readPosition(node.position);
    const fallback = positioned.get(node.id) ?? { x: 0, y: 0 };
    const color = asString(node.color) ?? palette[index % palette.length];
    const value = row !== undefined ? row.value : undefined;

    return {
      id: node.id,
      type: "default",
      position: explicit ?? fallback,
      data: {
        label: readLabel(node, row),
        ...(typeof value === "number" && Number.isFinite(value) ? { value } : {}),
      },
      ...(color === undefined ? {} : { style: { background: color } }),
      ...(asString(node.className) === undefined ? {} : { className: asString(node.className) }),
      ...(typeof node.width === "number" && Number.isFinite(node.width) && node.width > 0
        ? { width: node.width }
        : {}),
      ...(typeof node.height === "number" && Number.isFinite(node.height) && node.height > 0
        ? { height: node.height }
        : {}),
    };
  });

  return { nodes: compiledNodes, edges, diagnostics };
}
