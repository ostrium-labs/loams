import { describe, it, expect, beforeEach } from "vite-plus/test";
import { Context } from "cordis";
import { FlintService } from "../src/service.js";
import { THEME_PRESETS, resolveThemeSpec } from "flint-chart/core";
import type { Widget } from "@loams-plugins/types";

function makeService(): FlintService {
  const ctx = new Context();
  ctx.provide("controlPlane", {});
  return new FlintService(ctx);
}

const chartWidget = (themeSpec?: unknown): Widget => ({
  id: "w1",
  type: "chart",
  flint: { chartType: "Bar Chart", encodings: {}, theme_spec: themeSpec as never },
});

describe("FlintService.listThemes", () => {
  let service: FlintService;
  beforeEach(() => {
    service = makeService();
  });

  it("returns one entry per shipped preset, carrying icons", () => {
    const themes = service.listThemes();
    expect(themes).toHaveLength(Object.keys(THEME_PRESETS).length);
    for (const t of themes) {
      expect(t.id).toBeTruthy();
      expect(t.label).toBeTruthy();
      expect(t.description).toBeTruthy();
      // The reason we read THEME_PRESETS instead of listThemePresets(): icons.
      expect(t.icon.length).toBeGreaterThan(0);
      expect(t.icon).toContain("<svg");
    }
  });

  it("sorts by label for a stable picker order", () => {
    const labels = service.listThemes().map((t) => t.label);
    const sorted = [...labels].sort((a, b) => a.localeCompare(b, "en"));
    expect(labels).toEqual(sorted);
  });

  it('exposes a distinct icon for "no theme"', () => {
    expect(service.defaultThemeIcon()).toContain("<svg");
    expect(service.defaultThemeIcon()).not.toBe(service.listThemes()[0]?.icon);
  });
});

describe("FlintService.resolveTheme", () => {
  let service: FlintService;
  beforeEach(() => {
    service = makeService();
  });

  it("resolves a valid preset selection", () => {
    const res = service.resolveTheme({ preset: "economist" });
    expect(res.valid).toBe(true);
    expect(res.source).toBe("preset");
    expect(res.report).toEqual([]);
    expect(res.spec?.id).toBe("economist");
  });

  it("accepts a bare preset-name string", () => {
    const res = service.resolveTheme("swiss");
    expect(res.valid).toBe(true);
    expect(res.spec?.id).toBe("swiss");
  });

  it("returns no theme for nullish input", () => {
    for (const input of [undefined, null, {}]) {
      const res = service.resolveTheme(input);
      expect(res).toEqual({ valid: true, source: "none", report: [], spec: undefined });
    }
  });

  // The important regression test: flint treats an unknown preset name as an
  // ERROR rather than silently falling back to some other house. The service
  // must absorb that into a report, never throw, and never invent a spec.
  it("does NOT throw on an unknown preset name, and reports valid: false", () => {
    const res = service.resolveTheme({ preset: "nope-not-real" });
    expect(res.valid).toBe(false);
    expect(res.spec).toBeUndefined();
    expect(res.report).toHaveLength(1);
    expect(res.report[0]?.stage).toBe("ground");
    expect(res.report[0]?.path).toBe("theme.preset");
    expect(res.report[0]?.message).toContain("nope-not-real");
  });

  it("does not throw on an unknown bare preset name either", () => {
    const res = service.resolveTheme("nope-not-real");
    expect(res.valid).toBe(false);
    expect(res.report[0]?.path).toBe("theme.preset");
  });

  it("reports an unknown base named by a custom spec.extends", () => {
    const res = service.resolveTheme({
      custom: { extends: "nope-not-real", ink: { accent: "#111" } },
    });
    expect(res.valid).toBe(false);
    expect(res.report[0]?.path).toBe("theme.custom.extends");
  });
});

