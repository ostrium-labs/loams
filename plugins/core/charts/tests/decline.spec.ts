/**
 * `charts` declining a widget it does not own.
 *
 * Before `graph` existed, every widget that reached `RenderService` was assumed
 * to be a chart, and the assumption was enforced by a throw -- `compileNativeWidget`
 * reports `Unknown chart kind: undefined` for a widget carrying no `chart`. For
 * a malformed chart widget that is a useful error. For a graph widget it is a
 * lie: the widget is well-formed, it simply terminates in a React component tree
 * rather than an ECharts option object, and there is no chart kind that could
 * ever express it.
 *
 * So the decline is a RESULT here, carrying the package that does own the widget.
 * The throw stays for what it was actually for: a chart widget whose `kind` is
 * misspelled or unregistered.
 */
import { describe, expect, it, vi } from "vite-plus/test";
import { Context } from "cordis";
import { compileNativeWidget, declineEChartsRender } from "../src/compiler.js";
import { NotAnEChartsWidgetError, RenderService } from "../src/service.js";

const GRAPH_WIDGET = {
  id: "w-graph",
  type: "graph",
  data: { source: "superset", datasetId: 1 },
  graph: { nodes: [{ id: "a" }], edges: [] },
};

function harness(options: { widget?: unknown; rows?: unknown[] } = {}) {
  const ctx = new Context();
  const dataCalls: unknown[] = [];
  ctx.provide("data", {
    async fetchWidgetData(widget: unknown) {
      dataCalls.push(widget);
      return { data: options.rows ?? [{ x: 1, y: 2 }], rowcount: 1 };
    },
  });
  ctx.provide("flint", {} as never);
  return { render: new RenderService(ctx), dataCalls };
}

describe("declineEChartsRender", () => {
  it("declines a graph widget", () => {
    const decline = declineEChartsRender(GRAPH_WIDGET);
    expect(decline).toEqual({
      rendered: false,
      widgetType: "graph",
      reason: expect.stringContaining("flows"),
    });
  });

  it("says plainly that the widget is not an ECharts widget", () => {
    expect(declineEChartsRender(GRAPH_WIDGET)?.reason).toMatch(/not an ECharts widget/i);
  });

  it("declines the other non-chart types too", () => {
    for (const type of ["kpi", "table", "text", "filter"]) {
      expect(declineEChartsRender({ id: "w", type })?.widgetType).toBe(type);
    }
  });

  it("does not decline a chart widget", () => {
    expect(
      declineEChartsRender({ id: "w", type: "chart", chart: { kind: "line" } }),
    ).toBeUndefined();
  });

  it("does not decline an untyped widget, so the existing contract is untouched", () => {
    // Pre-`graph` callers pass `{ chart: { kind } }` with no `type` at all.
    expect(declineEChartsRender({ chart: { kind: "line" } })).toBeUndefined();
  });

  it("does not decline on junk input", () => {
    expect(declineEChartsRender(undefined)).toBeUndefined();
    expect(declineEChartsRender(null)).toBeUndefined();
    expect(declineEChartsRender("chart")).toBeUndefined();
    expect(declineEChartsRender({ type: 7 })).toBeUndefined();
  });
});

describe("RenderService.tryCompileWidget", () => {
  it("declines a graph widget rather than throwing", async () => {
    const { render } = harness();
    const result = await render.tryCompileWidget(GRAPH_WIDGET);
    expect(result).toMatchObject({ rendered: false, widgetType: "graph" });
  });

  it("does not query the data service for a widget it has declined", async () => {
    const { render, dataCalls } = harness();
    await render.tryCompileWidget(GRAPH_WIDGET);
    expect(dataCalls).toHaveLength(0);
  });

  it("still compiles a chart widget", async () => {
    const { render } = harness();
    const result = await render.tryCompileWidget({
      id: "w",
      type: "chart",
      chart: { kind: "bar", encode: { x: "x", y: "y" } },
    });
    expect(result).toMatchObject({ rendered: true });
    expect(result.rendered && result.options.series).toEqual([
      { name: "y", type: "bar", encode: { x: "x", y: "y" } },
    ]);
  });

  it("still throws for a chart widget with an unregistered kind", async () => {
    const { render } = harness();
    await expect(
      render.tryCompileWidget({ id: "w", type: "chart", chart: { kind: "typo" } }),
    ).rejects.toThrow(/Unknown chart kind: typo/);
  });
});

describe("RenderService.compileWidget on a graph widget", () => {
  it("throws the decline error, not 'Unknown chart kind'", async () => {
    const { render } = harness();
    await expect(render.compileWidget(GRAPH_WIDGET)).rejects.toThrow(NotAnEChartsWidgetError);
  });

  it("the decline error names the flow package", async () => {
    const { render } = harness();
    await expect(render.compileWidget(GRAPH_WIDGET)).rejects.toThrow(/@loams-core\/flows/);
  });

  it("the decline error carries the widget type as data", async () => {
    const { render } = harness();
    const err = await render.compileWidget(GRAPH_WIDGET).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(NotAnEChartsWidgetError);
    expect((err as NotAnEChartsWidgetError).widgetType).toBe("graph");
    expect((err as NotAnEChartsWidgetError).reason).toMatch(/not an ECharts widget/i);
  });
});

describe("the pre-existing chart errors are unchanged", () => {
  it("still throws 'Unknown chart kind' for an unregistered kind", () => {
    expect(() => compileNativeWidget({ chart: { kind: "unknown_chart" } }, [])).toThrowError(
      "Unknown chart kind",
    );
  });

  it("still throws 'Chart kind is required' for a chart widget with no chart block", () => {
    expect(() => compileNativeWidget({ type: "chart" }, [])).toThrowError("Chart kind is required");
  });

  it("still throws for a flint widget whose native fallback has no chart", async () => {
    const { render } = harness();
    await expect(
      render.compileWidget({ id: "w", type: "chart", flint: { chartType: "Bar", encodings: {} } }),
    ).rejects.toThrow(/Chart kind is required/);
  });

  it("still compiles a chart widget whose flint compile fails, via the native path", async () => {
    const ctx = new Context();
    ctx.provide("data", {
      async fetchWidgetData() {
        return { data: [{ x: 1, y: 2 }] };
      },
    });
    ctx.provide("flint", {
      compile: vi.fn(async () => {
        throw new Error("flint is down");
      }),
      resolveWidgetTheme: () => ({ valid: true, source: "none" }),
    } as never);
    const warn = vi.spyOn(ctx.logger, "warn");
    const options = await new RenderService(ctx).compileWidget({
      id: "w",
      type: "chart",
      flint: { chartType: "Bar", encodings: {} },
      chart: { kind: "bar", encode: { x: "x", y: "y" } },
    });
    expect(options.series).toBeDefined();
    expect(warn).toHaveBeenCalled();
  });
});
