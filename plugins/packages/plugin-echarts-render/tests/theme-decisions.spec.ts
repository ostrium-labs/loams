import { describe, it, expect } from "vite-plus/test";
import {
  applyGroundedInk,
  buildThemeFacts,
  collectUnmappedDecisions,
  countColorDemands,
  readGroundedInk,
  resolvePalette,
} from "../src/theme-decisions.js";
import type { DesignDecisions } from "flint-chart/core";

/**
 * A grounded theme, written out.
 *
 * Cast rather than constructed: `DesignDecisions` has ~40 required fields and
 * this suite exercises the handful the mapping reads. The end-to-end proof that
 * real flint decisions map correctly lives in `theme-bridge.spec.ts`, which
 * grounds an actual shipped house.
 */
function grounded(overrides?: Record<string, unknown>): DesignDecisions {
  return {
    surface: { canvas: "#ffffff" },
    text: { primary: "#111111", secondary: "#555555", muted: "#999999", inverse: "#ffffff" },
    series: { mode: "categorical", single: "#aaaaaa", categorical: ["#c1", "#c2", "#c3"] },
    axes: {},
    legend: { show: true, label: { color: "#555555" } },
    dataLabels: { show: false, inkMode: "fixed", text: { color: "#111111" } },
    marks: {},
    ...overrides,
  } as unknown as DesignDecisions;
}

/** A compiled bar chart: two axes, one dataset-bound series. */
function barOption() {
  return {
    dataset: { source: [{ region: "a", value: 1 }] },
    xAxis: { type: "category" },
    yAxis: { type: "value" },
    series: [{ type: "bar", name: "value", encode: { x: "region", y: "value" } }],
    tooltip: { trigger: "axis" },
    legend: {},
  };
}

describe("buildThemeFacts", () => {
  it("calls a value axis quantitative, because ECharts says so itself", () => {
    const facts = buildThemeFacts(barOption());
    expect(facts.channelSemantics).toEqual({
      x: { field: "region" },
      y: { field: "value", type: "quantitative" },
    });
  });

  // The important restraint: ECharts cannot tell a nominal category from an
  // ordinal or a temporal one, so no type is claimed. Grounding binds the axis
  // as categorical and says in its report that it did not know more.
  it("claims no semantic type for a category axis", () => {
    const facts = buildThemeFacts(barOption());
    expect((facts.channelSemantics as Record<string, any>).x.type).toBeUndefined();
  });

  it("states nothing about an axis the option does not carry", () => {
    const facts = buildThemeFacts({ series: [{ type: "pie", data: [{ value: 1 }] }] });
    expect(facts.channelSemantics).toEqual({});
  });

  it("translates ECharts series types into flint mark families", () => {
    expect(buildThemeFacts({ series: [{ type: "bar" }] }).markTypes).toEqual(["band"]);
    expect(buildThemeFacts({ series: [{ type: "pie" }] }).markTypes).toEqual(["arc"]);
    expect(buildThemeFacts({ series: [{ type: "heatmap" }] }).markTypes).toEqual(["cell"]);
    // An unrecognized family contributes nothing rather than a wrong answer.
    expect(buildThemeFacts({ series: [{ type: "sankey" }] }).markTypes).toBeUndefined();
  });

  it("declares a part-to-whole mark, and only for marks that are one", () => {
    expect(buildThemeFacts({ series: [{ type: "pie" }] }).partToWhole).toBe(true);
    expect(buildThemeFacts({ series: [{ type: "bar" }] }).partToWhole).toBeUndefined();
  });

  it("records a title only when one is really there", () => {
    expect(buildThemeFacts({ ...barOption(), title: { text: 'Sales "net"' } }).titled).toBe(true);
    expect(buildThemeFacts({ ...barOption(), title: { show: false } }).titled).toBeUndefined();
    expect(buildThemeFacts(barOption()).titled).toBeUndefined();
  });

  it("never claims a mark channel", () => {
    // The one fact this path must not invent: naming a cognitive channel would
    // make grounding resolve policy against a channel the chart may not have.
    expect(buildThemeFacts(barOption())).not.toHaveProperty("markChannel");
  });

  it("passes rows through only when the caller has them", () => {
    const rows = [{ a: 1 }];
    expect(buildThemeFacts(barOption(), rows).table).toBe(rows);
    expect(buildThemeFacts(barOption())).not.toHaveProperty("table");
  });
});

describe("countColorDemands", () => {
  it("counts one ink per ordinary series", () => {
    expect(countColorDemands({ series: [{ type: "bar" }, { type: "line" }] })).toBe(2);
  });

  it("counts one ink per slice for a part-to-whole mark", () => {
    expect(
      countColorDemands({
        series: [{ type: "pie", data: [{ value: 1 }, { value: 2 }, { value: 3 }] }],
      }),
    ).toBe(3);
  });

  it("counts the rows when a pie is bound to a dataset instead of carrying data", () => {
    const rows = [{ a: 1 }, { a: 2 }, { a: 3 }, { a: 4 }];
    expect(countColorDemands({ series: [{ type: "pie", encode: { itemName: "a" } }] }, rows)).toBe(
      4,
    );
  });

  it("reports no demand for an option with no series", () => {
    expect(countColorDemands({})).toBe(0);
  });
});

