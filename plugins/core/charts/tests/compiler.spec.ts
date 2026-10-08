import { describe, it, expect } from "vite-plus/test";
import { compileNativeWidget, applyOverrides } from "../src/compiler.js";

describe("Compiler", () => {
  const data = [{ x: 1, y: 2 }];

  it("compiles line chart option", () => {
    const widget = { chart: { kind: "line" } };
    const option = compileNativeWidget(widget, data);
    expect(option).toEqual({
      dataset: { source: data },
      series: [{ type: "line" }],
    });
  });

  it("compiles bar chart option", () => {
    const widget = { chart: { kind: "bar" } };
    const option = compileNativeWidget(widget, data);
    expect(option).toEqual({
      dataset: { source: data },
      series: [{ type: "bar" }],
    });
  });

  it("compiles pie chart option", () => {
    const widget = { chart: { kind: "pie" } };
    const option = compileNativeWidget(widget, data);
    expect(option).toEqual({
      dataset: { source: data },
      series: [{ type: "pie" }],
    });
  });

  it("throws on unknown chart kind", () => {
    const widget = { chart: { kind: "unknown_chart" } };
    expect(() => compileNativeWidget(widget, data)).toThrowError("Unknown chart kind");
  });

  it("applyOverrides applies only allowlisted keys", () => {
    const option = { title: { text: "Old" } };
    const overrides = { title: { text: "New" }, malicious: "true" };
    const result = applyOverrides(option, overrides);
    expect(result).toEqual({ title: { text: "New" } });
    expect(result.malicious).toBeUndefined();
  });

  it("applyOverrides deep merges objects", () => {
    const option = { tooltip: { show: true, trigger: "axis" } };
    const overrides = { tooltip: { show: false, backgroundColor: "red" } };
    const result = applyOverrides(option, overrides);
    expect(result).toEqual({ tooltip: { show: false, trigger: "axis", backgroundColor: "red" } });
  });
});
