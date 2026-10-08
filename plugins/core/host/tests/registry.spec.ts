import { describe, it, expect, beforeEach } from "vite-plus/test";
import { Context } from "cordis";
import { PluginRegistry } from "../src/registry.js";
import { FakePluginStore, manifest, statusOf } from "./helpers.js";

/**
 * The registry is the console's source of truth. Two of its behaviours are
 * load-bearing for everything else in the platform: the ordering (the
 * always-on dashboard must be pinned at the top, not sorted by `order` alone)
 * and the persistence (a toggle that does not survive a restart reads as a
 * broken toggle).
 */

async function makeRegistry(store: FakePluginStore): Promise<PluginRegistry> {
  const ctx = new Context();
  ctx.provide("store", store);
  const registry = new PluginRegistry(ctx);
  await registry.resolveAll();
  return registry;
}

describe("PluginRegistry", () => {
  let store: FakePluginStore;
  let registry: PluginRegistry;

  beforeEach(async () => {
    store = new FakePluginStore();
    registry = await makeRegistry(store);
  });

  it("puts alwaysOn plugins first regardless of order", () => {
    registry.register(manifest({ id: "zulip", order: 1 }));
    registry.register(manifest({ id: "dashboard", order: 0, alwaysOn: true }));
    registry.register(manifest({ id: "matomo", order: 2 }));

    // The dashboard also has the lowest `order`, so ordering it by `order` alone
    // would pass: the case that matters is an alwaysOn plugin that would
    // otherwise sort late.
    registry.register(manifest({ id: "late-but-always", order: 900, alwaysOn: true }));
    registry.register(manifest({ id: "first-but-optional", order: -50 }));

    const ids = registry.list().map((entry) => entry.id);
    expect(ids.slice(0, 2)).toEqual(["dashboard", "late-but-always"]);
    expect(ids.indexOf("dashboard")).toBeLessThan(ids.indexOf("zulip"));
    expect(ids.indexOf("late-but-always")).toBeLessThan(ids.indexOf("first-but-optional"));
    expect(ids).toEqual(["dashboard", "late-but-always", "first-but-optional", "zulip", "matomo"]);
  });

  it("falls back to order ?? 100 then name", () => {
    registry.register(manifest({ id: "b", name: "Bravo" }));
    registry.register(manifest({ id: "a", name: "Alpha" }));
    registry.register(manifest({ id: "early", order: 5 }));

    expect(registry.list().map((entry) => entry.id)).toEqual(["early", "a", "b"]);
  });

  it("refuses to disable an alwaysOn plugin", async () => {
    registry.register(manifest({ id: "dashboard", alwaysOn: true }));
    await expect(registry.disable("dashboard")).rejects.toThrow(/always on/i);
    expect(registry.get("dashboard").enabled).toBe(true);
  });

  it("throws for an unknown plugin", async () => {
    expect(() => registry.get("nope")).toThrow("Unknown plugin: nope");
    await expect(registry.enable("nope")).rejects.toThrow("Unknown plugin: nope");
  });

  it("treats a repeated enable as a no-op, not an error", async () => {
    registry.register(manifest({ id: "zulip" }));
    const first = await registry.enable("zulip");
    const second = await registry.enable("zulip");
    expect(second.enabled).toBe(true);
    expect(second.changedAt).toBe(first.changedAt);
  });

  it("treats a repeated disable as a no-op", async () => {
    registry.register(manifest({ id: "zulip" }));
    await registry.disable("zulip");
    const second = await registry.disable("zulip");
    expect(second.enabled).toBe(false);
  });

  it("emits plugin:changed only on a real transition", async () => {
    // Registered off, so the first `enable` is a real transition; a plugin
    // registered on is already enabled and its first `enable` is a no-op.
    registry.register(manifest({ id: "zulip", defaultEnabled: false }));
    const seen: { id: string; enabled: boolean }[] = [];
    // `Service.ctx` is protected, so subscribe through the context the harness
    // built the registry on. Cordis events are per-context, not per-service.
    const events = (registry as unknown as { ctx: Context }).ctx;
    events.on("plugin:changed", (change) => seen.push(change));

    await registry.enable("zulip");
    await registry.enable("zulip");
    await registry.disable("zulip");

    expect(seen).toEqual([
      { id: "zulip", enabled: true },
      { id: "zulip", enabled: false },
    ]);
  });

  it("uses defaultEnabled only for a plugin that has never been toggled", async () => {
    const fresh = await makeRegistry(store);
    fresh.register(manifest({ id: "off-by-default", defaultEnabled: false }));
    await fresh.resolveAll();
    expect(fresh.get("off-by-default").enabled).toBe(false);

    await fresh.enable("off-by-default");

    // Restart over the same store: the explicit enable must win over the
    // manifest default, which is the whole point of persisting.
    const restarted = await makeRegistry(store);
    restarted.register(manifest({ id: "off-by-default", defaultEnabled: false }));
    await restarted.resolveAll();
    expect(restarted.get("off-by-default").enabled).toBe(true);
  });

  it("brings a disabled plugin back disabled after a fresh registry", async () => {
    registry.register(manifest({ id: "zulip", defaultEnabled: true }));
    await registry.disable("zulip");
    expect(store.state.get("zulip")).toBe(false);

    const restarted = await makeRegistry(store);
    restarted.register(manifest({ id: "zulip", defaultEnabled: true }));
    await restarted.resolveAll();
    expect(statusOf(restarted.list(), "zulip").enabled).toBe(false);
  });

  it("never restores a disabled flag for an alwaysOn plugin", async () => {
    store.state.set("dashboard", false);
    const fresh = await makeRegistry(store);
    fresh.register(manifest({ id: "dashboard", alwaysOn: true }));
    await fresh.resolveAll();
    expect(fresh.get("dashboard").enabled).toBe(true);
  });

  it("degrades to in-memory when no store is loaded", async () => {
    const ctx = new Context();
    const bare = new PluginRegistry(ctx);
    await bare.resolveAll();
    bare.register(manifest({ id: "zulip" }));
    await expect(bare.enable("zulip")).resolves.toMatchObject({ enabled: true });
    await bare.disable("zulip");
    expect(bare.get("zulip").enabled).toBe(false);
  });

  it("persists through an explicitly injected store, as the server supplies it", async () => {
    // apps/server builds the registry on a cordis fiber whose context cannot
    // resolve `store` without an `inject`, so it passes the store in through
    // `config.store` instead. That path is the one production uses; the fallback
    // above only runs in tests.
    const ctx = new Context();
    const store = new FakePluginStore();
    const wired = new PluginRegistry(ctx, { store });
    await wired.resolveAll();

    wired.register(manifest({ id: "zulip", defaultEnabled: true }));
    await wired.resolveAll();
    await wired.disable("zulip");

    expect(store.state.get("zulip")).toBe(false);

    const restarted = new PluginRegistry(new Context(), { store });
    await restarted.resolveAll();
    restarted.register(manifest({ id: "zulip", defaultEnabled: true }));
    await restarted.resolveAll();
    expect(restarted.get("zulip").enabled).toBe(false);
  });

  it("reports load state written by the host", () => {
    registry.register(manifest({ id: "zulip" }));
    expect(registry.get("zulip").state).toBe("unloaded");
    registry.markLoaded("zulip");
    expect(registry.get("zulip").state).toBe("loaded");
    registry.markError("zulip", "boom");
    expect(registry.get("zulip")).toMatchObject({ state: "error", error: "boom" });
    registry.markUnloaded("zulip");
    expect(registry.get("zulip").error).toBeUndefined();
  });

  it("rejects a duplicate registration", () => {
    registry.register(manifest({ id: "zulip" }));
    expect(() => registry.register(manifest({ id: "zulip" }))).toThrow(
      "Plugin already registered: zulip",
    );
  });

  it("reports a plugin that is merely unconfigured as off, not as failed", () => {
    registry.register(manifest({ id: "zulip", defaultEnabled: false }));
    registry.markNotConfigured("zulip", "Zulip is not configured: set ZULIP_URL to enable it.");

    expect(registry.get("zulip")).toMatchObject({
      enabled: false,
      // Not `error`: nothing was loaded and nothing failed, and a fresh
      // checkout is a supported configuration rather than a broken one.
      state: "unloaded",
      error: "Zulip is not configured: set ZULIP_URL to enable it.",
    });
    expect(registry.list()[0].error).toContain("ZULIP_URL");

    // Loading clears it: a plugin that has since loaded is no longer waiting for
    // configuration, and leaving the message would contradict its own state.
    registry.markLoaded("zulip");
    expect(registry.get("zulip").error).toBeUndefined();
  });
});
