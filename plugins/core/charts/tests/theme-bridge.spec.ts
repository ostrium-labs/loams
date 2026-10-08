import { describe, it, expect, vi } from "vite-plus/test";
import { Context } from "cordis";
import { FlintService } from "@loams-core/chart-specs";
import { DASHBOARD_THEME_PARAM, RenderService, splitRenderParams } from "../src/service.js";

/**
 * End-to-end: a widget rendered through `RenderService.compileWidget` with the
 * real `FlintService` behind it, so the whole chain is exercised - resolve the
 * selection, ground it against this chart, map the grounded ink onto option keys.
 *
 * These assertions are the ones that matter to a reader of a dashboard: a themed
 * widget and an unthemed widget of the same chart type must look different, and a
 * theme that cannot be resolved must change nothing at all.
 */

const ROWS = [
  { region: "North", value: 12 },
  { region: "South", value: 30 },
];

function barWidget(themeSpec?: unknown) {
  return {
    id: "w1",
    type: "chart",
    flint: {
      chartType: "Bar Chart",
      encodings: { x: { field: "region" }, y: { field: "value" } },
      ...(themeSpec === undefined ? {} : { theme_spec: themeSpec }),
    },
  };
}

/**
 * Structural comparison.
 *
 * `FlintService.compile` mints a fresh `tooltip.formatter` closure on every
 * call, so two compiles of the same widget are deeply equal but not reference
 * equal, and `toEqual` would fail on a difference no reader of the chart can see.
 */
const snapshot = (value: unknown): unknown => JSON.parse(JSON.stringify(value));

function harness() {
  const ctx = new Context();
  ctx.provide("controlPlane", {});
  // `new FlintService(ctx)` registers itself on the context it is given, which
  // is why this harness does not `provide` it again.
  const flint = new FlintService(ctx);

  const dataCalls: (Record<string, unknown> | undefined)[] = [];
  ctx.provide("data", {
    async fetchWidgetData(_widget: unknown, params?: Record<string, unknown>) {
      dataCalls.push(params);
      return { data: ROWS, rowcount: ROWS.length };
    },
  });

  const warn = vi.spyOn(ctx.logger, "warn");
  return { render: new RenderService(ctx), flint, warn, dataCalls };
}

