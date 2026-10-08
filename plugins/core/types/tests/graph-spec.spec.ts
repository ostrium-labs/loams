/**
 * The `graph` widget type, and the guards around it.
 *
 * Two things are being held here, and they are the reason `graph` is a sibling
 * `Widget.type` rather than a new `ChartSchema.kind`:
 *
 * 1. The chart rule must survive. "Exactly one of `flint` or `chart`" is stated
 *    for `type === "chart"` and nowhere else, so adding a type cannot have
 *    quietly widened or narrowed it. The chart cases below are the regression
 *    floor for that.
 * 2. A graph widget has to be recognisably a graph widget. It carries a `graph`
 *    block, and it must NOT also claim `flint` or `chart`: those are the two
 *    inputs to the ECharts pipeline, and a widget that fed both would be routed
 *    to two renderers that both believe they own it. The terminal-type contract
 *    is enforced at parse time, where the mistake is still cheap.
 *
 * The structural rules -- at least one node, unique ids, no dangling endpoints
 * -- live on `GraphSpecSchema` rather than on the widget, so the flow compiler
 * can validate the same thing whether it is handed a parsed spec or a raw body.
 */
import { describe, expect, it } from "vite-plus/test";
import { DashboardSpecSchema, GraphSpecSchema, WidgetSchema } from "../src/dashboard-spec.js";

/** A minimal well-formed graph widget, as an author would write it. */
const GRAPH_WIDGET = {
  id: "w-graph",
  type: "graph",
  data: { source: "superset", datasetId: 3 },
  graph: {
    nodes: [
      { id: "web", label: "Web" },
      { id: "api", label: "API" },
      { id: "db", label: "Database" },
    ],
    edges: [
      { source: "web", target: "api" },
      { source: "api", target: "db" },
    ],
  },
};

/** The issues of a failed parse, flattened for a readable assertion. */
function issuesOf(result: {
  success: boolean;
  error?: { issues: { path: (string | number)[]; message: string }[] };
}) {
  return (result.error?.issues ?? []).map((i) => `${i.path.join(".")}: ${i.message}`);
}

describe("graph widget type", () => {
  it("parses a graph widget", () => {
    const parsed = WidgetSchema.parse(GRAPH_WIDGET);
    expect(parsed.type).toBe("graph");
    expect(parsed.graph?.nodes).toHaveLength(3);
    expect(parsed.graph?.edges).toHaveLength(2);
  });

  it("accepts 'graph' as a widget type", () => {
    // The enum is closed; this is the assertion that it was widened at all.
    const parsed = WidgetSchema.parse({ id: "w", type: "graph", graph: { nodes: [{ id: "a" }] } });
    expect(parsed.type).toBe("graph");
  });

  it("fills the graph defaults so a consumer never branches on absence", () => {
    const parsed = WidgetSchema.parse({ id: "w", type: "graph", graph: { nodes: [{ id: "a" }] } });
    expect(parsed.graph?.layout).toEqual({
      direction: "LR",
      nodeWidth: 180,
      nodeHeight: 48,
      rankSep: 80,
      nodeSep: 32,
    });
    expect(parsed.graph?.edges).toEqual([]);
    expect(parsed.graph?.fitView).toBe(true);
    expect(parsed.graph?.pannable).toBe(true);
    expect(parsed.graph?.zoomable).toBe(true);
  });

  it("keeps an explicit layout and an explicit edge list", () => {
    const parsed = WidgetSchema.parse({
      id: "w",
      type: "graph",
      graph: {
        nodes: [{ id: "a" }, { id: "b" }],
        edges: [{ id: "e1", source: "a", target: "b", label: "reads", animated: true }],
        layout: { direction: "TB", nodeWidth: 120, nodeHeight: 40, rankSep: 10, nodeSep: 10 },
      },
    });
    expect(parsed.graph?.layout.direction).toBe("TB");
    expect(parsed.graph?.edges[0]).toMatchObject({ id: "e1", label: "reads", animated: true });
  });

  it("requires a graph block on a graph widget", () => {
    const result = WidgetSchema.safeParse({ id: "w", type: "graph" });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain("Graph widgets must define 'graph'.");
  });

  it("rejects a graph widget that also declares chart", () => {
    const result = WidgetSchema.safeParse({
      ...GRAPH_WIDGET,
      chart: { kind: "line", encode: { x: "a", y: "b" } },
    });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain("must not declare 'flint' or 'chart'");
  });

  it("rejects a graph widget that also declares flint", () => {
    const result = WidgetSchema.safeParse({
      ...GRAPH_WIDGET,
      flint: { chartType: "Bar Chart", encodings: {} },
    });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain("must not declare 'flint' or 'chart'");
  });
});