describe("resolvePalette", () => {
  it("gives a single-mark chart the house's single ink", () => {
    expect(resolvePalette(readGroundedInk(grounded()), 1)).toEqual(["#aaaaaa"]);
  });

  it("gives a multi-series chart the categorical set", () => {
    expect(resolvePalette(readGroundedInk(grounded()), 2)).toEqual(["#c1", "#c2", "#c3"]);
  });

  it("appends the overflow ink when the chart needs more names than the house named", () => {
    const ink = readGroundedInk(
      grounded({ series: { mode: "categorical", categorical: ["#c1", "#c2"], overflow: "#o1" } }),
    );
    expect(resolvePalette(ink, 3)).toEqual(["#c1", "#c2", "#o1"]);
    expect(resolvePalette(ink, 2)).toEqual(["#c1", "#c2"]);
  });

  // Grounding saying "the house set does not apply at this cardinality" is an
  // instruction, not a hint: the option keeps the colour it already had.
  it("declines to impose a palette grounding declared exhausted", () => {
    const ink = readGroundedInk(
      grounded({ series: { mode: "categorical", categorical: ["#c1"], exhausted: true } }),
    );
    expect(resolvePalette(ink, 9)).toBeUndefined();
  });

  it("says nothing when the theme names no ink", () => {
    expect(
      resolvePalette(readGroundedInk(grounded({ series: { mode: "single" } })), 2),
    ).toBeUndefined();
  });
});

describe("applyGroundedInk — axes", () => {
  const axisDecisions = grounded({
    axes: {
      x: {
        role: "categorical",
        orient: "bottom",
        domain: { show: true, color: "#222222", width: 2 },
        ticks: { show: false, color: "transparent", size: 0 },
        grid: { show: false, color: "transparent", width: 0 },
        label: { color: "#444444", padding: 4 },
        title: { show: false, color: "#444444" },
      },
      y: {
        role: "measure",
        orient: "left",
        domain: { show: false, color: "transparent", width: 0 },
        ticks: { show: false, color: "transparent", size: 0 },
        grid: { show: true, color: "#dddddd", width: 1 },
        label: { color: "#444444", padding: 4 },
        title: { show: false, color: "#444444" },
      },
    },
  });

  it("writes the axis rule, its ink and its weight", () => {
    const out = applyGroundedInk(barOption(), axisDecisions);
    expect(out.xAxis).toMatchObject({
      axisLine: { show: true, lineStyle: { color: "#222222", width: 2 } },
    });
  });

  it("turns a rule the house omits off, rather than leaving the default line", () => {
    const out = applyGroundedInk(barOption(), axisDecisions);
    expect((out.yAxis as any).axisLine).toEqual({ show: false });
    expect((out.xAxis as any).axisTick).toEqual({ show: false });
  });

  it("writes the split line where the axis declares a grid", () => {
    const out = applyGroundedInk(barOption(), axisDecisions);
    expect((out.yAxis as any).splitLine).toEqual({
      show: true,
      lineStyle: { color: "#dddddd", width: 1 },
    });
    expect((out.xAxis as any).splitLine).toEqual({ show: false });
  });

  it("writes axis label and axis title ink", () => {
    const out = applyGroundedInk(barOption(), axisDecisions);
    expect((out.xAxis as any).axisLabel.color).toBe("#444444");
    expect((out.xAxis as any).nameTextStyle.color).toBe("#444444");
  });

  it("falls back to the baseline rule when grounding bound no axis", () => {
    const out = applyGroundedInk(
      barOption(),
      grounded({ baseline: { show: true, color: "#010101", width: 1 } }),
    );
    expect((out.xAxis as any).axisLine).toEqual({
      show: true,
      lineStyle: { color: "#010101", width: 1 },
    });
    expect((out.xAxis as any).axisLabel.color).toBe("#555555");
  });

  it("styles every axis when ECharts wrote them as an array", () => {
    const out = applyGroundedInk({ xAxis: [{ type: "value" }] }, axisDecisions);
    expect((out.xAxis as any)[0].axisLabel.color).toBe("#444444");
  });

  it("expands the boolean axisLine shorthand rather than dropping the ink", () => {
    const out = applyGroundedInk({ xAxis: { axisLine: true } }, axisDecisions);
    expect((out.xAxis as any).axisLine).toEqual({
      show: true,
      lineStyle: { color: "#222222", width: 2 },
    });
  });
});

