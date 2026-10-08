import { describe, it, expect, beforeEach, afterEach } from "vite-plus/test";
import { Context, Service } from "cordis";
import { createHarness, type Harness } from "../../../core/host/tests/helpers.js";
import { DASHBOARD_MANIFEST, dashboardLoader, mountCoreApi } from "@loams-core/host";
import type { PluginStatus, RouteRequest } from "@loams-core/host";
import { planPluginCatalog, registerPluginCatalog, type Env } from "../src/plugin-catalog.js";

/**
 * The catalog is what makes the adapters reachable from the console, so these
 * tests assert on the exact `GET /api/plugins` body a browser would render --
 * ids, ordering, enabled flags and skill counts -- rather than on the internals
 * that produced it.
 *
 * They run against a fresh environment on purpose. A fresh checkout is the case
 * most likely to be broken: every adapter is missing its credentials, and the
 * difference between "listed and off, here is the variable" and "listed and
 * red" is the difference between a console somebody reads and one they mute.
 */

/** Every adapter's configuration, i.e. a fully provisioned deployment. */
const FULL_ENV: Env = {
  ZULIP_URL: "https://chat.example.com",
  ZULIP_EMAIL: "agent@example.com",
  ZULIP_API_KEY: "zulip-key",
  FORGEJO_URL: "https://git.example.com",
  FORGEJO_TOKEN: "forgejo-pat",
  LANGFUSE_URL: "https://cloud.langfuse.com",
  LANGFUSE_PUBLIC_KEY: "pk-lf-1",
  LANGFUSE_SECRET_KEY: "sk-lf-1",
  OPENPANEL_URL: "https://analytics.example.com",
  OPENPANEL_CLIENT_ID: "3f1c9a2e-4b7d-4c2e-9a51-2f6b8d0c1e44",
  OPENPANEL_CLIENT_SECRET: "openpanel-secret",
  GLITCHTIP_URL: "https://errors.example.com",
  GLITCHTIP_TOKEN: "glitchtip-token",
  MATOMO_URL: "https://matomo.example.com",
  MATOMO_API_TOKEN: "matomo-token",
  ITSAPLAN_URL: "https://plan.example.com",
  ITSAPLAN_API_KEY: "itp_1",
  LOAMS_URL: "http://127.0.0.1:8080",
  LOAMS_NAMESPACE: "demo",
};

/** Console order, as the registry sorts it: always-on, then `order`, then name. */
const FRESH_CHECKOUT_INVENTORY = [
  { id: "dashboard", uiPath: "/", enabled: true, alwaysOn: true, skills: 3 },
  {
    id: "control-plane",
    uiPath: "/plugins/control-plane",
    enabled: true,
    alwaysOn: true,
    skills: 0,
  },
  // `order` 30 is shared by langfuse and zulip, so name breaks the tie.
  { id: "langfuse", uiPath: "/plugins/langfuse", enabled: false, alwaysOn: false, skills: 8 },
  { id: "zulip", uiPath: "/plugins/zulip", enabled: false, alwaysOn: false, skills: 7 },
  { id: "forgejo", uiPath: "/plugins/forgejo", enabled: false, alwaysOn: false, skills: 9 },
  { id: "openpanel", uiPath: "/plugins/openpanel", enabled: false, alwaysOn: false, skills: 9 },
  { id: "glitchtip", uiPath: "/plugins/glitchtip", enabled: false, alwaysOn: false, skills: 9 },
  { id: "matomo", uiPath: "/plugins/matomo", enabled: false, alwaysOn: false, skills: 11 },
  { id: "itsaplan", uiPath: "/plugins/itsaplan", enabled: false, alwaysOn: false, skills: 10 },
  { id: "loams", uiPath: "/plugins/loams", enabled: false, alwaysOn: false, skills: 8 },
];

function inventory(list: PluginStatus[]) {
  return list.map((plugin) => ({
    id: plugin.id,
    uiPath: plugin.uiPath,
    enabled: plugin.enabled,
    alwaysOn: plugin.alwaysOn === true,
    skills: plugin.agent?.skills.length ?? 0,
  }));
}

