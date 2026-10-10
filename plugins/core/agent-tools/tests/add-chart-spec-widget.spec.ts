import { Context } from "cordis";
import { describe, expect, it, vi } from "vitest";
import { AgentToolsService } from "../src/service.js";

function harness() {
  const ctx = new Context();
  const addWidget = vi.fn().mockResolvedValue({ ok: true });
  const validate = vi.fn().mockImplementation(async (spec: unknown) => ({
    valid: true,
    errors: [],
    spec,
  }));
  for (const name of ["data", "render", "bi", "store"]) ctx.provide(name, {} as never);
  ctx.provide("dashboard", { addWidget } as never);
  ctx.provide("chartSpecs", { validate } as never);
  const service = new AgentToolsService(ctx);
  const tool = (name: string) => service.getToolDefinitions().find((t) => t.name === name)!;
  return { tool, addWidget, validate };
}

const SPEC = { chartType: "Bar Chart", encodings: {} };

describe("add_chart_spec_widget and its deprecated alias", () => {
  it("add_chart_spec_widget takes chartSpec", async () => {
    const { tool, addWidget } = harness();
    await tool("add_chart_spec_widget").handler({ dashboardId: "d", datasetId: 1, chartSpec: SPEC });
    const widget = addWidget.mock.calls[0][1];
    expect(widget.chartSpec).toEqual(SPEC);
    expect(widget.data).toEqual({ source: "bi", datasetId: 1 });
  });

  it("add_chart_spec_widget falls back to flintSpec", async () => {
    const { tool, addWidget } = harness();
    await tool("add_chart_spec_widget").handler({ dashboardId: "d", datasetId: 1, flintSpec: SPEC });
    expect(addWidget.mock.calls[0][1].chartSpec).toEqual(SPEC);
  });

  it("add_flint_widget still works and is marked deprecated", async () => {
    const { tool, addWidget } = harness();
    const def = tool("add_flint_widget");
    expect(def.description).toMatch(/^Deprecated: use add_chart_spec_widget/);
    await def.handler({ dashboardId: "d", datasetId: 2, flintSpec: SPEC });
    expect(addWidget.mock.calls[0][1].chartSpec).toEqual(SPEC);
  });

  it("rejects an invalid spec", async () => {
    const { tool, validate } = harness();
    validate.mockResolvedValueOnce({ valid: false, errors: ["bad"], spec: undefined });
    await expect(
      tool("add_chart_spec_widget").handler({ dashboardId: "d", datasetId: 1, chartSpec: {} }),
    ).rejects.toThrow(/Invalid chart spec: bad/);
  });
});