describe("applyGroundedInk — series, legend, tooltip, title", () => {
  it("inks a value label only when the house declares a fixed label ink", () => {
    const option = { series: [{ type: "bar", label: { show: true } }] };
    expect((applyGroundedInk(option, grounded()).series as any)[0].label.color).toBe("#111111");

    for (const inkMode of ["contrastWithMark", "matchSeries"]) {
      const themed = grounded({ dataLabels: { show: true, inkMode, text: { color: "#111111" } } });
      expect((applyGroundedInk(option, themed).series as any)[0].label).toEqual({ show: true });
    }
  });

  it("leaves a series with no label block alone", () => {
    const out = applyGroundedInk({ series: [{ type: "bar" }] }, grounded());
    expect(out.series).toEqual([{ type: "bar" }]);
  });

  it("separates wedges with the house's slice rule", () => {
    const decisions = grounded({ marks: { slice: { gap: 1.5, style: "rule", color: "#ffffff" } } });
    const out = applyGroundedInk({ series: [{ type: "pie", data: [{ value: 1 }] }] }, decisions);
    expect((out.series as any)[0].itemStyle).toEqual({ borderColor: "#ffffff", borderWidth: 1.5 });
  });

  it("does not invent a separator for a mark the house draws without one", () => {
    const out = applyGroundedInk({ series: [{ type: "pie", data: [{ value: 1 }] }] }, grounded());
    expect((out.series as any)[0].itemStyle).toBeUndefined();
  });

  it("writes legend label ink, and hides a legend the house omitted", () => {
    const out = applyGroundedInk(
      barOption(),
      grounded({ legend: { show: false, label: { color: "#54585a" } } }),
    );
    expect(out.legend).toEqual({ textStyle: { color: "#54585a" }, show: false });
  });

  it("gives the tooltip the house panel, its edge and its label ink", () => {
    const decisions = grounded({ surface: { canvas: "#ffffff", panel: "#faf9f8" } });
    expect(applyGroundedInk(barOption(), decisions).tooltip).toEqual({
      trigger: "axis",
      backgroundColor: "#faf9f8",
      borderColor: "#ffffff",
      borderWidth: 1,
      textStyle: { color: "#111111" },
    });
  });

  it("falls back to the canvas when the house has no separate panel", () => {
    const out = applyGroundedInk(barOption(), grounded());
    expect((out.tooltip as any).backgroundColor).toBe("#ffffff");
    expect((out.tooltip as any).borderColor).toBe("#555555");
  });

  it("does not invent a tooltip for a chart that has none", () => {
    const out = applyGroundedInk({ series: [{ type: "bar" }] }, grounded());
    expect(out.tooltip).toBeUndefined();
  });

  it("inks the title and its subtext", () => {
    const out = applyGroundedInk(
      { ...barOption(), title: { text: "Sales", subtext: "net" } },
      grounded(),
    );
    expect(out.title).toEqual({
      text: "Sales",
      subtext: "net",
      textStyle: { color: "#111111" },
      subtextStyle: { color: "#555555" },
    });
  });

  it("hands a sampled ramp to the visualMap when one exists", () => {
    const decisions = grounded({ series: { mode: "sequential", range: ["#0a0", "#0f0"] } });
    const out = applyGroundedInk({ ...barOption(), visualMap: { min: 0, max: 1 } }, decisions);
    expect(out.visualMap).toEqual({
      min: 0,
      max: 1,
      inRange: { color: ["#0a0", "#0f0"] },
      textStyle: { color: "#555555" },
    });
  });
});

describe("applyGroundedInk — purity", () => {
  it("never mutates the option it was handed", () => {
    const option = barOption();
    const before = JSON.parse(JSON.stringify(option));
    applyGroundedInk(
      option,
      grounded({ axes: { x: {}, y: {} }, baseline: { show: true, color: "#010101", width: 1 } }),
    );
    expect(option).toEqual(before);
  });

  it("returns the same reference when the theme says nothing applicable", () => {
    const option = barOption();
    const silence = {
      series: { mode: "single" },
      axes: {},
      text: {},
    } as unknown as DesignDecisions;
    expect(applyGroundedInk(option, silence)).toBe(option);
  });
});

describe("collectUnmappedDecisions", () => {
  it("says when grounding bound no axis at all", () => {
    expect(collectUnmappedDecisions(grounded())).toContain("axes.*");
  });

  it("names the status ink the mapping refuses to colour series by", () => {
    const decisions = grounded({
      series: {
        mode: "status",
        single: "#a",
        categorical: [],
        status: { positive: "#0f0", negative: "#f00" },
      },
    });
    const unmapped = collectUnmappedDecisions(decisions);
    expect(unmapped).toContain("series.status.*");
    expect(unmapped).toContain("series.mode=status");
  });

  it("names a zero rule, which has no option key", () => {
    const decisions = grounded({
      axes: { x: { zeroRule: { show: true, color: "#000", width: 1 } } },
    });
    expect(collectUnmappedDecisions(decisions)).toContain("axes.x.zeroRule");
  });
});