describe("compileWidget — a theme actually changes the option", () => {
  it("gives a themed widget a different palette, axes, labels and tooltip", async () => {
    const { render } = harness();

    const unthemed = await render.compileWidget(barWidget());
    const themed = (await render.compileWidget(barWidget(), undefined, { preset: "swiss" })) as any;

    // Palette: the house's single ink for a one-series chart, replacing the
    // hardcoded Power BI fallback `FlintService.compile` leaves behind.
    expect(themed.color).toEqual(["#e2231a"]);
    expect(unthemed.color).not.toEqual(themed.color);

    // Axis rule, split line and label ink, all from the same grounded theme.
    expect(themed.xAxis.axisLine).toEqual({
      show: true,
      lineStyle: { color: "#1a1a1a", width: 1 },
    });
    expect(themed.yAxis.axisLine).toEqual({
      show: true,
      lineStyle: { color: "#1a1a1a", width: 1 },
    });
    expect(themed.yAxis.splitLine).toEqual({
      show: true,
      lineStyle: { color: "#e1ddd4", width: 1 },
    });
    expect(themed.xAxis.splitLine).toEqual({ show: false });
    expect(themed.xAxis.axisLabel.color).toBe("#555555");
    expect(themed.yAxis.axisLabel.color).toBe("#555555");
    expect(themed.xAxis.nameTextStyle.color).toBe("#1a1a1a");

    // Tooltip chrome and the canvas behind the plot.
    expect(themed.tooltip.backgroundColor).toBe("#f4f1ea");
    expect(themed.tooltip.textStyle.color).toBe("#1a1a1a");
    expect(themed.backgroundColor).toBe("#f4f1ea");

    // And none of that is present on the unthemed chart.
    expect((unthemed as any).xAxis.axisLabel.color).toBeUndefined();
    expect((unthemed as any).yAxis.splitLine).toBeUndefined();
    expect((unthemed as any).tooltip.backgroundColor).toBeUndefined();
    expect((unthemed as any).backgroundColor).toBeUndefined();
  });

  it("keeps the compiled data, axes and series structure intact", async () => {
    const { render } = harness();
    const option = (await render.compileWidget(barWidget(), undefined, { preset: "swiss" })) as any;
    expect(option.xAxis.data).toEqual(["North", "South"]);
    expect(option.series[0].type).toBe("bar");
    expect(option.series[0].data).toEqual([12, 30]);
  });

  it("hands a native chart kind the palette too, one ink per slice", async () => {
    const { render } = harness();
    // A native (non-flint) widget: the theme path must not be flint-only.
    const widget = {
      id: "w2",
      type: "chart",
      chart: { kind: "pie", encode: { x: "region", value: "value" } },
    };
    const themed = (await render.compileWidget(widget, undefined, { preset: "swiss" })) as any;
    // Two rows, so the pie needs two inks and takes the categorical set.
    expect(themed.color).toEqual(["#e2231a", "#1a1a1a", "#0067a5", "#f2b705", "#2a7f4f"]);
    expect(themed.legend.textStyle.color).toBe("#555555");
  });

  it("themes a copy: the options the compiler returned are not touched", async () => {
    const { render, flint } = harness();
    const compile = flint.compile.bind(flint);
    let compiled: Record<string, unknown> | undefined;
    vi.spyOn(flint, "compile").mockImplementation(async (widget, data) => {
      compiled = await compile(widget as any, data as any);
      return compiled;
    });

    await render.compileWidget(barWidget(), undefined, { preset: "swiss" });
    expect(compiled).toBeDefined();
    expect((compiled as any).xAxis.axisLabel.color).toBeUndefined();
    expect((compiled as any).tooltip.backgroundColor).toBeUndefined();
  });
});

describe("compileWidget — precedence and fallbacks", () => {
  it("lets a per-widget theme_spec beat the dashboard theme", async () => {
    const { render } = harness();
    const option = (await render.compileWidget(barWidget("cartoon"), undefined, {
      preset: "swiss",
    })) as any;
    expect(option.color).toEqual(["#3aa9ff"]);
  });

  it("accepts a bare preset name as the dashboard theme", async () => {
    const { render } = harness();
    const themed = (await render.compileWidget(barWidget(), undefined, "swiss")) as any;
    expect(themed.color).toEqual(["#e2231a"]);

    const alsoThemed = (await render.compileWidget(barWidget(), undefined, {
      preset: "swiss",
    })) as any;
    expect(alsoThemed.color).toEqual(themed.color);
  });

  it("layers a custom spec on the dashboard house", async () => {
    const { render } = harness();
    const themed = (await render.compileWidget(barWidget(), undefined, {
      preset: "swiss",
      custom: { ink: { series: { single: "#ff00ff" } } },
    })) as any;
    expect(themed.color).toEqual(["#ff00ff"]);
    // Swiss' own ink still shows through where the custom spec is silent.
    expect(themed.yAxis.splitLine.lineStyle.color).toBe("#e1ddd4");
  });

  it("renders exactly as it does today when nothing names a theme", async () => {
    const { render, flint } = harness();
    const rendered = await render.compileWidget(barWidget());
    expect(snapshot(rendered)).toEqual(
      snapshot(await flint.compile(barWidget() as any, { data: ROWS } as any)),
    );
  });

  it("treats source 'none' as a real answer, not a failure", async () => {
    const { render, flint, warn } = harness();
    const rendered = await render.compileWidget(barWidget(), undefined, {});
    expect(snapshot(rendered)).toEqual(
      snapshot(await flint.compile(barWidget() as any, { data: ROWS } as any)),
    );
    expect(warn).not.toHaveBeenCalled();
  });
});