describe("FlintService.resolveTheme — inheritance semantics", () => {
  let service: FlintService;
  beforeEach(() => {
    service = makeService();
  });

  it("MERGES nested policy objects from the base house", () => {
    const res = service.resolveTheme({
      custom: { extends: "nyt", ink: { text: { primary: "#111111" } } },
    });
    expect(res.valid).toBe(true);
    // The stated key wins...
    expect(res.spec?.ink?.text?.primary).toBe("#111111");
    // ...while its nested siblings survive the merge.
    const baseText = resolveThemeSpec("nyt")?.ink?.text;
    expect(res.spec?.ink?.text?.secondary).toBe(baseText?.secondary);
    expect(res.spec?.ink?.surface).toEqual(resolveThemeSpec("nyt")?.ink?.surface);
  });

  it("REPLACES arrays rather than merging them element-wise", () => {
    const res = service.resolveTheme({
      custom: { extends: "nyt", ink: { series: { categorical: ["#111", "#222"] } } },
    });
    expect(res.valid).toBe(true);
    expect(res.spec?.ink?.series?.categorical).toEqual(["#111", "#222"]);
    expect(res.spec?.ink?.series?.categorical).toHaveLength(2);
  });

  it("layers a selection-level preset UNDER a custom spec that names no base", () => {
    const res = service.resolveTheme({
      preset: "nyt",
      custom: { ink: { series: { categorical: ["#abc"] } } },
    });
    expect(res.valid).toBe(true);
    expect(res.source).toBe("custom");
    expect(res.spec?.ink?.series?.categorical).toEqual(["#abc"]);
    // base house merged in underneath
    expect(res.spec?.ink?.text?.primary).toBe(resolveThemeSpec("nyt")?.ink?.text?.primary);
  });

  it("lets an explicit custom.extends outrank the selection preset", () => {
    const res = service.resolveTheme({ preset: "nyt", custom: { extends: "economist" } });
    expect(res.valid).toBe(true);
    expect(res.spec?.ink?.text?.primary).toBe(resolveThemeSpec("economist")?.ink?.text?.primary);
  });

  it("passes unknown extra keys through the service without stripping them", () => {
    const res = service.resolveTheme({
      custom: { ink: { text: { primary: "#121212" }, futureInkBlock: { k: 1 } } },
    });
    expect(res.valid).toBe(true);
    expect(res.spec).toHaveProperty("ink.futureInkBlock");
  });
});

describe("FlintService.resolveWidgetTheme — precedence", () => {
  let service: FlintService;
  beforeEach(() => {
    service = makeService();
  });

  it("uses the dashboard-level theme when the widget has no override", () => {
    const res = service.resolveWidgetTheme(chartWidget(), { preset: "swiss" });
    expect(res.valid).toBe(true);
    expect(res.spec?.id).toBe("swiss");
  });

  it("lets a per-widget theme_spec WIN over the dashboard-level theme", () => {
    const res = service.resolveWidgetTheme(chartWidget("economist"), { preset: "swiss" });
    expect(res.valid).toBe(true);
    expect(res.source).toBe("preset");
    expect(res.spec?.id).toBe("economist");
  });

  it("lets a per-widget custom spec win, layered onto the dashboard house", () => {
    const res = service.resolveWidgetTheme(
      chartWidget({ ink: { series: { categorical: ["#f00"] } } }),
      { preset: "nyt" },
    );
    expect(res.valid).toBe(true);
    expect(res.source).toBe("custom");
    expect(res.spec?.ink?.series?.categorical).toEqual(["#f00"]);
    expect(res.spec?.ink?.text?.primary).toBe(resolveThemeSpec("nyt")?.ink?.text?.primary);
  });

  it("falls back to the dashboard theme when the per-widget override is invalid", () => {
    const res = service.resolveWidgetTheme(chartWidget({ ink: { accent: "" } }), {
      preset: "swiss",
    });
    expect(res.valid).toBe(true);
    expect(res.spec?.id).toBe("swiss");
  });

  it("accepts a bare preset-name string as the dashboard theme", () => {
    const res = service.resolveWidgetTheme(chartWidget(), "nature");
    expect(res.spec?.id).toBe("nature");
  });
});

describe("FlintService.groundTheme", () => {
  let service: FlintService;
  beforeEach(() => {
    service = makeService();
  });

  it("grounds a spec against a chart and always returns a report array", () => {
    const spec = resolveThemeSpec("nyt");
    expect(spec).toBeDefined();
    const res = service.groundTheme(spec!, "Bar Chart");
    expect(res.valid).toBe(true);
    expect(Array.isArray(res.report)).toBe(true);
    expect(typeof res.decisions?.bound.seriesCount).toBe("number");
  });

  it("reports downgrades rather than swallowing them", () => {
    const spec = resolveThemeSpec("nyt");
    const res = service.groundTheme(spec!, "Bar Chart");
    // Unknown mark channel + no table means the ground cannot honour everything,
    // and flint says so rather than pretending it did.
    expect(res.report.length).toBeGreaterThan(0);
    for (const entry of res.report) {
      expect(entry.stage).toBe("ground");
      expect(entry.path).toBeTruthy();
      expect(entry.message).toBeTruthy();
    }
  });

  it("honours the facts it IS given", () => {
    const spec = resolveThemeSpec("economist");
    const res = service.groundTheme(spec!, "Bar Chart", {
      markChannel: "length",
      markTypes: ["bar"],
      canvasSize: { width: 640, height: 400 },
      titled: true,
      headline: "Revenue by region",
    });
    expect(res.valid).toBe(true);
    expect(res.decisions?.bound.markChannel).toBe("length");
  });
});
