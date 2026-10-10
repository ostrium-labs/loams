/**
 * The pure half of the graph widget.
 *
 * `toFlowView` is the boundary between "whatever the preview route sent" and
 * "the arrays React Flow mounts". It is defensive because its input is
 * untrusted: a JSON response from a route that compiles whatever widget it was
 * posted. A missing field, a node with no position, `nodes` arriving as an
 * object instead of an array -- React Flow does not fail loudly on those. It
 * throws deep inside its own renderer, or renders nothing, and either way the
 * tile is a blank box with no message. Turning those into an empty-but-valid
 * view here means the component has one failure mode to report, and it is a
 * testable one.
 *
 * This split follows the precedent set by `chartOption.ts`: in a workspace where
 * the interesting part of a card is a rule that markup cannot show, the rule is
 * extracted into a pure function and asserted on directly.
 *
 * `graphParamFilter` is the mirror of `paramFilterFor` in `chartOption.ts` --
 * same `interactions` shape, same precedence, different event source. There, a
 * clicked series; here, a clicked node.
 */

type Dict = Record<string, unknown>;

/** The node shape `GraphWidget` mounts. Only the fields React Flow requires. */
export interface FlowNode {
  id: string;
  position: { x: number; y: number };
  data: Dict;
  style?: Dict;
  className?: string;
}

export interface FlowEdge {
  id: string;
  source: string;
  target: string;
  label?: string;
  animated?: boolean;
  style?: Dict;
}

export interface FlowView {
  nodes: FlowNode[];
  edges: FlowEdge[];
  /** True when there is nothing to draw, which the tile states rather than showing a blank canvas. */
  empty: boolean;
}

function isPlainObject(value: unknown): value is Dict {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

/** A finite coordinate, or 0. React Flow feeds these straight into a transform. */
function asCoordinate(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

/**
 * Repair a compiled payload into arrays React Flow can mount.
 *
 * Never throws. Every rejection is a drop, and a dropped element is one fewer
 * box on screen rather than a blank tile -- except for the two cases React Flow
 * cannot survive at all: a node with no `id`, and an edge naming a node that is
 * not in the graph. Both are dropped here, with no diagnostic channel, because
 * the server already logs them (`compileGraph`'s `diagnostics`) and this runs
 * against the server's own output.
 */
export function toFlowView(payload: unknown): FlowView {
  const empty: FlowView = { nodes: [], edges: [], empty: true };
  if (!isPlainObject(payload)) return empty;

  const rawNodes = Array.isArray(payload.nodes) ? payload.nodes : [];
  const seen = new Set<string>();
  const nodes: FlowNode[] = [];

  for (const raw of rawNodes) {
    if (!isPlainObject(raw)) continue;
    const id = asString(raw.id);
    // No id means no React key, and a duplicate would silently replace the
    // first. Both are unrenderable, so both are dropped rather than renamed:
    // an invented id would show the author a graph they did not author.
    if (id === undefined || seen.has(id)) continue;
    seen.add(id);

    const data = isPlainObject(raw.data) ? raw.data : {};
    nodes.push({
      id,
      position: isPlainObject(raw.position)
        ? { x: asCoordinate(raw.position.x), y: asCoordinate(raw.position.y) }
        : { x: 0, y: 0 },
      // A node with no label renders as an empty box, which looks identical to
      // a node that failed to mount. The id is always something to show.
      data: { ...data, label: asString(data.label) ?? id },
      ...(isPlainObject(raw.style) ? { style: raw.style } : {}),
      ...(asString(raw.className) === undefined ? {} : { className: asString(raw.className) }),
    });
  }

  const rawEdges = Array.isArray(payload.edges) ? payload.edges : [];
  const edges: FlowEdge[] = [];
  for (const raw of rawEdges) {
    if (!isPlainObject(raw)) continue;
    const id = asString(raw.id);
    const source = asString(raw.source);
    const target = asString(raw.target);
    if (id === undefined || source === undefined || target === undefined) continue;
    if (!seen.has(source) || !seen.has(target)) continue;
    edges.push({
      id,
      source,
      target,
      ...(asString(raw.label) === undefined ? {} : { label: asString(raw.label) }),
      ...(raw.animated === true ? { animated: true } : {}),
      ...(isPlainObject(raw.style) ? { style: raw.style } : {}),
    });
  }

  return { nodes, edges, empty: nodes.length === 0 };
}

/** A clicked node, as React Flow reports it. */
export interface ClickedNode {
  id?: unknown;
  data?: unknown;
}

/**
 * The params a clicked node sets, for cross-widget filtering.
 *
 * Precedence per interaction entry: the field the interaction names, then the
 * node's label, then its id. The fallbacks apply only when the field is ABSENT,
 * never when it is present and empty -- a node whose `region` is explicitly
 * `null` has said "no region", and substituting the label or the id there would
 * filter the dashboard on something the reader never chose.
 *
 * An absent or empty value is OMITTED rather than written as `undefined` or
 * `""`, because every params entry becomes a SQL filter column -- filtering on
 * an empty string is not "no filter", it is a query that matches nothing. A value
 * of `0` IS written, since it is a real filter value and not an absence.
 */
export function graphParamFilter(widget: unknown, node: ClickedNode): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  if (!isPlainObject(widget) || !Array.isArray(widget.interactions)) return out;
  const data = isPlainObject(node?.data) ? node.data : {};
  const id = asString(node?.id);

  for (const interaction of widget.interactions) {
    if (!isPlainObject(interaction) || interaction.on !== "click") continue;
    if (!isPlainObject(interaction.set)) continue;
    for (const [paramName, fieldName] of Object.entries(interaction.set)) {
      if (typeof fieldName !== "string") continue;
      // `in`, not `??`: presence and emptiness are different facts.
      const value = fieldName in data ? data[fieldName] : (data.label ?? id);
      if (value === undefined || value === null || value === "") continue;
      out[paramName] = value;
    }
  }
  return out;
}

/**
 * The tile's title.
 *
 * `WidgetCard` falls back to `"CHART Chart"` or `"Chart Widget"`. A graph tile
 * saying "Chart Widget" would be telling the reader something untrue, so this
 * has its own default.
 */
export function graphTitle(widget: unknown): string {
  const explicit =
    isPlainObject(widget) && isPlainObject(widget.graph) ? asString(widget.graph.title) : undefined;
  return explicit ?? "Graph Widget";
}

/** The dataset line under the title, matching what the chart card shows. */
export function graphSubtitle(widget: unknown): string {
  if (!isPlainObject(widget)) return "";
  const data = isPlainObject(widget.data) ? widget.data : undefined;
  const datasetId = data?.datasetId;
  return typeof datasetId === "number" ? `Dataset #${datasetId}` : "";
}
