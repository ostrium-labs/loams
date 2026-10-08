/**
 * `WidgetCard`'s theme gate -- the one thing the Tailwind migration must not
 * have touched.
 *
 * `WidgetCard` stands down a set of hardcoded Power BI greys when the server
 * has already compiled a theme for the widget. `hasTheme` is the signal for
 * that, and it is load-bearing in a way markup cannot show: `hasTheme=true` has
 * to leave the server's colours alone, and `hasTheme=false` has to overlay them.
 *
 * A conversion from CSS classes to utilities touches only the JSX, and a
 * regression here would be invisible in a screenshot -- the chart still renders,
 * just in the wrong ink. So this asserts on the option object itself, which is
 * why the rule was extracted into `composeChartOption`: this workspace has no
 * DOM (jsdom/happy-dom are not installed) and `renderToStaticMarkup` does not
 * run effects, so the only way to reach the option is to make it a pure
 * function. `WidgetCard.spec.ts` covers the markup side of the same card.
 */
import { describe, expect, it } from "vite-plus/test";
import { readFileSync } from "node:fs";
import {
  POWER_BI_SERIES,
  composeChartOption,
  paramFilterFor,
} from "../../src/components/chartOption.js";
import type { Widget } from "../../src/api";

/** A server option that is already themed, deliberately not neutral grey. */
const SERVER_THEMED = {
  xAxis: {
    type: "category",
    data: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul"],
    axisLine: { lineStyle: { color: "#ff00aa" } },
    axisTick: { show: false },
    axisLabel: { color: "#ff00aa" },
  },
  yAxis: {
    type: "value",
    axisLine: { show: false },
    axisTick: { show: false },
    axisLabel: { color: "#ff00aa" },
    splitLine: { lineStyle: { type: "solid", color: "#00ff00" } },
  },
  tooltip: { backgroundColor: "#0000ff", borderColor: "#0000ff" },
  legend: { textStyle: { color: "#ffff00" }, orient: "vertical" },
  series: [{ type: "bar", data: [1, 2, 3, 4, 5, 6, 7] }],
};

const compose = (base: Record<string, unknown>, hasTheme: boolean) =>
  composeChartOption(structuredClone(base), hasTheme) as Record<string, any>;

describe("the hasTheme gate: the server's colours survive", () => {
  const option = compose(SERVER_THEMED, true);

  it("leaves the x axis line and label exactly as they arrived", () => {
    expect(option.xAxis.axisLine).toEqual({ lineStyle: { color: "#ff00aa" } });
    expect(option.xAxis.axisLabel).toMatchObject({ color: "#ff00aa" });
  });

  it("leaves the split lines alone", () => {
    expect(option.yAxis.splitLine).toEqual({ lineStyle: { type: "solid", color: "#00ff00" } });
    // The x axis had no splitLine, and the tile SUPPRESSES one (`show: false`)
    // rather than inventing ink for it. Suppression is not theming: there is
    // no colour here for a theme to have been overwritten by.
    expect(option.xAxis.splitLine).toEqual({ show: false });
  });

  it("leaves the tooltip alone, including the server's own trigger", () => {
    // `trigger` is still the tile's decision; only the COLOURS are the server's.
    expect(option.tooltip).toEqual({
      trigger: "axis",
      backgroundColor: "#0000ff",
      borderColor: "#0000ff",
    });
  });

  it("leaves the legend's text colour alone while still placing the legend", () => {
    expect(option.legend.textStyle.color).toBe("#ffff00");
    expect(option.legend.orient).toBe("vertical");
    // Placement is chrome, and chrome is the tile's to apply either way.
    expect(option.legend).toMatchObject({ left: 16, top: "middle", icon: "circle" });
  });

  it("applies none of the Power BI greys anywhere in the option", () => {
    const serialised = JSON.stringify(option, (key, value) =>
      typeof value === "function" ? "<fn>" : value,
    );
    for (const colour of ["#c8c6c4", "#605e5c", "#d2d0ce", "#ededed", "#ffffff"]) {
      expect(serialised).not.toContain(`"${colour}"`);
    }
  });

  it("does not apply the Power BI palette when the server picked its own", () => {
    expect(option.color).toBeUndefined();
  });
});

