import { describe, it, expect } from "vite-plus/test";
import {
  DashboardSpecSchema,
  ThemeInkSchema,
  ThemeSelectionSchema,
  ThemeSpecSchema,
} from "../src/index.js";

describe("ThemeSpecSchema — authoring subset", () => {
  it("accepts a minimal spec with only ink overrides", () => {
    const parsed = ThemeSpecSchema.safeParse({ ink: { text: { primary: "#121212" } } });
    expect(parsed.success).toBe(true);
  });

  it("accepts both a preset id and a custom spec", () => {
    const parsed = ThemeSelectionSchema.safeParse({
      preset: "economist",
      custom: { extends: "economist", ink: { accent: "#e3120b" } },
    });
    expect(parsed.success).toBe(true);
  });

  it('treats a selection with neither field as valid — "no theme" is a real state', () => {
    const parsed = ThemeSelectionSchema.safeParse({});
    expect(parsed.success).toBe(true);
  });

  it("rejects a non-empty-string requirement on the preset id", () => {
    expect(ThemeSelectionSchema.safeParse({ preset: "" }).success).toBe(false);
  });
});

describe("ThemeSpecSchema — strict colour validation", () => {
  it("rejects an empty colour string", () => {
    const parsed = ThemeSpecSchema.safeParse({ ink: { text: { primary: "" } } });
    expect(parsed.success).toBe(false);
  });

  it("rejects an empty accent", () => {
    expect(ThemeSpecSchema.safeParse({ ink: { accent: "" } }).success).toBe(false);
  });

  it("rejects an empty colour inside a structural ink", () => {
    expect(ThemeSpecSchema.safeParse({ ink: { structure: { zero: "" } } }).success).toBe(false);
  });

  it("rejects an empty colour inside a ramp stop", () => {
    expect(
      ThemeSpecSchema.safeParse({ ink: { series: { sequential: { stops: ["#000", ""] } } } })
        .success,
    ).toBe(false);
  });

  it("rejects a non-string in ink.series.categorical", () => {
    const parsed = ThemeSpecSchema.safeParse({ ink: { series: { categorical: ["#111", 42] } } });
    expect(parsed.success).toBe(false);
  });

  it("rejects a non-array categorical", () => {
    expect(ThemeSpecSchema.safeParse({ ink: { series: { categorical: "#111" } } }).success).toBe(
      false,
    );
  });

  it("rejects a bad colour in the mirrored type roles", () => {
    expect(ThemeSpecSchema.safeParse({ type: { headline: { color: "" } } }).success).toBe(false);
  });

  it("accepts a well-formed custom ink block", () => {
    const parsed = ThemeInkSchema.safeParse({
      surface: { source: "house", canvas: "#ffffff" },
      text: { primary: "#121212", secondary: "#6b6b6b" },
      structure: { axis: "#cccccc", zero: "#e3120b" },
      series: {
        single: "#118dff",
        categorical: ["#118dff", "#12239e", "#e66c37"],
        categoricalExtended: ["#118dff", "#12239e"],
        overflow: "#9a9a9a",
        status: { positive: "#0f7b3f", negative: "#c8102e", neutral: "#6b6b6b" },
      },
      accent: "#e3120b",
    });
    expect(parsed.success).toBe(true);
  });
});

describe("ThemeSpecSchema — passthrough preserves unknown keys", () => {
  it("round-trips an unknown TOP-level key from a newer flint", () => {
    const authored = {
      ink: { text: { primary: "#121212" } },
      someFutureTopLevelBlock: { enabled: true, strength: 0.5 },
    };
    const parsed = ThemeSpecSchema.safeParse(authored);
    expect(parsed.success).toBe(true);
    if (!parsed.success) return;
    expect(parsed.data).toHaveProperty("someFutureTopLevelBlock");
    expect((parsed.data as Record<string, unknown>).someFutureTopLevelBlock).toEqual({
      enabled: true,
      strength: 0.5,
    });
    // And it survives an actual JSON round trip, which is what storing a dashboard does.
    expect(JSON.parse(JSON.stringify(parsed.data))).toEqual(authored);
  });

  it("round-trips an unknown key nested INSIDE ink", () => {
    const authored = { ink: { text: { primary: "#121212" }, futureInkBlock: { k: 1 } } };
    const parsed = ThemeSpecSchema.safeParse(authored);
    expect(parsed.success).toBe(true);
    if (!parsed.success) return;
    expect(JSON.parse(JSON.stringify(parsed.data))).toEqual(authored);
  });

  it("round-trips unknown keys inside an opaque policy block", () => {
    const authored = { legend: { placement: "seriesEnd", futureLegendKnob: "x" } };
    const parsed = ThemeSpecSchema.safeParse(authored);
    expect(parsed.success).toBe(true);
    if (!parsed.success) return;
    expect(JSON.parse(JSON.stringify(parsed.data))).toEqual(authored);
  });
});

describe("DashboardSpecSchema — dashboard-level theme", () => {
  const base = {
    id: "d1",
    version: 0,
    title: "Sales",
    params: [],
    layout: [],
    widgets: {},
  };

  it("accepts a dashboard with no theme", () => {
    expect(DashboardSpecSchema.safeParse(base).success).toBe(true);
  });

  it("accepts a dashboard-level theme selection", () => {
    const parsed = DashboardSpecSchema.safeParse({ ...base, theme: { preset: "swiss" } });
    expect(parsed.success).toBe(true);
    if (!parsed.success) return;
    expect(parsed.data.theme?.preset).toBe("swiss");
  });

  it("rejects a dashboard whose theme carries a bad colour", () => {
    expect(
      DashboardSpecSchema.safeParse({ ...base, theme: { custom: { ink: { accent: "" } } } })
        .success,
    ).toBe(false);
  });

  it("keeps the per-widget theme_spec override independent of the dashboard theme", () => {
    const parsed = DashboardSpecSchema.safeParse({
      ...base,
      theme: { preset: "swiss" },
      widgets: {
        w1: {
          id: "w1",
          type: "chart",
          flint: { chartType: "Bar Chart", encodings: {}, theme_spec: "economist" },
        },
      },
    });
    expect(parsed.success).toBe(true);
    if (!parsed.success) return;
    expect(parsed.data.widgets.w1?.flint?.theme_spec).toBe("economist");
    expect(parsed.data.theme?.preset).toBe("swiss");
  });
});
