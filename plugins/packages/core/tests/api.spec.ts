import { describe, it, expect, beforeEach, afterEach } from "vite-plus/test";
import { Service } from "cordis";
import { createHarness, manifest, type Harness } from "./helpers.js";
import { mountCoreApi } from "../src/api.js";
import type { PluginStatus } from "../src/types.js";

/**
 * The REST control plane is what the console actually talks to, so these tests
 * assert on the exact body the UI parses: a `PluginStatus`, not a bare `{ok:true}`.
 * A toggle endpoint that answered "fine" while the plugin stayed unloaded would
 * leave the switch lying, which is the failure the UI's optimistic-update
 * rollback exists to catch -- and it can only catch it if the server reports the
 * real state.
 */

function plugin(id: string, overrides: Record<string, unknown> = {}) {
  return manifest({ id, defaultEnabled: false, ...overrides } as any) as any;
}

async function boot(h: Harness) {
  await (h.registry as any)[Service.init]();
  await (h.host as any)[Service.init]();
}

/** Register the control plane the way the server does. */
function mount(h: Harness) {
  return mountCoreApi(h.ctx, { baseUrl: () => h.baseUrl });
}

describe("mountCoreApi", () => {
  let h: Harness;

  beforeEach(async () => {
    h = await createHarness();
  });

  afterEach(async () => {
    await h.close();
  });

  /* ---------------------------------------------------------------------- */
  /* read                                                                    */
  /* ---------------------------------------------------------------------- */

  it("GET /api/plugins returns the list in console order", async () => {
    h.registry.register(plugin("zulip", { order: 1 }));
    h.registry.register(plugin("dashboard", { alwaysOn: true, order: 0 }));
    h.registry.register(plugin("matomo", { order: 2 }));
    mount(h);
    await boot(h);

    const response = await h.request("GET", "/api/plugins");
    expect(response.status).toBe(200);
    expect(Array.isArray(response.body.plugins)).toBe(true);
    expect(response.body.plugins.map((entry: PluginStatus) => entry.id)).toEqual([
      "dashboard",
      "zulip",
      "matomo",
    ]);
  });

  it("GET /api/plugins/:id returns one status, or 404 for an unknown id", async () => {
    h.registry.register(plugin("zulip"));
    mount(h);
    await boot(h);

    const found = await h.request("GET", "/api/plugins/zulip");
    expect(found.status).toBe(200);
    expect(found.body).toMatchObject({ id: "zulip", enabled: false, state: "unloaded" });

    const missing = await h.request("GET", "/api/plugins/ghost");
    expect(missing.status).toBe(404);
    expect(missing.body.error).toContain("Unknown plugin");
  });

  it("GET /api/plugins/:id/agent serves a card, and 404s a plugin with no agent", async () => {
    h.registry.register({
      ...plugin("zulip"),
      agent: {
        name: "Zulip Agent",
        description: "Reads Zulip.",
        version: "1.0.0",
        skills: [{ id: "listChannels", name: "List channels", description: "d" }],
      },
    });
    h.registry.register(plugin("plain"));
    mount(h);
    await boot(h);

    const card = await h.request("GET", "/api/plugins/zulip/agent");
    expect(card.status).toBe(200);
    expect(card.body.skills.map((skill: any) => skill.id)).toEqual(["listChannels"]);
    expect(card.body.url).toBe(`${h.baseUrl}/a2a/v1/message:send`);

    const none = await h.request("GET", "/api/plugins/plain/agent");
    expect(none.status).toBe(404);
    expect(none.body.error).toContain("no agent");
  });

  /* ---------------------------------------------------------------------- */
  /* write                                                                   */
  /* ---------------------------------------------------------------------- */

  it("POST /api/plugins/:id/enable returns the loaded status", async () => {
    h.registry.register(plugin("zulip", { defaultEnabled: false }), {
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
    mount(h);
    await boot(h);

    const response = await h.request("POST", "/api/plugins/zulip/enable");
    expect(response.status).toBe(200);
    expect(response.body).toMatchObject({ id: "zulip", enabled: true, state: "loaded" });
    expect(response.body.error).toBeUndefined();
    expect(h.store.state.get("zulip")).toBe(true);

    // The enable endpoint claims the plugin is loaded, so its route must actually
    // be serving. A status saying "loaded" over a 404 route would be a lie the
    // console has no way to detect.
    expect((await h.request("GET", "/api/plugins/zulip/ping")).status).toBe(200);
  });

  it("POST /api/plugins/:id/disable returns the unloaded status and removes the route", async () => {
    h.registry.register(plugin("zulip", { defaultEnabled: true }), {
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
    mount(h);
    await boot(h);
    expect((await h.request("GET", "/api/plugins/zulip/ping")).status).toBe(200);

    const response = await h.request("POST", "/api/plugins/zulip/disable");
    expect(response.status).toBe(200);
    expect(response.body).toMatchObject({ id: "zulip", enabled: false, state: "unloaded" });
    expect(h.store.state.get("zulip")).toBe(false);

    expect((await h.request("GET", "/api/plugins/zulip/ping")).status).toBe(404);
  });

  it("reports a failed load as a 200 with state error, not as a 500", async () => {
    h.registry.register(plugin("broken", { defaultEnabled: false }), {
      routes: () => {
        throw new Error("loader exploded");
      },
    });
    mount(h);
    await boot(h);

    const response = await h.request("POST", "/api/plugins/broken/enable");
    // A 500 here would read in the console as "the API is broken" rather than
    // "this one plugin is broken", which is the entire point of `state: "error"`.
    expect(response.status).toBe(200);
    expect(response.body).toMatchObject({ id: "broken", enabled: true, state: "error" });
    expect(response.body.error).toContain("loader exploded");
  });

  it("is idempotent: a second enable does not change changedAt", async () => {
    h.registry.register(plugin("zulip", { defaultEnabled: false }));
    mount(h);
    await boot(h);

    const first = await h.request("POST", "/api/plugins/zulip/enable");
    const second = await h.request("POST", "/api/plugins/zulip/enable");
    expect(second.status).toBe(200);
    expect(second.body.changedAt).toBe(first.body.changedAt);
  });

  it("refuses to disable an alwaysOn plugin with 409, not 500", async () => {
    h.registry.register(plugin("dashboard", { alwaysOn: true, order: 0 }));
    mount(h);
    await boot(h);

    const response = await h.request("POST", "/api/plugins/dashboard/disable");
    expect(response.status).toBe(409);
    expect(response.body.error).toMatch(/always on/i);
    // Still up: the refusal must not have unloaded anything on the way out.
    expect((await h.request("GET", "/api/plugins/dashboard")).body.state).toBe("loaded");
  });

  it("404s an enable or disable for an unknown plugin", async () => {
    mount(h);
    await boot(h);

    expect((await h.request("POST", "/api/plugins/ghost/enable")).status).toBe(404);
    expect((await h.request("POST", "/api/plugins/ghost/disable")).status).toBe(404);
  });

  /* ---------------------------------------------------------------------- */
  /* A2A co-mount                                                            */
  /* ---------------------------------------------------------------------- */

  it("mounts the A2A surface alongside the plugin routes by default", async () => {
    h.registry.register(plugin("zulip"));
    mount(h);
    await boot(h);

    expect((await h.request("GET", "/.well-known/agent-card.json")).status).toBe(200);
    // The pre-1.0 spelling stays served.
    expect((await h.request("GET", "/.well-known/agent.json")).status).toBe(200);
    expect((await h.request("POST", "/a2a/v1/message:send", { agent: "zulip" })).status).toBe(400);
  });

  it("withA2A: false leaves the A2A surface unmounted", async () => {
    h.registry.register(plugin("zulip"));
    mountCoreApi(h.ctx, { baseUrl: () => h.baseUrl, withA2A: false });
    await boot(h);

    expect((await h.request("GET", "/api/plugins")).status).toBe(200);
    expect((await h.request("GET", "/.well-known/agent-card.json")).status).toBe(404);
    expect((await h.request("GET", "/.well-known/agent.json")).status).toBe(404);
  });

  it("unregisters every route it added when the mount is disposed", async () => {
    h.registry.register(plugin("zulip"));
    const remove = mount(h);
    await boot(h);

    expect((await h.request("GET", "/api/plugins")).status).toBe(200);
    remove();
    expect((await h.request("GET", "/api/plugins")).status).toBe(404);
    expect((await h.request("GET", "/.well-known/agent.json")).status).toBe(404);
  });

  it("survives a plugin route that throws without taking the control plane with it", async () => {
    h.registry.register(plugin("zulip", { defaultEnabled: false }), {
      routes: () => [
        {
          name: "zulip:boom",
          method: "GET",
          match: "/api/plugins/zulip/boom",
          handler: () => {
            throw new Error("handler bug");
          },
        },
      ],
    });
    mount(h);
    await boot(h);
    await h.host.enable("zulip");

    const boom = await h.request("GET", "/api/plugins/zulip/boom");
    expect(boom.status).toBe(500);
    expect(boom.body.error).toContain("handler bug");

    expect((await h.request("GET", "/api/plugins")).status).toBe(200);
  });
});