describe("the hasTheme gate: the tile owns the ink when nothing else does", () => {
  const option = compose(SERVER_THEMED, false);

  it("stamps the Power BI greys onto both axes", () => {
    expect(option.xAxis.axisLine).toEqual({ lineStyle: { color: "#d2d0ce" } });
    expect(option.xAxis.axisLabel).toMatchObject({ color: "#605e5c" });
    expect(option.yAxis.splitLine).toEqual({ lineStyle: { type: "solid", color: "#ededed" } });
  });

  it("gives the tooltip and legend their own ink", () => {
    // The tile's tooltip and legend colours are DEFAULTS, not overrides: the
    // server's own object is spread last, so a tooltip or legend it sent keeps
    // its colours. `SERVER_THEMED` sent both, so both blues/yellows survive --
    // which is the behaviour that stops a theme being un-themed by the tile.
    expect(option.tooltip).toMatchObject({
      backgroundColor: "#0000ff",
      borderColor: "#0000ff",
      // ...while the tile still owns the parts the server never speaks for.
      trigger: "axis",
      textStyle: { fontSize: 12, fontFamily: "'Segoe UI', sans-serif" },
      extraCssText: "box-shadow: 0 4px 12px rgba(0, 0, 0, 0.12); border-radius: 4px;",
    });
    expect(option.legend.textStyle).toMatchObject({
      // The server's `textStyle` object replaces the tile's wholesale, colour
      // and metrics together. That is why a theme cannot be half-applied by the
      // tile: it either speaks for the legend's text or it says nothing.
      color: "#ffff00",
    });
  });

  it("paints the tooltip and legend itself when the server sent neither", () => {
    const bare = compose(
      {
        xAxis: { type: "category", data: [] },
        yAxis: { type: "value" },
        series: [{ type: "bar" }],
      },
      false,
    );
    expect(bare.tooltip).toMatchObject({
      backgroundColor: "#ffffff",
      borderColor: "#e1dfdd",
      borderWidth: 1,
      textStyle: { color: "#252423" },
    });
    // No legend on the option at all, so the tile does not invent one -- but
    // the axis labels still get the Power BI grey.
    expect(bare.legend).toBeUndefined();
    expect(bare.xAxis.axisLabel.color).toBe("#605e5c");
    expect(bare.yAxis.axisLabel.color).toBe("#605e5c");
  });

  it("still applies the chrome: fonts, rotation, grid padding, hidden title", () => {
    // Both branches must agree on everything that is not colour, or the "themed"
    // chart would quietly lose its Segoe UI and its hairline layout.
    expect(option.xAxis.axisLabel.fontFamily).toBe("'Segoe UI', wf_segoe-ui_normal, sans-serif");
    expect(option.xAxis.axisLabel.fontSize).toBe(11);
    expect(option.xAxis.axisLabel.interval).toBe(0);
    // Seven categories: past the six the card rotates the labels.
    expect(option.xAxis.axisLabel.rotate).toBe(25);
    expect(option.title).toEqual({ show: false });
    expect(option.grid).toEqual({ top: 20, bottom: 25, left: 20, right: 20, containLabel: true });
    expect(option.yAxis.axisLine).toEqual({ show: false });
    expect(option.xAxis.axisTick).toEqual({ show: false });
  });
});

describe("the gate is the only difference", () => {
  const themed = compose(SERVER_THEMED, true);
  const unthemed = compose(SERVER_THEMED, false);

  const WITHOUT_COLOUR = new Set([
    "axisLine",
    "axisTick",
    "axisLabel",
    "splitLine",
    "tooltip",
    "legend",
    "color",
  ]);

  /** Strip the colour-bearing keys so only structural differences remain. */
  const skeleton = (value: unknown, key?: string): unknown => {
    if (key && WITHOUT_COLOUR.has(key)) return "<colour>";
    if (Array.isArray(value)) return value.map((v) => skeleton(v));
    if (value && typeof value === "object") {
      return Object.fromEntries(
        Object.entries(value as Record<string, unknown>).map(([k, v]) => [k, skeleton(v, k)]),
      );
    }
    return value;
  };

  it("leaves series, title and grid identical", () => {
    expect(themed.series).toEqual(unthemed.series);
    expect(themed.title).toEqual(unthemed.title);
    expect(themed.grid).toEqual(unthemed.grid);
    expect(themed.dataset).toEqual(unthemed.dataset);
  });

  it("differs only in the colour-bearing keys the gate names", () => {
    const differing = [...new Set([...Object.keys(themed), ...Object.keys(unthemed)])].filter(
      (key) =>
        JSON.stringify(skeleton(themed[key], key)) !== JSON.stringify(skeleton(unthemed[key], key)),
    );
    expect(differing.sort()).toEqual([]);
  });
});

describe("the categorical palette", () => {
  it("replaces an empty server palette", () => {
    expect(compose({ series: [] }, false).color).toEqual([...POWER_BI_SERIES]);
  });

  it("replaces either of the two defaults chart-specs and ECharts ship", () => {
    expect(compose({ color: ["#0284c7"], series: [] }, false).color).toEqual([...POWER_BI_SERIES]);
    expect(compose({ color: ["#5470c6"], series: [] }, false).color).toEqual([...POWER_BI_SERIES]);
  });

  it("keeps a palette the server chose deliberately", () => {
    const themedPalette = ["#123456", "#654321"];
    expect(compose({ color: themedPalette, series: [] }, false).color).toEqual(themedPalette);
  });

  it("keeps it under a theme too -- hasTheme is the only thing that matters", () => {
    expect(compose({ color: ["#0284c7"], series: [] }, true).color).toEqual(["#0284c7"]);
  });
});

