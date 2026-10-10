import { describe, it, expect, beforeEach } from "vite-plus/test";
import { Context } from "cordis";
import { ChartSpecService } from "../src/service.js";
import { THEME_PRESETS } from "flint-chart/core";
import type { BiQueryResult, Widget } from "@loams-core/types";

/**
 * `compile` used to hardcode `powerbi-light` as the assembler's `theme_spec` and
 * then unconditionally stamp the Power BI palette over `res.color` when the
 * result was ECharts' default `#5470c6`. Both behaviours silently defeated a
 * dashboard-level theme: the chart rendered in Power BI colours while the UI
 * showed a picker claiming some other house was active.
 *
 * These tests pin the fixed contract against the real service.
 */

const PBI_PALETTE_HEAD = "#118dff";

function makeService(): ChartSpecService {
  const ctx = new Context();
  ctx.provide("bi", {});
  return new ChartSpecService(ctx);
}

const ROWS: BiQueryResult = {
  data: [
    { category: "alpha", value: 10 },
    { category: "beta", value: 20 },
    { category: "gamma", value: 30 },
  ],
} as unknown as BiQueryResult;

const chartWidget = (themeSpec?: unknown): Widget =>
  ({
    id: "w1",
    type: "chart",
    position: { x: 0, y: 0, w: 6, h: 4 },
    title: "Test",
    datasetId: 1,
    chartSpec: {
      chartType: "Bar Chart",
      encodings: { x: { field: "category" }, y: { field: "value" } },
      theme_spec: themeSpec as never,
    },
  }) as unknown as Widget;

function paletteOf(option: Record<string, unknown>): string[] {
  const color = option?.color;
  return Array.isArray(color) ? (color as string[]) : [];
}

describe("ChartSpecService.compile theme handling", () => {
  let service: ChartSpecService;
  beforeEach(() => {
    service = makeService();
  });

  it("does not stamp the Power BI palette over a dashboard-themed chart", async () => {
    const themed = await service.compile(chartWidget(), ROWS, { preset: "economist" });
    // The regression: a themed chart came back wearing the PBI palette head.
    expect(paletteOf(themed)[0]).not.toBe(PBI_PALETTE_HEAD);
  });

  it("keeps the Power BI fallback when no theme is set at all", async () => {
    const unthemed = await service.compile(chartWidget(), ROWS);
    // "No theme" is a real renderable state and must still look like today.
    expect(paletteOf(unthemed)).toContain(PBI_PALETTE_HEAD);
  });

  it("treats an unknown preset as unthemed rather than throwing", async () => {
    const option = await service.compile(chartWidget(), ROWS, { preset: "no-such-house" });
    // Bad theme data must never be able to stop a chart rendering.
    expect(option).toBeTruthy();
    expect(Array.isArray(option.series)).toBe(true);
  });

  it("lets a per-widget override beat the dashboard theme", async () => {
    const widget = chartWidget({ ink: { text: { primary: "#123456" } } });
    const option = await service.compile(widget, ROWS, { preset: "economist" });
    expect(paletteOf(option)).not.toContain(PBI_PALETTE_HEAD);
  });

  it("resolves a bare per-widget preset name without error", async () => {
    const widget = chartWidget("economist");
    const option = await service.compile(widget, ROWS, undefined);
    expect(paletteOf(option)).not.toContain(PBI_PALETTE_HEAD);
  });

  it("accepts a bare preset-name string as the dashboard theme", async () => {
    const option = await service.compile(chartWidget(), ROWS, "economist");
    expect(paletteOf(option)).not.toContain(PBI_PALETTE_HEAD);
  });

  it("renders a chart with no rows without throwing", async () => {
    const option = await service.compile(
      chartWidget(),
      { data: [] } as unknown as BiQueryResult,
      { preset: "economist" },
    );
    expect(option).toBeTruthy();
  });
});

describe("every shipped preset survives compile", () => {
  it("produces a renderable option for each preset id", async () => {
    const service = makeService();
    for (const id of Object.keys(THEME_PRESETS)) {
      const option = await service.compile(chartWidget(), ROWS, { preset: id });
      expect(option, `preset ${id} produced no option`).toBeTruthy();
      expect(Array.isArray(option.series), `preset ${id} produced no series`).toBe(true);
    }
  });
});
