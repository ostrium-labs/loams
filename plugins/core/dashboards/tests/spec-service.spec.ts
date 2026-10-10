import { describe, it, expect, vi, beforeEach } from "vite-plus/test";
import { Context } from "cordis";
import { DashboardSpecService } from "../src/service.js";
import { DashboardSpec, Widget } from "@loams-core/types";
import { v4 as uuidv4 } from "uuid";

describe("DashboardSpecService", () => {
  let ctx: Context;
  let service: DashboardSpecService;
  let storeMap: Map<string, DashboardSpec>;

  beforeEach(() => {
    ctx = new Context();
    storeMap = new Map();

    const mockStore = {
      getDashboard: vi.fn(async (id: string) => {
        if (!storeMap.has(id)) throw new Error("Not found");
        return storeMap.get(id);
      }),
      saveDashboard: vi.fn(async (spec: DashboardSpec) => {
        storeMap.set(spec.id, spec);
      }),
      listDashboards: vi.fn(async () => Array.from(storeMap.values())),
    };

    ctx.provide("store", mockStore);
    service = new DashboardSpecService(ctx);
  });

  it("create() generates valid spec with uuid and version 0", async () => {
    const spec = await service.create("Test Dashboard");
    expect(spec.id).toBeDefined();
    expect(spec.version).toBe(0);
    expect(spec.title).toBe("Test Dashboard");
    expect(storeMap.get(spec.id)).toEqual(spec);
  });

  it("patch() applies ops correctly and bumps version", async () => {
    const spec = await service.create("Test Dashboard");

    const patched = await service.patch(spec.id, 0, [
      { op: "replace", path: "/title", value: "New Title" },
    ]);

    expect(patched.version).toBe(1);
    expect(patched.title).toBe("New Title");
    expect(storeMap.get(spec.id)?.title).toBe("New Title");
  });

  it("patch() rejects on version conflict (baseVersion mismatch)", async () => {
    const spec = await service.create("Test Dashboard");

    await expect(
      service.patch(spec.id, 1, [{ op: "replace", path: "/title", value: "New Title" }]),
    ).rejects.toThrow("Version conflict: expected 1, got 0");
  });

  it("patch() rejects invalid patches (result fails Zod validation)", async () => {
    const spec = await service.create("Test Dashboard");

    await expect(
      service.patch(spec.id, 0, [
        { op: "replace", path: "/title", value: "" }, // min length is 1
      ]),
    ).rejects.toThrow();
  });

  it("addWidget() adds widget and layout item", async () => {
    const spec = await service.create("Test Dashboard");

    const widgetId = uuidv4();
    const widget: Widget = {
      id: widgetId,
      type: "text",
    };

    const patched = await service.addWidget(spec.id, widget);
    expect(patched.widgets[widgetId]).toBeDefined();
    expect(patched.layout.length).toBe(1);
    expect(patched.layout[0].id).toBe(widgetId);
    expect(patched.version).toBe(1);
  });

  it("removeWidget() removes both widget and layout entry", async () => {
    const spec = await service.create("Test Dashboard");
    const widgetId = uuidv4();
    const widget: Widget = {
      id: widgetId,
      type: "text",
    };

    const withWidget = await service.addWidget(spec.id, widget);
    expect(withWidget.layout.length).toBe(1);

    const removed = await service.removeWidget(spec.id, widgetId);
    expect(removed.widgets[widgetId]).toBeUndefined();
    expect(removed.layout.length).toBe(0);
    expect(removed.version).toBe(2);
  });
});
