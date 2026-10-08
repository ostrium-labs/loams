/**
 * `FlowRenderService`: data in, themed `{ nodes, edges }` out.
 *
 * The shape mirrors `charts`'s `RenderService` exactly, and for
 * the same reason. Both are cordis services declaring `static inject = ["data",
 * "flint"]`, both fetch through `ctx.data.fetchWidgetData`, and both resolve
 * their theme through `ctx.flint.resolveWidgetTheme` rather than reading
 * `widget.flint.theme_spec` or `dashboardSpec.theme` themselves -- that
 * precedence is the documented contract in `dashboard-spec.ts`, and
 * re-implementing it here would be a second, silently divergent copy.
 *
 * What differs is the terminal value. ECharts terminates in an options object;
 * this terminates in a node/edge pair, which is why `graph` is a sibling widget
 * type and not a chart kind.
 *
 * The theme assertions here are about DEGRADATION, not about flint's colour
 * choices. A theme that cannot be resolved must leave a renderable graph, which
 * is the property that a screenshot cannot show.
 */
import { describe, expect, it, vi } from "vite-plus/test";
import { Context } from "cordis";
import { DASHBOARD_THEME_PARAM } from "@loams-core/charts";
import { FlowRenderService, NotAGraphWidgetError } from "../src/service.js";
import { DEFAULT_FLOW_THEME } from "../src/theme.js";

const ROWS = [
  { id: "web", name: "Web tier" },
  { id: "api", name: "API tier" },
];

/** A resolution+grounding the stub hands back, shaped like flint's real one. */
const GROUNDED = {
  valid: true,
  decisions: {
    themeId: "house",
    surface: { canvas: "#ffffff", panel: "#f5f5f5" },
    text: { primary: "#111111", secondary: "#555555", muted: "#777777", inverse: "#ffffff" },
    frame: { show: true, color: "#cccccc", width: 1 },
    series: { single: "#00aa88", categorical: ["#00aa88", "#aa00aa"] },
  },
};

interface HarnessOptions {
  rows?: unknown[];
  fetch?: (widget: unknown, params?: Record<string, unknown>) => unknown;
  flint?: unknown;
}

/**
 * A context with stubbed `data` and `flint`.
 *
 * `flint` defaults to a bridge that resolves and grounds successfully. Pass
 * `flint: null` for "no flint service at all" -- `undefined` cannot express
 * that here, because it is the default-bridge sentinel.
 */
function harness({ rows = ROWS, fetch, flint }: HarnessOptions = {}) {
  const ctx = new Context();
  const resolveCalls: { widget: unknown; dashboardTheme: unknown }[] = [];
  const dataCalls: (Record<string, unknown> | undefined)[] = [];

  ctx.provide("data", {
    async fetchWidgetData(widget: unknown, params?: Record<string, unknown>) {
      dataCalls.push(params);
      return fetch ? fetch(widget, params) : { data: rows, rowcount: rows.length };
    },
  });

  if (flint === null) {
    // What "chart-specs is not installed" looks like on a cordis context.
    ctx.provide("flint", null as never);
  } else {
    const bridge =
      flint === undefined
        ? {
            resolveWidgetTheme(widget: unknown, dashboardTheme: unknown) {
              resolveCalls.push({ widget, dashboardTheme });
              return { valid: true, source: "preset", spec: { preset: "house" } };
            },
            groundTheme() {
              return GROUNDED;
            },
          }
        : flint;
    ctx.provide("flint", bridge as never);
  }
  return { flow: new FlowRenderService(ctx), resolveCalls, dataCalls, ctx };
}

const GRAPH_WIDGET = {
  id: "w-graph",
  type: "graph",
  data: { source: "superset", datasetId: 1 },
  graph: {
    // `labelField`, not an auto-detected column: guessing which field holds a
    // node's label is magic, and the spec names it explicitly instead.
    nodes: [
      { id: "web", labelField: "name" },
      { id: "api", labelField: "name" },
    ],
    edges: [{ source: "web", target: "api" }],
  },
};