describe("input hardening", () => {
  it("flattens a wrapped dataset.source", () => {
    const option = compose({ dataset: { source: { data: [{ a: 1 }] } } }, true) as any;
    expect(option.dataset.source).toEqual([{ a: 1 }]);
  });

  it("leaves an already-flat dataset.source alone", () => {
    const option = compose({ dataset: { source: [{ a: 1 }] } }, true) as any;
    expect(option.dataset.source).toEqual([{ a: 1 }]);
  });

  it("survives an empty option object", () => {
    expect(() => composeChartOption({}, false)).not.toThrow();
    expect(compose({}, false).title).toEqual({ show: false });
  });

  it("abbreviates large and small numbers on the value axis", () => {
    const option = compose({ yAxis: { type: "value" }, series: [] }, false) as any;
    const format = option.yAxis.axisLabel.formatter;
    expect(format(1500000)).toBe("1.5M");
    // Rounded, not truncated: 2500/1000 is 2.5 and rounds to 3.
    expect(format(2500)).toBe("3K");
    expect(format(2400)).toBe("2K");
    expect(format(999)).toBe("999");
    expect(format(42)).toBe("42");
    expect(format("plain")).toBe("plain");
    // A non-numeric value falls through to itself rather than to "NaN".
    expect(format("abc")).toBe("abc");
  });

  it("uses the category formatter for multi-part values", () => {
    const option = compose({ xAxis: { type: "category", data: [] }, series: [] }, false) as any;
    const format = option.xAxis.axisLabel.formatter;
    expect(format({ name: "Q1", value: 1 })).toBe("Q1");
    // Falls back to the raw `value`, uncoerced -- pre-existing behaviour, and
    // ECharts stringifies it on the way out.
    expect(format({ value: 7 })).toBe(7);
    expect(format(null)).toBe("");
  });

  it("rotates a crowded category axis and leaves a sparse one alone", () => {
    const crowded = (n: number) =>
      compose({ xAxis: { type: "category", data: Array.from({ length: n }, (_, i) => i) } }, false)
        .xAxis.axisLabel.rotate;
    expect(crowded(7)).toBe(25);
    expect(crowded(6)).toBe(0);
  });
});

describe("paramFilterFor", () => {
  const widget = (interactions: NonNullable<Widget["interactions"]>): Widget =>
    ({ id: "w", type: "chart", data: { source: "bi" }, interactions }) as Widget;

  it("writes the clicked field's value into every param it names", () => {
    const filter = paramFilterFor(
      widget([{ on: "click", set: { region: "region", team: "team" } }]),
      { data: { region: "EMEA", team: "blue" } },
    );
    expect(filter).toEqual({ region: "EMEA", team: "blue" });
  });

  it("falls back to the series name when the field is not in the datum", () => {
    const filter = paramFilterFor(widget([{ on: "click", set: { region: "region" } }]), {
      name: "APAC",
    });
    expect(filter).toEqual({ region: "APAC" });
  });

  it("ignores non-click interactions", () => {
    expect(
      paramFilterFor(widget([{ on: "brush", set: { region: "region" } }]), { data: {} }),
    ).toEqual({});
  });

  it("writes nothing when there is no interaction or no value", () => {
    expect(paramFilterFor(widget([]), { name: "x" })).toEqual({});
    expect(paramFilterFor(widget([{ on: "click", set: { r: "region" } }]), {})).toEqual({});
  });
});

describe("WidgetCard still wires the gate up", () => {
  const source = readFileSync(
    new URL("../../src/components/WidgetCard.tsx", import.meta.url),
    "utf8",
  );

  it("passes `hasTheme` straight through to the option composer", () => {
    // The extraction is only safe if nothing in between can alter the flag.
    expect(source).toContain("composeChartOption(res.option as Record<string, unknown>, hasTheme)");
  });

  it("re-renders when the flag or the params change", () => {
    expect(source).toContain("}, [widget, JSON.stringify(params), hasTheme]);");
  });
});

describe("App threads the effective theme through", () => {
  const source = readFileSync(new URL("../../src/App.tsx", import.meta.url), "utf8");

  it("computes the flag from the widget override, falling back to the dashboard", () => {
    // Per-widget WINS over dashboard-level, so the override is read first.
    expect(source).toContain("hasTheme={Boolean(widget.chartSpec?.theme_spec ?? spec.theme)}");
  });

  it("passes the dashboard theme alongside it", () => {
    expect(source).toContain("dashboardTheme={spec.theme}");
  });
});
