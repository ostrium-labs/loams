import { describe, expect, it } from "vite-plus/test";
import { ApiError } from "../../src/ui/api.js";
import {
  ALWAYS_ON_REASON,
  countSkills,
  describeState,
  enabledPlugins,
  findPlugin,
  isAlwaysOn,
  messageOf,
  optimisticToggle,
  partitionPlugins,
  pluginHref,
} from "../../src/ui/pluginState.js";
import { dashboardPlugin, plugin, zulipPlugin } from "./fixtures.js";

describe("isAlwaysOn", () => {
  it("is true only when the manifest says so", () => {
    expect(isAlwaysOn(dashboardPlugin())).toBe(true);
    expect(isAlwaysOn(zulipPlugin())).toBe(false);
  });
});

describe("pluginHref", () => {
  it("uses the manifest uiPath", () => {
    expect(pluginHref(zulipPlugin())).toBe("/plugins/zulip");
  });

  it("tolerates a trailing slash", () => {
    expect(pluginHref(plugin({ id: "glitchtip", uiPath: "/plugins/glitchtip/" }))).toBe(
      "/plugins/glitchtip",
    );
  });

  it("tolerates a missing leading slash", () => {
    expect(pluginHref(plugin({ id: "matomo", uiPath: "plugins/matomo" }))).toBe("/plugins/matomo");
  });

  it("falls back to the id when uiPath is blank, root, or missing", () => {
    expect(pluginHref(plugin({ id: "langfuse", uiPath: "" }))).toBe("/plugins/langfuse");
    expect(pluginHref(plugin({ id: "langfuse", uiPath: "   " }))).toBe("/plugins/langfuse");
    expect(pluginHref(plugin({ id: "langfuse", uiPath: "/" }))).toBe("/plugins/langfuse");
    expect(pluginHref(plugin({ id: "its a plan", uiPath: "" }))).toBe("/plugins/its%20a%20plan");
  });
});

describe("describeState", () => {
  it("labels every state the contract allows", () => {
    expect(describeState(plugin({ id: "a", state: "loaded" }))).toBe("Loaded");
    expect(describeState(plugin({ id: "a", state: "unloaded" }))).toBe("Not loaded");
    expect(describeState(plugin({ id: "a", state: "error" }))).toBe("Failed to load");
  });
});

describe("countSkills", () => {
  it("counts the agent's skills, or zero without an agent", () => {
    expect(countSkills(zulipPlugin())).toBe(2);
    expect(countSkills(plugin({ id: "a" }))).toBe(0);
  });
});

describe("findPlugin", () => {
  it("resolves an id, and is null for a missing or empty one", () => {
    const list = [dashboardPlugin(), zulipPlugin()];
    expect(findPlugin(list, "zulip")?.name).toBe("Zulip");
    expect(findPlugin(list, "forgejo")).toBeNull();
    expect(findPlugin(list, undefined)).toBeNull();
    expect(findPlugin(list, "")).toBeNull();
  });
});

describe("partitionPlugins", () => {
  it("splits the pinned always-on plugin out and keeps the server's order", () => {
    const list = [dashboardPlugin(), zulipPlugin(), plugin({ id: "matomo" })];
    const { pinned, rest } = partitionPlugins(list);
    expect(pinned.map((p) => p.id)).toEqual(["dashboard"]);
    expect(rest.map((p) => p.id)).toEqual(["zulip", "matomo"]);
  });

  it("does not re-sort: a deliberately unsorted list comes back unsorted", () => {
    const list = [zulipPlugin(), dashboardPlugin(), plugin({ id: "matomo" })];
    const { pinned, rest } = partitionPlugins(list);
    expect(rest.map((p) => p.id)).toEqual(["zulip", "matomo"]);
  });
});

describe("enabledPlugins", () => {
  it("keeps only what is on, in order", () => {
    const list = [
      dashboardPlugin(),
      zulipPlugin({ enabled: false }),
      plugin({ id: "matomo", enabled: true }),
    ];
    expect(enabledPlugins(list).map((p) => p.id)).toEqual(["dashboard", "matomo"]);
  });
});

describe("optimisticToggle", () => {
  const list = [dashboardPlugin(), zulipPlugin({ enabled: false, state: "unloaded" })];

  it("flips the intent locally and clears any stale error", () => {
    const next = optimisticToggle(list, "zulip", true);
    expect(next[1]).toMatchObject({ enabled: true, state: "loaded", error: undefined });
    expect(list[1]?.enabled).toBe(false);
  });

  it("turning off drops back to unloaded", () => {
    // The switch and the row label would otherwise contradict each other for the
    // duration of the request: "Off" next to "Loaded".
    const on = [dashboardPlugin(), zulipPlugin({ enabled: true, state: "loaded" })];
    expect(optimisticToggle(on, "zulip", false)[1]).toMatchObject({
      enabled: false,
      state: "unloaded",
    });
  });

  it("leaves every other entry the very same object", () => {
    const matomo = plugin({ id: "matomo", enabled: true });
    const next = optimisticToggle([...list, matomo], "zulip", true);
    // Identity, not just equality: the rows are memo-friendly on purpose, and a
    // copy per toggle would re-render every row on the page.
    expect(next[0]).toBe(list[0]);
    expect(next[2]).toBe(matomo);
  });
});

describe("messageOf", () => {
  it("prefers a real message and falls back to something readable", () => {
    expect(messageOf(new ApiError("nope", 409, "/api/plugins/zulip/disable"))).toBe("nope");
    expect(messageOf(new Error("boom"))).toBe("boom");
    expect(messageOf("plain")).toBe("plain");
    expect(messageOf(undefined)).toBe("Something went wrong.");
  });
});

describe("ALWAYS_ON_REASON", () => {
  it("is a sentence, because it is shown next to the locked switch", () => {
    // The console renders this verbatim as the row's description, so it has to
    // read as prose rather than as an error code.
    expect(ALWAYS_ON_REASON).toMatch(/^Always on — .+/);
  });
});