describe("FlowRenderService.compileGraphWidget", () => {
  it("returns nodes and edges for a graph widget", async () => {
    const { flow } = harness();
    const result = await flow.compileGraphWidget(GRAPH_WIDGET);
    expect(result.nodes.map((n) => n.id)).toEqual(["web", "api"]);
    expect(result.edges).toHaveLength(1);
  });

  it("labels nodes from the fetched rows", async () => {
    const { flow } = harness();
    const result = await flow.compileGraphWidget(GRAPH_WIDGET);
    expect(result.nodes[0].data.label).toBe("Web tier");
  });

  it("tolerates a data service that returns a bare array", async () => {
    const { flow } = harness({ fetch: () => ROWS });
    const result = await flow.compileGraphWidget(GRAPH_WIDGET);
    expect(result.nodes[0].data.label).toBe("Web tier");
  });

  it("tolerates a data service that returns nothing useful", async () => {
    for (const payload of [undefined, null, {}, { data: "nope" }]) {
      const { flow } = harness({ fetch: () => payload });
      const result = await flow.compileGraphWidget(GRAPH_WIDGET);
      expect(result.nodes).toHaveLength(2);
      expect(result.nodes[0].data.label).toBe("web");
    }
  });

  it("passes the widget's data params through to the query", async () => {
    const { flow, dataCalls } = harness();
    await flow.compileGraphWidget(GRAPH_WIDGET, { region: "North" });
    expect(dataCalls[0]).toEqual({ region: "North" });
  });

  it("strips the reserved theme param before the query", async () => {
    // Every params entry becomes a SQL filter column, so a theme left in there
    // would query a column literally named `__dashboardTheme`.
    const { flow, dataCalls } = harness();
    await flow.compileGraphWidget(GRAPH_WIDGET, {
      region: "North",
      [DASHBOARD_THEME_PARAM]: "house",
    });
    expect(dataCalls[0]).toEqual({ region: "North" });
  });

  it("resolves the theme through ctx.flint, handing it the dashboard theme", async () => {
    const { flow, resolveCalls } = harness();
    await flow.compileGraphWidget(GRAPH_WIDGET, undefined, { preset: "house" });
    expect(resolveCalls).toHaveLength(1);
    expect(resolveCalls[0].widget).toBe(GRAPH_WIDGET);
    expect(resolveCalls[0].dashboardTheme).toEqual({ preset: "house" });
  });

  it("reads the reserved theme param when no explicit argument is given", async () => {
    const { flow, resolveCalls } = harness();
    await flow.compileGraphWidget(GRAPH_WIDGET, { [DASHBOARD_THEME_PARAM]: "from-params" });
    expect(resolveCalls[0].dashboardTheme).toBe("from-params");
  });

  it("lets an explicit theme outrank the reserved key", async () => {
    const { flow, resolveCalls } = harness();
    await flow.compileGraphWidget(
      GRAPH_WIDGET,
      { [DASHBOARD_THEME_PARAM]: "from-params" },
      "explicit",
    );
    expect(resolveCalls[0].dashboardTheme).toBe("explicit");
  });

  it("applies the grounded ink to the compiled graph", async () => {
    const { flow } = harness();
    const result = await flow.compileGraphWidget(GRAPH_WIDGET);
    expect(result.theme.nodeBackground).toBe("#f5f5f5");
    expect(result.theme.nodeText).toBe("#111111");
    expect(result.theme.nodeBorder).toBe("#cccccc");
    expect(result.theme.edgeStroke).toBe("#cccccc");
    expect(result.theme.palette).toEqual(["#00aa88", "#aa00aa"]);
  });

  it("paints the nodes from the theme's palette", async () => {
    const { flow } = harness();
    const result = await flow.compileGraphWidget(GRAPH_WIDGET);
    expect(result.nodes[0].style?.background).toBe("#00aa88");
    expect(result.nodes[1].style?.background).toBe("#aa00aa");
  });

  it("carries the interaction flags through", async () => {
    const { flow } = harness();
    const widget = {
      ...GRAPH_WIDGET,
      graph: { ...GRAPH_WIDGET.graph, fitView: false, pannable: false, zoomable: false },
    };
    const result = await flow.compileGraphWidget(widget);
    expect(result).toMatchObject({ fitView: false, pannable: false, zoomable: false });
  });

  it("defaults the interaction flags when the spec omits them", async () => {
    const { flow } = harness();
    const result = await flow.compileGraphWidget(GRAPH_WIDGET);
    expect(result).toMatchObject({ fitView: true, pannable: true, zoomable: true });
  });
});