function statusOf(list: PluginStatus[], id: string): PluginStatus {
  const found = list.find((entry) => entry.id === id);
  if (!found) throw new Error(`no such plugin in list: ${id}`);
  return found;
}

/** Mount the control plane and stand the catalog up the way the server does. */
async function bootWithCatalog(h: Harness, env: Env) {
  mountCoreApi(h.ctx, { baseUrl: () => h.baseUrl });
  // The dashboard is registered first in `apps/server/src/index.ts`, so it is
  // registered first here: the catalog's ordering is asserted against it.
  await h.host.register(DASHBOARD_MANIFEST, dashboardLoader);
  return registerPluginCatalog(h.ctx, env);
}

async function boot(h: Harness) {
  await (h.registry as any)[Service.init]();
  await (h.host as any)[Service.init]();
}

describe("upstream plugin catalog", () => {
  let h: Harness;

  beforeEach(async () => {
    h = await createHarness();
  });

  afterEach(async () => {
    await h.close();
  });

  /* ---------------------------------------------------------------------- */
  /* the catalog on a fresh checkout                                         */
  /* ---------------------------------------------------------------------- */

  it("lists every adapter, in console order, with its uiPath and skill count", async () => {
    await bootWithCatalog(h, {});
    await boot(h);

    const response = await h.request("GET", "/api/plugins");
    expect(response.status).toBe(200);
    expect(inventory(response.body.plugins as PluginStatus[])).toEqual(FRESH_CHECKOUT_INVENTORY);
  });

  it("keeps the always-on plugins first and the rest in `order`", async () => {
    await bootWithCatalog(h, {});
    await boot(h);

    const list = (await h.request("GET", "/api/plugins")).body.plugins as PluginStatus[];
    // The dashboard is pinned to the top: it is the thing every other route
    // lives inside.
    expect(list[0].id).toBe("dashboard");
    const rest = list.slice(list.findIndex((entry) => entry.alwaysOn !== true));
    const orders = rest.map((entry) => entry.order ?? 100);
    expect([...orders].sort((a, b) => a - b)).toEqual(orders);
  });

  it("refuses to disable the dashboard and never unloads it", async () => {
    await bootWithCatalog(h, {});
    await boot(h);

    const response = await h.request("POST", "/api/plugins/dashboard/disable");
    expect(response.status).toBe(409);
    expect(response.body.error).toMatch(/always on/i);
    expect((await h.request("GET", "/api/plugins/dashboard")).body.state).toBe("loaded");
  });

  /* ---------------------------------------------------------------------- */
  /* not-configured is not an error                                          */
  /* ---------------------------------------------------------------------- */

  it("reports an unconfigured adapter as off, naming the variables it needs", async () => {
    await bootWithCatalog(h, {});
    await boot(h);

    const zulip = statusOf((await h.request("GET", "/api/plugins")).body.plugins, "zulip");
    expect(zulip.enabled).toBe(false);
    // NOT `error`: nothing failed. A catalog that opens with nine red rows is a
    // catalog nobody reads.
    expect(zulip.state).toBe("unloaded");
    expect(zulip.error).toContain("not configured");
    expect(zulip.error).toContain("ZULIP_URL");
    expect(zulip.error).toContain("ZULIP_EMAIL");
    expect(zulip.error).toContain("ZULIP_API_KEY");
  });

  it("does not enable an unconfigured adapter whose manifest asks to be on by default", async () => {
    const plan = planPluginCatalog({});
    const zulip = plan.entries.find((entry) => entry.manifest.id === "zulip")!;
    // `zulipManifest.defaultEnabled` is true, which is right for a deployment
    // that has exported its credentials and wrong for one that has not.
    expect(zulip.manifest.defaultEnabled).toBe(false);
    expect(zulip.missing).toEqual(["ZULIP_URL", "ZULIP_EMAIL", "ZULIP_API_KEY"]);
  });

  it("explains, rather than crashes, when an unconfigured adapter is enabled by hand", async () => {
    await bootWithCatalog(h, {});
    await boot(h);

    const response = await h.request("POST", "/api/plugins/zulip/enable");
    // A user pressing the switch deserves the missing variable names, not a
    // service constructor complaining about `undefined`.
    expect(response.body.error).toContain("ZULIP_URL");
    expect(response.body.state).toBe("error");
  });

  /* ---------------------------------------------------------------------- */
  /* auto-enable policy                                                     */
  /* ---------------------------------------------------------------------- */

  it("auto-enables an adapter once its credentials are configured, and attaches its service", async () => {
    await bootWithCatalog(h, FULL_ENV);
    await boot(h);

    const list = (await h.request("GET", "/api/plugins")).body.plugins as PluginStatus[];
    for (const id of ["zulip", "forgejo", "matomo", "itsaplan", "loams"]) {
      const status = statusOf(list, id);
      expect({ id, enabled: status.enabled, state: status.state }).toEqual({
        id,
        enabled: true,
        state: "loaded",
      });
      // Loaded means the cordis service really is attached and its skills are
      // subscribed -- a status saying "loaded" over an empty bus is a lie the
      // console cannot detect.
      expect(h.host.isLoaded(id)).toBe(true);
      expect(h.bus.agents()).toContain(id);
    }
  });

  it("keeps a configured adapter off when its package ships no loader for its skills", async () => {
    await bootWithCatalog(h, FULL_ENV);
    await boot(h);

    const list = (await h.request("GET", "/api/plugins")).body.plugins as PluginStatus[];
    // langfuse/openpanel/glitchtip export a manifest but no `PluginLoader`, so
    // enabling them by default would advertise agent skills with no handlers.
    for (const id of ["langfuse", "openpanel", "glitchtip"]) {
      expect(statusOf(list, id).enabled).toBe(false);
      // Configured, so this is a policy decision and not a missing variable.
      expect(statusOf(list, id).error).toBeUndefined();
    }
  });

  it("passes the environment through to the service config, field for field", () => {
    const plan = planPluginCatalog({ ...FULL_ENV, MATOMO_DEFAULT_PERIOD: "week" });
    const matomo = plan.entries.find((entry) => entry.manifest.id === "matomo")!;
    expect(matomo.loader?.config).toMatchObject({
      baseUrl: "https://matomo.example.com",
      apiToken: "matomo-token",
      defaultPeriod: "week",
    });
    // An unrecognised period is dropped rather than forwarded: Matomo would
    // reject it, and the adapter's own default is safer than a bad guess.
    const dropped = planPluginCatalog({ ...FULL_ENV, MATOMO_DEFAULT_PERIOD: "fortnight" });
    expect(
      dropped.entries.find((entry) => entry.manifest.id === "matomo")!.loader?.config,
    ).not.toHaveProperty("defaultPeriod");
  });

  it("lets a persisted toggle override the auto-enable decision", async () => {
    h.store.state.set("loams", false);
    await bootWithCatalog(h, FULL_ENV);
    await boot(h);

    expect(statusOf((await h.request("GET", "/api/plugins")).body.plugins, "loams").enabled).toBe(
      false,
    );
    expect(h.host.isLoaded("loams")).toBe(false);
  });

  /* ---------------------------------------------------------------------- */
  /* load / unload lifecycle                                                */
  /* ---------------------------------------------------------------------- */

  describe("enable -> disable -> enable", () => {
    // A fixture rather than a real adapter, because no adapter package declares
    // `loader.routes` yet: this is the lifecycle the host promises, exercised
    // through the same `host.register(manifest, loader)` call the catalog makes.
    let constructed = 0;
    let handled = 0;

    class ProbeService extends Service {
      static inject = [];
      constructor(ctx: Context, _config?: unknown) {
        super(ctx, "probe");
        constructed += 1;
      }
    }

    const PROBE = {
      id: "probe",
      name: "Probe",
      description: "Fixture adapter.",
      version: "1.0.0",
      uiPath: "/plugins/probe",
      order: 40,
      defaultEnabled: false,
      agent: {
        name: "Probe Agent",
        description: "Counts requests.",
        version: "1.0.0",
        skills: [{ id: "ping", name: "Ping", description: "Answers pong." }],
      },
    };

    beforeEach(async () => {
      constructed = 0;
      handled = 0;
      await bootWithCatalog(h, {});
      await boot(h);
      await h.host.register(
        PROBE as never,
        {
          service: ProbeService as never,
          skills: () => [{ id: "ping", handle: () => ({ pong: true }) }],
          routes: () => [
            {
              name: "probe:ping",
              method: "GET",
              match: "/probe/ping",
              handler: ({ sendJson }: RouteRequest) => {
                handled += 1;
                sendJson(200, { pong: true });
                return true;
              },
            },
          ],
        } as never,
      );
    });

    it("loads the service and registers its routes on enable", async () => {
      expect((await h.request("GET", "/probe/ping")).status).toBe(404);
      expect(constructed).toBe(0);

      const enabled = await h.request("POST", "/api/plugins/probe/enable");
      expect(enabled.body).toMatchObject({ id: "probe", enabled: true, state: "loaded" });
      expect(constructed).toBe(1);

      const ping = await h.request("GET", "/probe/ping");
      expect(ping.status).toBe(200);
      expect(handled).toBe(1);
    });

    it("unloads the service and lets its routes 404 on disable", async () => {
      await h.request("POST", "/api/plugins/probe/enable");
      const disabled = await h.request("POST", "/api/plugins/probe/disable");
      expect(disabled.body).toMatchObject({ id: "probe", enabled: false, state: "unloaded" });
      expect(constructed).toBe(1);
      expect((await h.request("GET", "/probe/ping")).status).toBe(404);
    });

    it("does not duplicate route handlers across a re-enable", async () => {
      await h.request("POST", "/api/plugins/probe/enable");
      await h.request("POST", "/api/plugins/probe/disable");
      await h.request("POST", "/api/plugins/probe/enable");

      const ping = await h.request("GET", "/probe/ping");
      expect(ping.status).toBe(200);
      // One request, one handler. A leaked registration would answer twice, and
      // the double-write is invisible until the second or third toggle.
      expect(handled).toBe(1);
      expect(constructed).toBe(2);
    });
  });

  /* ---------------------------------------------------------------------- */
  /* A2A                                                                    */
  /* ---------------------------------------------------------------------- */

  it("serves the aggregate card at the canonical path and the legacy alias", async () => {
    await bootWithCatalog(h, FULL_ENV);
    await boot(h);

    const canonical = await h.request("GET", "/.well-known/agent-card.json");
    const legacy = await h.request("GET", "/.well-known/agent.json");
    expect(canonical.status).toBe(200);
    expect(legacy.status).toBe(200);
    // Identical bodies, from one handler: the alias cannot drift.
    expect(legacy.body).toEqual(canonical.body);
    // Only plugins that are LOADED are advertised.
    expect(canonical.body.skills.map((skill: { id: string }) => skill.id)).toContain(
      "zulip.listChannels",
    );
    expect(canonical.body.url).toBe(`${h.baseUrl}/a2a/v1/message:send`);
  });

  it("refuses the card and the skills of a disabled adapter", async () => {
    await bootWithCatalog(h, FULL_ENV);
    await boot(h);

    const before = await h.request("GET", "/.well-known/agent-card/zulip");
    expect(before.status).toBe(200);
    expect(before.body.skills.length).toBeGreaterThan(0);

    await h.request("POST", "/api/plugins/zulip/disable");

    const card = await h.request("GET", "/.well-known/agent-card/zulip");
    expect(card.status).toBe(404);
    expect(card.body.error.code).toBe(-32001);

    const send = await h.request("POST", "/a2a/v1/message:send", {
      message: {
        kind: "message",
        id: "msg-1",
        role: "user",
        parts: [
          { kind: "text", text: "hello" },
          { kind: "data", data: { agent: "zulip", skill: "listChannels", params: {} } },
        ],
      },
    });
    expect(send.status).toBe(404);
    expect(send.body.error.code).toBe(-32001);
    expect(h.bus.agents()).not.toContain("zulip");
  });
});
