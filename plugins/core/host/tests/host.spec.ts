import { describe, it, expect, beforeEach, afterEach } from "vite-plus/test";
import { Context, Service } from "cordis";
import { createHarness, manifest, type Harness } from "./helpers.js";
import type { PluginRuntime } from "../src/types.js";

/**
 * "Off" means unloaded. These tests are the evidence for that claim, and they
 * are deliberately written against HTTP rather than against internal calls: a
 * handler invoked directly would keep working after its route was removed, so a
 * direct-call test would pass while the server 500s or hangs.
 */

describe("PluginHost load/unload", () => {
  let h: Harness;

  beforeEach(async () => {
    h = await createHarness();
  });

  afterEach(async () => {
    await h.close();
  });

  async function boot() {
    await (h.registry as any)[Service.init]();
    await (h.host as any)[Service.init]();
  }

  it("404s a disabled plugin's route and 200s it again once re-enabled", async () => {
    h.registry.register(manifest({ id: "zulip", defaultEnabled: false }), {
      routes: () => [
        {
          name: "zulip:channels",
          method: "GET",
          match: "/api/plugins/zulip/channels",
          handler: ({ sendJson }) => {
            sendJson(200, { channels: ["general"] });
            return true;
          },
        },
      ],
    });
    await boot();
    expect(h.host.isLoaded("zulip")).toBe(false);

    const before = await h.request("GET", "/api/plugins/zulip/channels");
    expect(before.status).toBe(404);

    await h.host.enable("zulip");
    const enabled = await h.request("GET", "/api/plugins/zulip/channels");
    expect(enabled.status).toBe(200);
    expect(enabled.body).toEqual({ channels: ["general"] });

    await h.host.disable("zulip");
    const after = await h.request("GET", "/api/plugins/zulip/channels");
    expect(after.status).toBe(404);
    expect(h.router.paths()).not.toContain("zulip:channels");
  });

  it("does not duplicate handlers across enable -> disable -> enable", async () => {
    let hits = 0;
    let built = 0;

    h.registry.register(manifest({ id: "zulip", defaultEnabled: false }), {
      attach: (runtime: PluginRuntime) => {
        built += 1;
        void runtime;
        return () => {
          built -= 1;
        };
      },
      routes: () => [
        {
          name: "zulip:ping",
          method: "GET",
          match: "/api/plugins/zulip/ping",
          handler: ({ sendJson }) => {
            hits += 1;
            sendJson(200, { hits });
            return true;
          },
        },
      ],
      skills: () => [
        {
          id: "listChannels",
          handle: () => ({ count: 1 }),
        },
      ],
    });
    await boot();

    await h.host.enable("zulip");
    await h.host.disable("zulip");
    await h.host.enable("zulip");

    const response = await h.request("GET", "/api/plugins/zulip/ping");
    expect(response.status).toBe(200);
    // One increment, not two: a second copy of the same route would be
    // registered under a different id and would answer this request first.
    expect(response.body).toEqual({ hits: 1 });
    expect(built).toBe(1);
    expect(h.router.paths().filter((name) => name === "zulip:ping")).toHaveLength(1);
    expect(h.bus.agents()).toEqual(["zulip"]);
  });

  it("is idempotent: enabling twice loads once, disabling twice is harmless", async () => {
    let built = 0;
    h.registry.register(manifest({ id: "matomo", defaultEnabled: false }), {
      attach: () => {
        built += 1;
        return () => {
          built -= 1;
        };
      },
    });
    await boot();

    const first = await h.host.enable("matomo");
    const second = await h.host.enable("matomo");
    expect(first.state).toBe("loaded");
    expect(second.state).toBe("loaded");
    expect(built).toBe(1);

    const off = await h.host.disable("matomo");
    const offAgain = await h.host.disable("matomo");
    expect(off).toMatchObject({ enabled: false, state: "unloaded" });
    expect(offAgain).toMatchObject({ enabled: false, state: "unloaded" });
    expect(built).toBe(0);
  });

  it("records a loader failure as state error and keeps serving other plugins", async () => {
    h.registry.register(manifest({ id: "broken", defaultEnabled: false }), {
      routes: () => {
        throw new Error("bad manifest wiring");
      },
    });
    h.registry.register(manifest({ id: "healthy", defaultEnabled: false }), {
      routes: () => [
        {
          name: "healthy:ping",
          method: "GET",
          match: "/api/plugins/healthy/ping",
          handler: ({ sendJson }) => {
            sendJson(200, { ok: true });
            return true;
          },
        },
      ],
    });
    await boot();

    const broken = await h.host.enable("broken");
    expect(broken.state).toBe("error");
    expect(broken.error).toContain("bad manifest wiring");
    expect(broken.enabled).toBe(true);

    await h.host.enable("healthy");
    const response = await h.request("GET", "/api/plugins/healthy/ping");
    expect(response.status).toBe(200);
    expect(response.body).toEqual({ ok: true });
    expect(h.host.isLoaded("broken")).toBe(false);
  });

  it("disposes the cordis service so its context key vanishes", async () => {
    class ProbeService extends Service {
      constructor(ctx: Context) {
        super(ctx, "probe");
      }
    }
    h.registry.register(manifest({ id: "probe", defaultEnabled: false }), {
      service: ProbeService,
    });
    await boot();

    await h.host.enable("probe");
    expect((h.ctx as any).probe).toBeDefined();

    await h.host.disable("probe");
    expect((h.ctx as any).probe).toBeUndefined();
  });

  it("unsubscribes skills so an unloaded agent stops answering", async () => {
    h.registry.register(manifest({ id: "zulip", defaultEnabled: false }), {
      skills: () => [{ id: "listChannels", handle: () => ({ channels: ["general"] }) }],
    });
    await boot();

    await h.host.enable("zulip");
    const reply = await h.bus.request({
      from: "test",
      to: "zulip",
      messageId: "m-1",
      skill: "listChannels",
      timestamp: Date.now(),
    });
    expect(reply.data).toEqual({ channels: ["general"] });

    await h.host.disable("zulip");
    await expect(
      h.bus.request({
        from: "test",
        to: "zulip",
        messageId: "m-2",
        skill: "listChannels",
        timestamp: Date.now(),
      }),
    ).rejects.toThrow('agent "zulip" is not loaded');
  });

  it("loads alwaysOn plugins at boot and refuses to unload them", async () => {
    h.registry.register(manifest({ id: "dashboard", alwaysOn: true, order: 0, uiPath: "/" }), {});
    await boot();

    expect(h.registry.get("dashboard")).toMatchObject({
      enabled: true,
      state: "loaded",
    });
    await expect(h.host.disable("dashboard")).rejects.toThrow(/always on/i);
    expect(h.host.isLoaded("dashboard")).toBe(true);
  });

  it("does not load a plugin that was disabled before the restart", async () => {
    h.registry.register(manifest({ id: "zulip" }), {});
    await boot();
    expect(h.host.isLoaded("zulip")).toBe(true);

    await h.host.disable("zulip");
    await h.close();

    // A fresh context over the same store is what a server restart looks like.
    const restarted = await createHarness({ store: h.store });
    restarted.registry.register(manifest({ id: "zulip" }), {});
    await (restarted.registry as any)[Service.init]();
    await (restarted.host as any)[Service.init]();

    expect(restarted.registry.get("zulip").enabled).toBe(false);
    expect(restarted.host.isLoaded("zulip")).toBe(false);
    await restarted.close();
  });

  it("reacts to a registry-level toggle made by something other than the host", async () => {
    h.registry.register(manifest({ id: "zulip", defaultEnabled: false }), {
      routes: () => [
        {
          name: "zulip:ping",
          method: "GET",
          match: "/api/plugins/zulip/ping",
          handler: ({ sendJson }) => {
            sendJson(200, { ok: true });
            return true;
          },
        },
      ],
    });
    await boot();

    await h.registry.enable("zulip");
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(h.host.isLoaded("zulip")).toBe(true);
    expect((await h.request("GET", "/api/plugins/zulip/ping")).status).toBe(200);

    await h.registry.disable("zulip");
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(h.host.isLoaded("zulip")).toBe(false);
    expect((await h.request("GET", "/api/plugins/zulip/ping")).status).toBe(404);
  });

  it("loads on register() for a plugin that resolves to enabled", async () => {
    const status = await h.host.register(manifest({ id: "glitchtip" }), {});
    expect(status.state).toBe("loaded");
    expect(h.host.loadedPlugins()).toContain("glitchtip");
  });
});