describe("compileWidget — an unresolvable theme", () => {
  it("does not throw on an unknown preset id, and leaves the chart unthemed", async () => {
    const { render } = harness();
    await expect(
      render.compileWidget(barWidget(), undefined, { preset: "nope-not-real" }),
    ).resolves.toBeDefined();

    const unthemed = await render.compileWidget(barWidget());
    const broken = await render.compileWidget(barWidget(), undefined, { preset: "nope-not-real" });
    expect(snapshot(broken)).toEqual(snapshot(unthemed));
  });

  it("warns once, naming the theme and the report, not once a render", async () => {
    const { render, warn } = harness();
    for (let i = 0; i < 3; i += 1) {
      await render.compileWidget(barWidget(), undefined, { preset: "nope-not-real" });
    }

    // Only this package's own line is deduplicated: `FlintService.resolveTheme`
    // logs the same failure on every call, which is upstream's behaviour.
    const mine = warn.mock.calls
      .map((call) => call.join(" "))
      .filter((line) => line.includes("could not be resolved"));
    expect(mine).toHaveLength(1);
    expect(mine[0]).toContain("nope-not-real");
    expect(mine[0]).toContain("theme.preset");
    expect(mine[0]).toContain("w1");
  });

  it("does not throw when the flint service cannot resolve anything at all", async () => {
    const { render, warn } = harness();
    (render as any).ctx.flint = undefined;
    // A native widget, so this exercises the degraded path without depending on
    // the flint assembler being able to compile anything.
    const widget = {
      id: "w3",
      type: "chart",
      chart: { kind: "bar", encode: { x: "region", y: "value" } },
    } as any;
    const option = await render.compileWidget(widget, undefined, { preset: "swiss" });
    expect(option.color).toBeUndefined();
    expect(option.xAxis).toEqual({ type: "category" });
    expect(warn.mock.calls.map((c) => c.join(" ")).join("\n")).toContain("groundTheme()");
  });
});

describe("the dashboard theme as an explicit argument or a reserved param", () => {
  it("reaches the renderer through the third argument", async () => {
    const { render } = harness();
    const themed = (await render.compileWidget(barWidget(), undefined, { preset: "swiss" })) as any;
    expect(themed.color).toEqual(["#e2231a"]);
  });

  it("also reaches it through the reserved params key", async () => {
    const { render } = harness();
    const themed = (await render.compileWidget(barWidget(), {
      time_range: "7d",
      [DASHBOARD_THEME_PARAM]: { preset: "swiss" },
    })) as any;
    expect(themed.color).toEqual(["#e2231a"]);
  });

  // Every entry in `params` becomes a filter column in `queryData`, so the
  // reserved key must not survive into the data call - or the chart would be
  // queried on a column named after it.
  it("strips the reserved key before the data query, keeping real params", async () => {
    const { render, dataCalls } = harness();
    await render.compileWidget(barWidget(), {
      time_range: "7d",
      [DASHBOARD_THEME_PARAM]: { preset: "swiss" },
    });
    expect(dataCalls[0]).toEqual({ time_range: "7d" });
  });

  it("splits the reserved key out of a params bag, and leaves a clean one alone", () => {
    expect(splitRenderParams({ [DASHBOARD_THEME_PARAM]: "swiss", a: 1 })).toEqual({
      dataParams: { a: 1 },
      dashboardTheme: "swiss",
    });
    const clean = { a: 1 };
    expect(splitRenderParams(clean).dataParams).toBe(clean);
    expect(splitRenderParams()).toEqual({});
  });

  it("prefers the explicit argument over the reserved key", async () => {
    const { render } = harness();
    const option = (await render.compileWidget(
      barWidget(),
      { [DASHBOARD_THEME_PARAM]: "swiss" },
      "cartoon",
    )) as any;
    expect(option.color).toEqual(["#3aa9ff"]);
  });
});

describe("previewWidget", () => {
  it("returns the themed options with the widget that produced them", async () => {
    const { render } = harness();
    const widget = barWidget();
    const preview = await render.previewWidget(widget, undefined, { preset: "swiss" });
    expect(preview.widget).toBe(widget);
    expect((preview.options as any).color).toEqual(["#e2231a"]);
  });
});
