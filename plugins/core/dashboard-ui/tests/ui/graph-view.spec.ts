/**
 * The pure half of the graph widget: what the server sent, turned into the
 * arrays React Flow mounts, plus the click -> param mapping that drives
 * cross-widget filtering.
 *
 * `toFlowView` is defensive on purpose. Its input is a JSON response from a
 * route that compiles whatever widget it was posted, so it is an untrusted
 * boundary: a missing field, a node with no position, `nodes` arriving as an
 * object instead of an array. React Flow does not fail loudly on those -- it
 * throws deep inside its own renderer, or renders nothing, and either way the
 * tile is a blank box with no message. So every one of those cases is turned
 * into an empty-but-valid view here, where it is testable.
 *
 * `graphParamFilter` mirrors `paramFilterFor` in `chartOption.ts`, which does the
 * same job for an ECharts series click. Same `interactions` shape, same
 * precedence, different event source: there, a clicked series; here, a node id.
 */
import { describe, expect, it } from "vite-plus/test";
import {
  graphParamFilter,
  graphSubtitle,
  graphTitle,
  toFlowView,
} from "../../src/components/graphView.js";
import type { Widget } from "../../src/api";

/** A well-formed server response: two nodes, one edge. */
const PAYLOAD = {
  nodes: [
    { id: "web", position: { x: 0, y: 0 }, data: { label: "Web" } },
    { id: "api", position: { x: 200, y: 0 }, data: { label: "API" } },
  ],
  edges: [{ id: "e1", source: "web", target: "api" }],
};

const widget = (over: Partial<Widget> = {}) =>
  ({
    id: "w-graph",
    type: "graph",
    graph: {
      nodes: [
        { id: "web", label: "Web" },
        { id: "api", label: "API" },
      ],
    },
    ...over,
  }) as unknown as Widget;

describe("toFlowView", () => {
  it("passes nodes and edges through", () => {
    const view = toFlowView(PAYLOAD);
    expect(view.nodes).toHaveLength(2);
    expect(view.edges).toHaveLength(1);
    expect(view.nodes[0].id).toBe("web");
    expect(view.edges[0]).toMatchObject({ source: "web", target: "api" });
  });

  it("reports a graph with content as non-empty", () => {
    expect(toFlowView(PAYLOAD).empty).toBe(false);
  });

  it("reports a graph with no nodes as empty", () => {
    expect(toFlowView({ nodes: [], edges: [] }).empty).toBe(true);
  });

  it("gives a node with no position one, so React Flow can mount it", () => {
    const view = toFlowView({ nodes: [{ id: "a", data: { label: "A" } }], edges: [] });
    expect(Number.isFinite(view.nodes[0].position.x)).toBe(true);
    expect(Number.isFinite(view.nodes[0].position.y)).toBe(true);
  });

  it("gives a node with no label one", () => {
    const view = toFlowView({ nodes: [{ id: "a", position: { x: 0, y: 0 } }], edges: [] });
    expect(view.nodes[0].data.label).toBe("a");
  });

  it("replaces a non-finite position rather than passing NaN through", () => {
    const view = toFlowView({
      nodes: [{ id: "a", position: { x: NaN, y: 0 }, data: {} }],
      edges: [],
    });
    expect(Number.isFinite(view.nodes[0].position.x)).toBe(true);
  });

  it("drops a node with no id, which React Flow cannot key on", () => {
    const view = toFlowView({ nodes: [{ position: { x: 0, y: 0 }, data: {} }], edges: [] });
    expect(view.nodes).toHaveLength(0);
  });

  it("keeps the first of two nodes sharing an id", () => {
    const view = toFlowView({
      nodes: [
        { id: "a", position: { x: 0, y: 0 }, data: { label: "First" } },
        { id: "a", position: { x: 9, y: 9 }, data: { label: "Second" } },
      ],
      edges: [],
    });
    expect(view.nodes).toHaveLength(1);
    expect(view.nodes[0].data.label).toBe("First");
  });

  it("drops an edge naming a node that is not in the graph", () => {
    const view = toFlowView({ ...PAYLOAD, edges: [{ id: "e", source: "web", target: "ghost" }] });
    expect(view.edges).toHaveLength(0);
  });

  it("drops an edge with no id rather than mounting an unkeyed edge", () => {
    const view = toFlowView({ ...PAYLOAD, edges: [{ source: "web", target: "api" }] });
    expect(view.edges).toHaveLength(0);
  });

  it("survives junk payloads without throwing", () => {
    for (const junk of [undefined, null, {}, "nope", 42, [], { nodes: "nope", edges: 7 }]) {
      const view = toFlowView(junk);
      expect(view).toEqual({ nodes: [], edges: [], empty: true });
    }
  });

  it("survives a node that is not an object", () => {
    expect(toFlowView({ nodes: [null, 3, "a"], edges: [] }).nodes).toHaveLength(0);
  });

  it("does not mutate the payload it was given", () => {
    const payload = { nodes: [{ id: "a", data: {} }], edges: [] };
    const snapshot = JSON.stringify(payload);
    toFlowView(payload);
    expect(JSON.stringify(payload)).toBe(snapshot);
  });
});

