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

describe("data providers", () => {
  let ctx: Context;
  beforeEach(async () => {
    ctx = new Context();
    ctx.provide("controlPlane");
    ctx.set("controlPlane", { queryData: vi.fn().mockResolvedValue([{ a: 1 }]) });
    await ctx.plugin(DataService);
  });

  const loamsWidget = { data: { source: "loams", stream: "operations" } };

  it("routes a widget to the provider naming its source", async () => {
    const query = vi.fn().mockResolvedValue({ data: [{ id: "op1" }], rowcount: 1 });
    ctx.data.registerProvider({ source: "loams", live: true, query });

    const result = await ctx.data.fetchWidgetData(loamsWidget);

    expect(result).toEqual({ data: [{ id: "op1" }], rowcount: 1 });
    expect(query).toHaveBeenCalledTimes(1);
    // The Superset path must not run for a widget that named another source.
    expect(ctx.controlPlane.queryData).not.toHaveBeenCalled();
  });

  it("passes the widget's own params through to the provider", async () => {
    const query = vi.fn().mockResolvedValue({ data: [], rowcount: 0 });
    ctx.data.registerProvider({ source: "loams", live: true, query });
    ctx.data.setParam("region", "US");

    await ctx.data.fetchWidgetData({ ...loamsWidget, params: { project: "loams" } });

    expect(query.mock.calls[0][1]).toEqual({ region: "US", project: "loams" });
  });

  it("never serves a live source from the result cache", async () => {
    // The point of `live`: two reads a moment apart must both reach the source,
    // because a tile labelled live that serves a five-minute-old row is worse
    // than no tile at all.
    let tick = 0;
    const query = vi.fn(async () => ({ data: [{ tick: tick++ }], rowcount: 1 }));
    ctx.data.registerProvider({ source: "loams", live: true, query });

    const first = await ctx.data.fetchWidgetData(loamsWidget);
    const second = await ctx.data.fetchWidgetData(loamsWidget);

    expect(first).toEqual({ data: [{ tick: 0 }], rowcount: 1 });
    expect(second).toEqual({ data: [{ tick: 1 }], rowcount: 1 });
    expect(query).toHaveBeenCalledTimes(2);
  });

  it("does not share one in-flight promise across two live widgets", async () => {
    const query = vi.fn(async () => ({ data: [], rowcount: 0 }));
    ctx.data.registerProvider({ source: "loams", live: true, query });

    await Promise.all([ctx.data.fetchWidgetData(loamsWidget), ctx.data.fetchWidgetData(loamsWidget)]);

    expect(query).toHaveBeenCalledTimes(2);
  });

  it("falls back to Superset for a source nobody registered", async () => {
    // A typo in `source` must reach Superset and fail there, rather than
    // rendering a permanently empty tile that looks like real data.
    const result = await ctx.data.fetchWidgetData({ data: { source: "suprset" } });
    expect(ctx.controlPlane.queryData).toHaveBeenCalledTimes(1);
    expect(result).toEqual([{ a: 1 }]);
  });

  it("still caches a non-live provider", async () => {
    const query = vi.fn().mockResolvedValue({ data: [{ n: 1 }], rowcount: 1 });
    ctx.data.registerProvider({ source: "warehouse", query });

    await ctx.data.fetchWidgetData({ data: { source: "warehouse" } });
    await ctx.data.fetchWidgetData({ data: { source: "warehouse" } });

    expect(query).toHaveBeenCalledTimes(1);
  });

  it("unregisters, and only the registration it handed back", async () => {
    const first = vi.fn().mockResolvedValue({ data: [{ n: 1 }], rowcount: 1 });
    const second = vi.fn().mockResolvedValue({ data: [{ n: 2 }], rowcount: 1 });
    const off = ctx.data.registerProvider({ source: "loams", live: true, query: first });
    ctx.data.registerProvider({ source: "loams", live: true, query: second });

    // A reload replaces the provider; the old unregister must not evict the new
    // one and leave the source unregistered.
    off();
    await ctx.data.fetchWidgetData(loamsWidget);
    expect(second).toHaveBeenCalledTimes(1);

    const offSecond = ctx.data.registerProvider({ source: "loams", live: true, query: second });
    offSecond();
    await ctx.data.fetchWidgetData(loamsWidget);
    expect(ctx.controlPlane.queryData).toHaveBeenCalledTimes(1);
  });
});
