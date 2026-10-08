/**
 * GraphSpec + rows -> the `{ nodes, edges }` pair React Flow actually mounts.
 *
 * This is the reason `graph` is a sibling `Widget.type` and not a new
 * `ChartSchema.kind`: React Flow's input is a node/edge pair it renders through
 * its own React tree, so the compiler's terminal value here is that pair rather
 * than an ECharts option object. `charts` cannot produce it and
 * declines the widget instead.
 *
 * The compiler is pure and defensive. Every input below is a boundary -- a
 * hand-written dashboard spec, or a raw JSON body posted straight to a preview
 * route that never went through zod -- so the cases here are the ones where a
 * naive map() produces a graph React Flow cannot mount: a dangling endpoint, a
 * duplicate id, a cycle, a non-finite coordinate.
 */
import { describe, expect, it } from "vite-plus/test";
import { compileGraph, layoutGraph } from "../src/compiler.js";

/** web -> api -> db: the canonical three-node chain. */
const CHAIN = {
  nodes: [{ id: "web" }, { id: "api" }, { id: "db" }],
  edges: [
    { source: "web", target: "api" },
    { source: "api", target: "db" },
  ],
};

describe("compileGraph", () => {
  it("returns a nodes/edges pair, never an options blob", () => {
    const graph = compileGraph(CHAIN, []);
    expect(Array.isArray(graph.nodes)).toBe(true);
    expect(Array.isArray(graph.edges)).toBe(true);
    // The ECharts contract has no meaning here, so nothing of that shape leaks.
    expect(graph).not.toHaveProperty("series");
    expect(graph).not.toHaveProperty("dataset");
  });

  it("gives every node a position and a label", () => {
    const { nodes } = compileGraph(CHAIN, []);
    expect(nodes).toHaveLength(3);
    for (const node of nodes) {
      expect(node.position).toEqual({ x: expect.any(Number), y: expect.any(Number) });
      expect(typeof node.data.label).toBe("string");
      expect(node.data.label.length).toBeGreaterThan(0);
    }
    expect(Number.isFinite(nodes[0].position.x)).toBe(true);
    expect(Number.isFinite(nodes[0].position.y)).toBe(true);
  });

  it("preserves node ids", () => {
    const { nodes } = compileGraph(CHAIN, []);
    expect(nodes.map((n) => n.id)).toEqual(["web", "api", "db"]);
  });

  it("lays a chain out left to right by default, one rank per depth", () => {
    const { nodes } = compileGraph(CHAIN, []);
    const x = Object.fromEntries(nodes.map((n) => [n.id, n.position.x]));
    expect(x.web).toBe(0);
    expect(x.api).toBeGreaterThan(x.web);
    expect(x.db).toBeGreaterThan(x.api);
  });

  it("puts sibling ranks on the same axis so they do not stack", () => {
    // web -> api, web -> cache: `api` and `cache` are both at depth 1.
    const { nodes } = compileGraph(
      {
        nodes: [{ id: "web" }, { id: "api" }, { id: "cache" }],
        edges: [
          { source: "web", target: "api" },
          { source: "web", target: "cache" },
        ],
      },
      [],
    );
    const api = nodes.find((n) => n.id === "api")!;
    const cache = nodes.find((n) => n.id === "cache")!;
    expect(api.position.x).toBe(cache.position.x);
    expect(api.position.y).not.toBe(cache.position.y);
  });

  it("lays top to bottom when the direction says so", () => {
    const { nodes } = compileGraph({ ...CHAIN, layout: { direction: "TB" } }, []);
    const y = Object.fromEntries(nodes.map((n) => [n.id, n.position.y]));
    expect(y.web).toBe(0);
    expect(y.api).toBeGreaterThan(y.web);
    expect(y.db).toBeGreaterThan(y.api);
  });

  it("reverses the flow for bottom-to-top and right-to-left", () => {
    const bt = compileGraph({ ...CHAIN, layout: { direction: "BT" } }, []).nodes;
    const tb = compileGraph({ ...CHAIN, layout: { direction: "TB" } }, []).nodes;
    // `db`, not `web`: rank 0 is the origin in every direction, so only a node
    // that actually moves along the rank axis can show the reversal.
    const dbY = (list: typeof bt) => list.find((n) => n.id === "db")!.position.y;
    expect(dbY(bt)).toBeLessThan(dbY(tb));

    const rl = compileGraph({ ...CHAIN, layout: { direction: "RL" } }, []).nodes;
    const lr = compileGraph({ ...CHAIN, layout: { direction: "LR" } }, []).nodes;
    const dbX = (list: typeof rl) => list.find((n) => n.id === "db")!.position.x;
    expect(dbX(rl)).toBeLessThan(dbX(lr));
  });

  it("gives every node a distinct position", () => {
    const { nodes } = compileGraph(
      {
        nodes: [{ id: "a" }, { id: "b" }, { id: "c" }, { id: "d" }],
        edges: [
          { source: "a", target: "b" },
          { source: "a", target: "c" },
          { source: "a", target: "d" },
        ],
      },
      [],
    );
    const keys = nodes.map((n) => `${n.position.x}:${n.position.y}`);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("honours an explicit position over the computed one", () => {
    const { nodes } = compileGraph(
      {
        nodes: [{ id: "a", position: { x: 42, y: 7 } }, { id: "b" }],
        edges: [{ source: "a", target: "b" }],
      },
      [],
    );
    expect(nodes.find((n) => n.id === "a")!.position).toEqual({ x: 42, y: 7 });
  });

  it("takes the label from an explicit node label first", () => {
    const { nodes } = compileGraph({ nodes: [{ id: "a", label: "Alpha" }] }, []);
    expect(nodes[0].data.label).toBe("Alpha");
  });

  it("takes the label from a matching row when the node has none", () => {
    const { nodes } = compileGraph({ nodes: [{ id: "a", labelField: "name" }] }, [
      { id: "a", name: "From Row" },
      { id: "b", name: "Other" },
    ]);
    expect(nodes.find((n) => n.id === "a")!.data.label).toBe("From Row");
  });

  it("falls back to the node id when there is no label and no row", () => {
    const { nodes } = compileGraph({ nodes: [{ id: "lonely" }] }, []);
    expect(nodes[0].data.label).toBe("lonely");
  });

  it("falls back to the node id when the label field is missing from the row", () => {
    const { nodes } = compileGraph({ nodes: [{ id: "a", labelField: "name" }] }, [{ id: "a" }]);
    expect(nodes[0].data.label).toBe("a");
  });

  it("survives a row that is not an object", () => {
    expect(() => compileGraph({ nodes: [{ id: "a" }] }, [null, 7, "x"] as unknown[])).not.toThrow();
    expect(compileGraph({ nodes: [{ id: "a" }] }, [null, 7] as unknown[]).nodes[0].data.label).toBe(
      "a",
    );
  });

  it("survives no rows at all", () => {
    expect(() => compileGraph(CHAIN, [])).not.toThrow();
    expect(compileGraph(CHAIN, []).nodes).toHaveLength(3);
  });

  it("emits one edge per declared edge", () => {
    const { edges } = compileGraph(CHAIN, []);
    expect(edges).toHaveLength(2);
    expect(edges[0]).toMatchObject({ source: "web", target: "api" });
    expect(edges[1]).toMatchObject({ source: "api", target: "db" });
  });

  it("gives every edge a unique id", () => {
    // Two edges between the same pair is a parallel edge, not a duplicate.
    const { edges } = compileGraph(
      {
        nodes: [{ id: "a" }, { id: "b" }],
        edges: [
          { source: "a", target: "b" },
          { source: "a", target: "b" },
        ],
      },
      [],
    );
    const ids = edges.map((e) => e.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("keeps an authored edge id", () => {
    const { edges } = compileGraph(
      { nodes: [{ id: "a" }, { id: "b" }], edges: [{ id: "e7", source: "a", target: "b" }] },
      [],
    );
    expect(edges[0].id).toBe("e7");
  });

  it("keeps edge label, animated and colour", () => {
    const { edges } = compileGraph(
      {
        nodes: [{ id: "a" }, { id: "b" }],
        edges: [{ source: "a", target: "b", label: "reads", animated: true, color: "#abc" }],
      },
      [],
    );
    expect(edges[0].label).toBe("reads");
    expect(edges[0].animated).toBe(true);
    expect(edges[0].style?.stroke).toBe("#abc");
  });

  it("does not animate an edge unless asked", () => {
    const { edges } = compileGraph(CHAIN, []);
    expect(edges[0].animated).toBe(false);
  });

  it("emits a self-loop rather than dropping it", () => {
    const { edges } = compileGraph(
      { nodes: [{ id: "a" }], edges: [{ source: "a", target: "a" }] },
      [],
    );
    expect(edges).toHaveLength(1);
    expect(edges[0]).toMatchObject({ source: "a", target: "a" });
  });

  it("drops a dangling edge instead of emitting an edge React Flow cannot mount", () => {
    const graph = compileGraph(
      {
        nodes: [{ id: "a" }],
        edges: [
          { source: "a", target: "ghost" },
          { source: "a", target: "a" },
        ],
      } as never,
      [],
    );
    expect(graph.edges).toHaveLength(1);
    // The diagnostic names both endpoints and the edge's index, not just the
    // dangling one: an author looking at three edges needs to know which.
    expect(graph.diagnostics.droppedEdges).toEqual(["a->ghost#0"]);
  });

  it("drops a duplicate node id, keeping the first", () => {
    const graph = compileGraph(
      {
        nodes: [
          { id: "a", label: "First" },
          { id: "a", label: "Second" },
        ],
      } as never,
      [],
    );
    expect(graph.nodes).toHaveLength(1);
    expect(graph.nodes[0].data.label).toBe("First");
    expect(graph.diagnostics.droppedNodes).toEqual(["a"]);
  });

  it("keeps a node no edge mentions", () => {
    const { nodes } = compileGraph({ nodes: [{ id: "orphan" }] }, []);
    expect(nodes.map((n) => n.id)).toEqual(["orphan"]);
  });

  it("lays a cycle out rather than looping forever", () => {
    const graph = compileGraph(
      {
        nodes: [{ id: "a" }, { id: "b" }, { id: "c" }],
        edges: [
          { source: "a", target: "b" },
          { source: "b", target: "c" },
          { source: "c", target: "a" },
        ],
      },
      [],
    );
    expect(graph.nodes).toHaveLength(3);
    expect(graph.edges).toHaveLength(3);
    const keys = graph.nodes.map((n) => `${n.position.x}:${n.position.y}`);
    expect(new Set(keys).size).toBe(3);
  });

  it("lays a self-referential graph out rather than looping forever", () => {
    expect(() =>
      compileGraph({ nodes: [{ id: "a" }], edges: [{ source: "a", target: "a" }] }, []),
    ).not.toThrow();
  });

  it("paints nodes from the palette by index and lets an explicit colour win", () => {
    const { nodes } = compileGraph(
      { nodes: [{ id: "a" }, { id: "b" }, { id: "c", color: "#000000" }] },
      [],
      { palette: ["#111111", "#222222"] },
    );
    const colour = Object.fromEntries(nodes.map((n) => [n.id, n.style?.background]));
    expect(colour.a).toBe("#111111");
    expect(colour.b).toBe("#222222");
    expect(colour.c).toBe("#000000");
  });

  it("leaves node colour unset when there is no palette and no explicit colour", () => {
    const { nodes } = compileGraph({ nodes: [{ id: "a" }] }, []);
    expect(nodes[0].style?.background).toBeUndefined();
  });

  it("rejects a non-finite position and lays the node out instead", () => {
    const graph = compileGraph(
      { nodes: [{ id: "a", position: { x: NaN, y: Infinity } }] } as never,
      [],
    );
    expect(Number.isFinite(graph.nodes[0].position.x)).toBe(true);
    expect(Number.isFinite(graph.nodes[0].position.y)).toBe(true);
  });

  it("compiles a graph with no edges at all", () => {
    const graph = compileGraph({ nodes: [{ id: "a" }, { id: "b" }] }, []);
    expect(graph.nodes).toHaveLength(2);
    expect(graph.edges).toEqual([]);
  });

  it("throws only on a spec with no nodes to draw", () => {
    expect(() => compileGraph({ nodes: [] }, [])).toThrow(/at least one node/i);
    expect(() => compileGraph(undefined, [])).toThrow();
    expect(() => compileGraph({ nodes: null }, [])).toThrow();
  });
});

describe("layoutGraph", () => {
  it("assigns each node a rank equal to its longest path from a root", () => {
    // a -> c and b -> c -> d: `d` must sit one rank past `c`, not level with it.
    const ranks = layoutGraph({
      nodes: [{ id: "a" }, { id: "b" }, { id: "c" }, { id: "d" }],
      edges: [
        { source: "a", target: "c" },
        { source: "b", target: "c" },
        { source: "c", target: "d" },
      ],
    });
    expect(ranks.get("a")).toBe(0);
    expect(ranks.get("b")).toBe(0);
    expect(ranks.get("c")).toBe(1);
    expect(ranks.get("d")).toBe(2);
  });

  it("ranks a node with no incoming edge at zero", () => {
    expect(layoutGraph({ nodes: [{ id: "a" }, { id: "b" }], edges: [] }).get("a")).toBe(0);
  });
});

describe("a graph derived from the rows", () => {
  // The live-stream case: the ids are not known when the spec is written, so the
  // spec names columns and the compiler reads them off each row.
  const SPEC = {
    derive: {
      idField: "id",
      labelField: "kind",
      sourceField: "parent_id",
      targetField: "id",
      colorField: "state",
    },
    layout: { direction: "LR" },
  };

  const ROWS = [
    { id: "op-1", kind: "collection.import", parent_id: "", state: "RUNNING" },
    { id: "op-2", kind: "connector.sync", parent_id: "op-1", state: "DONE" },
    { id: "op-3", kind: "webhook.fire", parent_id: "op-1", state: "RUNNING" },
  ];

  it("makes one node per row, keyed on the named id column", () => {
    const { nodes } = compileGraph(SPEC, ROWS);
    expect(nodes.map((n) => n.id).sort()).toEqual(["op-1", "op-2", "op-3"]);
  });

  it("labels each node from the label column", () => {
    const { nodes } = compileGraph(SPEC, ROWS);
    const byId = new Map(nodes.map((n) => [n.id, n.data.label]));
    expect(byId.get("op-1")).toBe("collection.import");
    expect(byId.get("op-2")).toBe("connector.sync");
  });

  it("derives an edge per row that names both ends", () => {
    const { edges } = compileGraph(SPEC, ROWS);
    // op-1 has no parent, so it contributes no edge; the other two do.
    expect(edges.map((e) => `${e.source}->${e.target}`).sort()).toEqual([
      "op-1->op-2",
      "op-1->op-3",
    ]);
  });

  it("drops an edge whose source names no node in the graph", () => {
    // The row claims a parent the rows never delivered. React Flow would mount
    // that edge and render nothing for it, so it is dropped and reported.
    const { edges, diagnostics } = compileGraph(SPEC, [
      ...ROWS,
      { id: "op-4", kind: "orphan", parent_id: "op-999", state: "RUNNING" },
    ]);
    expect(edges.map((e) => e.target)).not.toContain("op-4");
    expect(diagnostics.droppedEdges.length).toBe(1);
  });

  it("keeps one colour per distinct state as the stream churns", () => {
    // Same states, different arrival order: the fill must not shift with index,
    // or every new operation would repaint the whole graph.
    const forwards = compileGraph(SPEC, ROWS).nodes;
    const backwards = compileGraph(SPEC, [...ROWS].reverse()).nodes;
    const fill = (nodes: typeof forwards, id: string) =>
      nodes.find((n) => n.id === id)?.style?.background;
    expect(fill(forwards, "op-1")).toBe(fill(backwards, "op-1"));
    expect(fill(forwards, "op-2")).toBe(fill(backwards, "op-2"));
  });

  it("ignores a row with no id rather than inventing one", () => {
    const { nodes } = compileGraph(SPEC, [{ kind: "nameless" }, ...ROWS]);
    expect(nodes).toHaveLength(3);
  });

  it("ignores a duplicate id, keeping the first", () => {
    const { nodes } = compileGraph(SPEC, [...ROWS, { id: "op-1", kind: "again" }]);
    expect(nodes.filter((n) => n.id === "op-1")).toHaveLength(1);
  });

  it("gives a stream with nothing in it yet an empty graph, not a throw", () => {
    // Unlike a hand-authored spec, an empty row set here is a data condition:
    // nothing has arrived yet.
    const graph = compileGraph(SPEC, []);
    expect(graph.nodes).toEqual([]);
    expect(graph.edges).toEqual([]);
  });

  it("refuses a derive with no idField", () => {
    expect(() => compileGraph({ derive: { labelField: "kind" } }, ROWS)).toThrow(/idField/);
  });

  it("draws nodes but no edges when only one end is named", () => {
    const { nodes, edges } = compileGraph(
      { derive: { idField: "id", sourceField: "parent_id" } },
      ROWS,
    );
    expect(nodes).toHaveLength(3);
    expect(edges).toEqual([]);
  });
});