describe("graphParamFilter", () => {
  it("maps a clicked node onto the interaction's param", () => {
    const w = widget({ interactions: [{ on: "click", set: { service: "label" } }] });
    expect(graphParamFilter(w, { id: "api", data: { label: "API" } })).toEqual({ service: "API" });
  });

  it("emits nothing when the widget declares no click interaction", () => {
    expect(graphParamFilter(widget(), { id: "api", data: { label: "API" } })).toEqual({});
  });

  it("ignores a brush interaction", () => {
    const w = widget({ interactions: [{ on: "brush", set: { service: "label" } }] });
    expect(graphParamFilter(w, { id: "api", data: { label: "API" } })).toEqual({});
  });

  it("prefers the named field on the node's data", () => {
    const w = widget({ interactions: [{ on: "click", set: { region: "region" } }] });
    const filter = graphParamFilter(w, { id: "api", data: { label: "API", region: "eu-west" } });
    expect(filter).toEqual({ region: "eu-west" });
  });

  it("falls back to the node label when the named field is absent", () => {
    const w = widget({ interactions: [{ on: "click", set: { region: "region" } }] });
    expect(graphParamFilter(w, { id: "api", data: { label: "API" } })).toEqual({ region: "API" });
  });

  it("falls back to the node id when there is no data either", () => {
    const w = widget({ interactions: [{ on: "click", set: { region: "region" } }] });
    expect(graphParamFilter(w, { id: "api" })).toEqual({ region: "api" });
  });

  it("omits a param whose value is empty, rather than filtering on nothing", () => {
    const w = widget({ interactions: [{ on: "click", set: { region: "region" } }] });
    expect(graphParamFilter(w, { id: "", data: {} })).toEqual({});
  });

  it("omits a param whose value is null", () => {
    const w = widget({ interactions: [{ on: "click", set: { region: "region" } }] });
    expect(graphParamFilter(w, { id: "a", data: { region: null } })).toEqual({});
  });

  it("carries a numeric value through unstringified", () => {
    const w = widget({ interactions: [{ on: "click", set: { shard: "shard" } }] });
    expect(graphParamFilter(w, { id: "a", data: { shard: 0 } })).toEqual({ shard: 0 });
  });

  it("does not throw on a node with no data and no id", () => {
    const w = widget({ interactions: [{ on: "click", set: { region: "region" } }] });
    expect(graphParamFilter(w, {})).toEqual({});
  });

  it("emits every declared interaction param", () => {
    const w = widget({
      interactions: [
        { on: "click", set: { a: "label" } },
        { on: "click", set: { b: "label" } },
      ],
    });
    expect(Object.keys(graphParamFilter(w, { id: "x", data: { label: "L" } }))).toEqual(["a", "b"]);
  });
});

describe("graph titles", () => {
  it("uses an explicit title when the widget carries one", () => {
    expect(
      graphTitle(widget({ graph: { title: "Service Map", nodes: [{ id: "a" }] } } as never)),
    ).toBe("Service Map");
  });

  it("falls back to a graph-specific default rather than the chart default", () => {
    // `WidgetCard` falls back to "CHART Chart" / "Chart Widget"; a graph tile
    // that said "Chart Widget" would be telling the reader something untrue.
    expect(graphTitle(widget())).toBe("Graph Widget");
  });

  it("shows the dataset as the subtitle, like the chart card does", () => {
    const w = widget({ data: { source: "superset", datasetId: 7 } });
    expect(graphSubtitle(w)).toBe("Dataset #7");
  });

  it("has no subtitle when there is no dataset", () => {
    expect(graphSubtitle(widget())).toBe("");
  });
});
