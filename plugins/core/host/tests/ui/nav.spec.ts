import { describe, expect, it } from "vite-plus/test";
import { DASHBOARD_PLUGIN_ID, NAV_ITEMS, isNavActive } from "../../src/ui/nav.js";

function activeLabels(pathname: string, dashboardPluginId?: string): string[] {
  return NAV_ITEMS.filter((item) => isNavActive(item, pathname, dashboardPluginId)).map(
    (item) => item.label,
  );
}

describe("NAV_ITEMS", () => {
  it("is the Dashboard / Plugins / Console set, in that order", () => {
    expect(NAV_ITEMS.map((item) => item.to)).toEqual(["/", "/plugins", "/console"]);
    expect(NAV_ITEMS.map((item) => item.label)).toEqual(["Dashboard", "Plugins", "Console"]);
  });
});

describe("isNavActive", () => {
  it("lights exactly one entry per route", () => {
    expect(activeLabels("/")).toEqual(["Dashboard"]);
    expect(activeLabels("/plugins")).toEqual(["Plugins"]);
    expect(activeLabels("/console")).toEqual(["Console"]);
  });

  it("keeps Plugins lit on a plugin's own page", () => {
    expect(activeLabels("/plugins/zulip")).toEqual(["Plugins"]);
    expect(activeLabels("/plugins/zulip/")).toEqual(["Plugins"]);
  });

  it("lights Dashboard on the dashboard plugin's page, not on others", () => {
    expect(activeLabels(`/plugins/${DASHBOARD_PLUGIN_ID}`)).toEqual(["Dashboard"]);
    expect(activeLabels("/plugins/zulip")).not.toContain("Dashboard");
  });

  it("honours a host that registers the dashboard under another id", () => {
    expect(activeLabels("/plugins/dashboard", "analytics")).toEqual(["Plugins"]);
    expect(activeLabels("/plugins/analytics", "analytics")).toEqual(["Dashboard"]);
  });

  it("lights nothing on an unknown path", () => {
    expect(activeLabels("/nope")).toEqual([]);
  });

  it("tolerates a trailing slash and a doubled slash", () => {
    expect(activeLabels("/console/")).toEqual(["Console"]);
    expect(activeLabels("/plugins//")).toEqual(["Plugins"]);
    expect(activeLabels("/plugins//zulip")).toEqual(["Plugins"]);
  });

  it("ignores the query string and hash when deciding", () => {
    expect(activeLabels("/console?refresh=1")).toEqual(["Console"]);
    expect(activeLabels("/plugins/zulip#skills")).toEqual(["Plugins"]);
  });

  it("decodes a percent-escaped plugin id", () => {
    expect(activeLabels("/plugins/my%20plugin")).toEqual(["Plugins"]);
  });

  it("falls back to the raw segment when the id is a malformed escape", () => {
    // Not a throw: the page then renders its own "unknown plugin" state.
    expect(activeLabels("/plugins/100%")).toEqual(["Plugins"]);
  });

  it("treats a missing plugin id as the plugins index, not an empty plugin", () => {
    // `/plugins/` must not light Dashboard, which would need id === "dashboard".
    expect(activeLabels("/plugins/")).toEqual(["Plugins"]);
  });
});