describe("the chart rule survives the new type", () => {
  it("still rejects a chart widget with neither flint nor chart", () => {
    const result = WidgetSchema.safeParse({ id: "w", type: "chart" });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain(
      "Chart widgets must have exactly one of 'flint' or 'chart' defined.",
    );
  });

  it("still rejects a chart widget with both flint and chart", () => {
    const result = WidgetSchema.safeParse({
      id: "w",
      type: "chart",
      flint: { chartType: "Bar Chart", encodings: {} },
      chart: { kind: "line", encode: {} },
    });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain(
      "Chart widgets must have exactly one of 'flint' or 'chart' defined.",
    );
  });

  it("still accepts a chart widget with exactly one of them", () => {
    expect(
      WidgetSchema.safeParse({ id: "w", type: "chart", chart: { kind: "bar", encode: {} } })
        .success,
    ).toBe(true);
    expect(
      WidgetSchema.safeParse({ id: "w", type: "chart", flint: { chartType: "Pie", encodings: {} } })
        .success,
    ).toBe(true);
  });

  it("does not constrain a non-chart widget that has no chart block", () => {
    // A text/kpi/table widget carries neither `flint` nor `chart`, and must
    // keep parsing. This is the case a careless "every widget needs exactly
    // one" rewrite would have broken.
    for (const type of ["kpi", "table", "text", "filter"] as const) {
      expect(WidgetSchema.safeParse({ id: "w", type }).success).toBe(true);
    }
  });
});

describe("GraphSpecSchema structural rules", () => {
  it("parses a bare spec with only nodes", () => {
    const parsed = GraphSpecSchema.parse({ nodes: [{ id: "a" }] });
    expect(parsed.nodes).toHaveLength(1);
    expect(parsed.edges).toEqual([]);
  });

  it("rejects a spec with neither nodes nor derive", () => {
    const result = GraphSpecSchema.safeParse({ edges: [] });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain("at least one node");
  });

  it("accepts a spec that derives its nodes from the data", () => {
    // The live-stream case: the ids are not known when the spec is written.
    const result = GraphSpecSchema.safeParse({ derive: { idField: "id" } });
    expect(result.success).toBe(true);
  });

  it("rejects derive alongside a literal node list", () => {
    // Two ways of saying what the nodes are; the compiler cannot honour both.
    const result = GraphSpecSchema.safeParse({
      nodes: [{ id: "a" }],
      derive: { idField: "id" },
    });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain("derive");
  });

  it("rejects a derive naming only one end of an edge", () => {
    const result = GraphSpecSchema.safeParse({
      derive: { idField: "id", sourceField: "parent_id" },
    });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain("come as a pair");
  });

  it("cannot check edge endpoints when the node set is derived", () => {
    // The ids are unknown until the rows arrive, so the endpoint rule is the
    // compiler's to enforce at render time, not zod's to enforce here.
    const result = GraphSpecSchema.safeParse({
      derive: { idField: "id", sourceField: "parent_id", targetField: "id" },
      edges: [{ source: "ghost", target: "also-ghost" }],
    });
    expect(result.success).toBe(true);
  });

  it("rejects an empty node list", () => {
    const result = GraphSpecSchema.safeParse({ nodes: [] });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain("at least one node");
  });

  it("rejects a node with an empty id", () => {
    const result = GraphSpecSchema.safeParse({ nodes: [{ id: "" }] });
    expect(result.success).toBe(false);
  });

  it("rejects duplicate node ids", () => {
    const result = GraphSpecSchema.safeParse({
      nodes: [{ id: "a" }, { id: "b" }, { id: "a" }],
    });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain("Duplicate node id: a");
  });

  it("rejects an edge whose source names no node", () => {
    const result = GraphSpecSchema.safeParse({
      nodes: [{ id: "a" }],
      edges: [{ source: "ghost", target: "a" }],
    });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain("Edge source references unknown node: ghost");
  });

  it("rejects an edge whose target names no node", () => {
    const result = GraphSpecSchema.safeParse({
      nodes: [{ id: "a" }],
      edges: [{ source: "a", target: "ghost" }],
    });
    expect(result.success).toBe(false);
    expect(issuesOf(result).join(" ")).toContain("Edge target references unknown node: ghost");
  });

  it("allows a self-loop, which is a real graph and not a typo", () => {
    const result = GraphSpecSchema.safeParse({
      nodes: [{ id: "a" }],
      edges: [{ source: "a", target: "a" }],
    });
    expect(result.success).toBe(true);
  });

  it("rejects a non-finite position", () => {
    const result = GraphSpecSchema.safeParse({ nodes: [{ id: "a", position: { x: NaN, y: 0 } }] });
    expect(result.success).toBe(false);
  });

  it("rejects an unknown layout direction", () => {
    const result = GraphSpecSchema.safeParse({
      nodes: [{ id: "a" }],
      layout: { direction: "sideways" },
    });
    expect(result.success).toBe(false);
  });

  it("rejects a non-positive node size", () => {
    const result = GraphSpecSchema.safeParse({ nodes: [{ id: "a" }], layout: { nodeWidth: 0 } });
    expect(result.success).toBe(false);
  });

  it("survives a full dashboard spec carrying a graph widget", () => {
    const parsed = DashboardSpecSchema.parse({
      id: "d1",
      version: 0,
      title: "Topology",
      params: [],
      layout: [{ id: "w-graph", x: 0, y: 0, w: 12, h: 6 }],
      widgets: { "w-graph": GRAPH_WIDGET },
    });
    expect(parsed.widgets["w-graph"].type).toBe("graph");
  });
});