describe("FlowRenderService theme degradation", () => {
  /** Every one of these must still yield a renderable graph. */
  const broken = {
    "no flint service at all": null,
    "resolveWidgetTheme throws": {
      resolveWidgetTheme() {
        throw new Error("boom");
      },
    },
    "resolveWidgetTheme reports invalid": {
      resolveWidgetTheme: () => ({
        valid: false,
        report: [{ path: "preset", message: "unknown" }],
      }),
    },
    "resolveWidgetTheme returns nothing": { resolveWidgetTheme: () => undefined },
    "groundTheme throws": {
      resolveWidgetTheme: () => ({ valid: true, source: "preset", spec: { preset: "house" } }),
      groundTheme() {
        throw new Error("boom");
      },
    },
    "grounding reports invalid": {
      resolveWidgetTheme: () => ({ valid: true, source: "preset", spec: { preset: "house" } }),
      groundTheme: () => ({ valid: false, report: [{ path: "chartType", message: "unknown" }] }),
    },
    "grounding returns no decisions": {
      resolveWidgetTheme: () => ({ valid: true, source: "preset", spec: { preset: "house" } }),
      groundTheme: () => ({ valid: true }),
    },
    "a bridge with neither method": {},
  };

  for (const [label, flint] of Object.entries(broken)) {
    it(`renders with the default theme when ${label}`, async () => {
      const { flow } = harness({ flint });
      const result = await flow.compileGraphWidget(GRAPH_WIDGET);
      expect(result.nodes).toHaveLength(2);
      expect(result.theme).toEqual(DEFAULT_FLOW_THEME);
    });
  }

  it("treats source 'none' as a real answer, not a failure", async () => {
    // The dashboard and the widget both named no house. That is a renderable
    // state, and it must not be reported as a broken theme.
    const { flow, ctx } = harness({
      flint: { resolveWidgetTheme: () => ({ valid: true, source: "none" }) },
    });
    const warn = vi.spyOn(ctx.logger, "warn");
    const result = await flow.compileGraphWidget(GRAPH_WIDGET);
    expect(result.theme).toEqual(DEFAULT_FLOW_THEME);
    expect(warn).not.toHaveBeenCalled();
  });

  it("logs a theme failure once rather than once per render", async () => {
    const { flow, ctx } = harness({ flint: { resolveWidgetTheme: () => undefined } });
    const warn = vi.spyOn(ctx.logger, "warn");
    await flow.compileGraphWidget(GRAPH_WIDGET);
    await flow.compileGraphWidget(GRAPH_WIDGET);
    await flow.compileGraphWidget(GRAPH_WIDGET);
    expect(warn).toHaveBeenCalledTimes(1);
  });

  it("falls back to the default theme when the grounded decisions are junk", async () => {
    const { flow } = harness({
      flint: {
        resolveWidgetTheme: () => ({ valid: true, source: "preset", spec: { preset: "house" } }),
        groundTheme: () => ({ valid: true, decisions: { surface: "not-an-object" } }),
      },
    });
    const result = await flow.compileGraphWidget(GRAPH_WIDGET);
    expect(result.theme).toEqual(DEFAULT_FLOW_THEME);
    expect(result.nodes).toHaveLength(2);
  });

  it("keeps whatever ink the decisions do state, and defaults the rest", async () => {
    const { flow } = harness({
      flint: {
        resolveWidgetTheme: () => ({ valid: true, source: "preset", spec: { preset: "house" } }),
        groundTheme: () => ({ valid: true, decisions: { text: { primary: "#010203" } } }),
      },
    });
    const result = await flow.compileGraphWidget(GRAPH_WIDGET);
    expect(result.theme.nodeText).toBe("#010203");
    expect(result.theme.nodeBackground).toBe(DEFAULT_FLOW_THEME.nodeBackground);
  });
});

describe("FlowRenderService widget typing", () => {
  it("refuses a widget that is not a graph", async () => {
    const { flow } = harness();
    await expect(
      flow.compileGraphWidget({ id: "w", type: "chart", chart: { kind: "line" } }),
    ).rejects.toThrow(NotAGraphWidgetError);
  });

  it("names the flow package in the refusal, so a misroute is legible", async () => {
    const { flow } = harness();
    await expect(flow.compileGraphWidget({ id: "w", type: "chart" })).rejects.toThrow(
      /@loams-core\/flows/,
    );
  });

  it("refuses a graph widget with no graph block", async () => {
    const { flow } = harness();
    await expect(flow.compileGraphWidget({ id: "w", type: "graph" })).rejects.toThrow(
      NotAGraphWidgetError,
    );
  });

  it("tryCompileGraphWidget reports the refusal instead of throwing", async () => {
    const { flow } = harness();
    const result = await flow.tryCompileGraphWidget({ id: "w", type: "chart" });
    expect(result).toEqual({
      ok: false,
      reason: expect.stringContaining("flows"),
    });
  });

  it("tryCompileGraphWidget does not query the data service when it declines", async () => {
    const { flow, dataCalls } = harness();
    await flow.tryCompileGraphWidget({ id: "w", type: "chart" });
    expect(dataCalls).toHaveLength(0);
  });

  it("tryCompileGraphWidget returns the graph when it accepts", async () => {
    const { flow } = harness();
    const result = await flow.tryCompileGraphWidget(GRAPH_WIDGET);
    expect(result.ok).toBe(true);
    expect(result.ok && result.graph.nodes).toHaveLength(2);
  });

  it("accepts a widget with no `type` but a graph block", async () => {
    // The type union is validated upstream; the service asks one question --
    // is there a graph to draw? -- and answers it from the payload.
    const { flow } = harness();
    const result = await flow.compileGraphWidget({ id: "w", graph: GRAPH_WIDGET.graph });
    expect(result.nodes).toHaveLength(2);
  });
});
