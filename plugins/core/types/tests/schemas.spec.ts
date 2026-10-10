import { describe, it, expect } from "vite-plus/test";
import { DashboardSpecSchema, WidgetSchema, LayoutItemSchema } from "../src/dashboard-spec.js";
import { randomUUID } from "crypto";

describe("Zod Schemas", () => {
  it("Valid DashboardSpec parses successfully", () => {
    const validSpec = {
      id: randomUUID(),
      version: 1,
      title: "Sales Dashboard",
      params: [],
      layout: [{ id: randomUUID(), x: 0, y: 0, w: 12, h: 4 }],
      widgets: {
        "widget-1": {
          id: randomUUID(),
          type: "text",
        },
      },
    };
    const result = DashboardSpecSchema.safeParse(validSpec);
    expect(result.success).toBe(true);
  });

  it("Invalid specs (missing title, negative version) are rejected", () => {
    const invalidSpec = {
      id: randomUUID(),
      version: -1, // negative
      // title missing
      params: [],
      layout: [],
      widgets: {},
    };
    const result = DashboardSpecSchema.safeParse(invalidSpec);
    expect(result.success).toBe(false);
  });

  it("Widget XOR constraint works: chart-type widget with neither chart-specs nor chart is rejected", () => {
    const widget = {
      id: randomUUID(),
      type: "chart",
      // no chart-specs, no chart
    };
    const result = WidgetSchema.safeParse(widget);
    expect(result.success).toBe(false);
  });

  it("Widget XOR constraint works: chart-type widget with both chart-specs and chart is rejected", () => {
    const widget = {
      id: randomUUID(),
      type: "chart",
      chartSpec: {
        chartType: "bar",
        encodings: {},
      },
      chart: {
        kind: "bar",
        encode: {},
      },
    };
    const result = WidgetSchema.safeParse(widget);
    expect(result.success).toBe(false);
  });

  it("Non-chart widgets (text, filter) pass without chartSpec/chart", () => {
    const widget1 = {
      id: randomUUID(),
      type: "text",
    };
    const widget2 = {
      id: randomUUID(),
      type: "filter",
    };
    expect(WidgetSchema.safeParse(widget1).success).toBe(true);
    expect(WidgetSchema.safeParse(widget2).success).toBe(true);
  });

  it("LayoutItem rejects w > 12 or negative x/y", () => {
    const invalidW = { id: randomUUID(), x: 0, y: 0, w: 13, h: 4 };
    const invalidX = { id: randomUUID(), x: -1, y: 0, w: 6, h: 4 };
    const invalidY = { id: randomUUID(), x: 0, y: -2, w: 6, h: 4 };

    expect(LayoutItemSchema.safeParse(invalidW).success).toBe(false);
    expect(LayoutItemSchema.safeParse(invalidX).success).toBe(false);
    expect(LayoutItemSchema.safeParse(invalidY).success).toBe(false);
  });
});

describe("legacy widget documents", () => {
  it("upgrades `flint` and `source: superset` on input", () => {
    const result = WidgetSchema.safeParse({
      id: "w",
      type: "chart",
      data: { source: "superset", datasetId: 1 },
      flint: { chartType: "Bar Chart", encodings: {} },
    });
    expect(result.success).toBe(true);
    if (result.success) {
      expect(result.data.data?.source).toBe("bi");
      expect(result.data.chartSpec).toBeDefined();
      expect("flint" in result.data).toBe(false);
    }
  });
});
