import { Context } from "cordis";
import { describe, it, expect, vi, beforeEach } from "vite-plus/test";
import { DataService } from "../src/service.js";

describe("DataService", () => {
  let ctx: Context;
  beforeEach(async () => {
    ctx = new Context();
    ctx.provide("controlPlane");
    ctx.set("controlPlane", { queryData: vi.fn().mockResolvedValue([{ a: 1 }]) });
    await ctx.plugin(DataService);
  });

  it("fetches data and caches it", async () => {
    const widget = { datasetId: "ds1" };
    const data1 = await ctx.data.fetchWidgetData(widget);
    expect(data1).toEqual([{ a: 1 }]);

    const data2 = await ctx.data.fetchWidgetData(widget);
    expect(data2).toEqual([{ a: 1 }]);

    expect(ctx.controlPlane.queryData).toHaveBeenCalledTimes(1);
  });

  it("deduplicates in-flight requests", async () => {
    let resolveQuery: (v: any) => void;
    ctx.controlPlane.queryData = vi.fn().mockReturnValue(new Promise((r) => (resolveQuery = r)));

    const widget = { datasetId: "ds2" };
    const p1 = ctx.data.fetchWidgetData(widget);
    const p2 = ctx.data.fetchWidgetData(widget);

    resolveQuery!([{ b: 2 }]);
    const [d1, d2] = await Promise.all([p1, p2]);
    expect(d1).toEqual([{ b: 2 }]);
    expect(d2).toEqual([{ b: 2 }]);
    expect(ctx.controlPlane.queryData).toHaveBeenCalledTimes(1);
  });

  it("binds params", async () => {
    ctx.data.setParam("region", "US");
    await ctx.data.fetchWidgetData({ datasetId: "ds3" });
    expect(ctx.controlPlane.queryData).toHaveBeenCalledWith({
      datasetId: "ds3",
      params: { region: "US" },
    });
  });

  it("invalidates cache", async () => {
    const widget = { datasetId: "ds4" };
    await ctx.data.fetchWidgetData(widget);
    ctx.data.invalidate("ds4");
    await ctx.data.fetchWidgetData(widget);
    expect(ctx.controlPlane.queryData).toHaveBeenCalledTimes(2);
  });
});
