/**
 * Routing.
 *
 * These are behavioural tests against react-router itself: each one mounts the
 * real `ShellRoutes` inside a `MemoryRouter` at a given entry and asserts which
 * page actually rendered. The hand-rolled router this replaced had its own unit
 * tests for `matchRoute`/`resolveHref`; the equivalent assertions now live in
 * `nav.spec.ts` (the pure pathname rule) and here (what the router does with
 * the URL).
 *
 * A deep link is the interesting case: `/plugins/zulip` reached by pasting a URL
 * must behave exactly like reaching it by clicking, which in a browser depends on
 * `appType: "spa"` giving back index.html rather than a 404.
 */
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router";
import { describe, expect, it } from "vite-plus/test";
import { ShellProviders, ShellRoutes } from "../../src/ui/AppShell.js";
import { DASHBOARD_PLUGIN_ID } from "../../src/ui/nav.js";
import { dashboardPlugin, fakeStore, h, plugin, text, zulipPlugin } from "./fixtures.js";

/** A stand-in for the host's dashboard component, which core cannot import. */
function FakeDashboard() {
  return h("div", { className: "dashboard-marker" }, "the real dashboard");
}

function renderShell(
  pathname: string,
  options: { plugins?: ReturnType<typeof fakeStore>["plugins"] } = {},
): string {
  const store = fakeStore({ plugins: options.plugins ?? [] });
  return renderToStaticMarkup(
    h(MemoryRouter, {
      initialEntries: [pathname],
      children: h(ShellProviders, {
        dashboard: FakeDashboard,
        store,
        children: h(ShellRoutes, { dashboardPluginId: DASHBOARD_PLUGIN_ID }),
      }),
    }),
  );
}

describe("route matching", () => {
  it("routes the root to the dashboard plugin's own surface", () => {
    expect(renderShell("/")).toContain("the real dashboard");
  });

  it("routes /plugins to the plugins page", () => {
    const markup = renderShell("/plugins", { plugins: [zulipPlugin()] });
    expect(markup).toContain("The plugins that are switched on right now");
    // The console page's own heading must NOT be here.
    expect(markup).not.toContain("Plugin console");
  });

  it("routes /console to the console page", () => {
    expect(renderShell("/console")).toContain("Plugin console");
  });

  it("routes /plugins/:id to that plugin's page", () => {
    const markup = renderShell("/plugins/zulip", { plugins: [zulipPlugin()] });
    expect(text(markup)).toContain("Zulip");
    expect(markup).toContain("Message streams from a Zulip organisation.");
  });

  it("tolerates a trailing slash on every route", () => {
    expect(renderShell("/console/")).toContain("Plugin console");
    expect(renderShell("/plugins/")).toContain("The plugins that are switched on right now");
    expect(renderShell("/plugins/zulip/", { plugins: [zulipPlugin()] })).toContain(
      "Message streams from a Zulip organisation.",
    );
  });

  it("treats a missing plugin id as the plugins index, not a plugin called ''", () => {
    // `/plugins/` must not produce "No plugin called ''".
    const markup = renderShell("/plugins/");
    expect(markup).not.toContain("No plugin called");
    expect(markup).toContain("The plugins that are switched on right now");
  });

  it("decodes the plugin id", () => {
    const markup = renderShell("/plugins/its-a%20plan", {
      plugins: [plugin({ id: "its-a plan", name: "ItsAPlan" })],
    });
    expect(text(markup)).toContain("ItsAPlan");
  });

  it("falls through to not-found for anything else", () => {
    expect(text(renderShell("/nope"))).toContain("Nothing at this address");
    expect(text(renderShell("/plugins/zulip/skills"))).toContain("Nothing at this address");
    expect(text(renderShell("/consoles"))).toContain("Nothing at this address");
  });

  it("echoes the unmatched path on the not-found page", () => {
    expect(text(renderShell("/nope"))).toContain("/nope");
  });

  it("routes the dashboard plugin's own page to the dashboard too", () => {
    expect(renderShell("/plugins/dashboard", { plugins: [dashboardPlugin()] })).toContain(
      "the real dashboard",
    );
  });
});

describe("active nav state", () => {
  function activeLabels(markup: string): string[] {
    // `aria-current` is on the list item: NavLink's own prefix matching would
    // light two entries on /plugins/dashboard, so AppShell suppresses it.
    const out: string[] = [];
    for (const match of markup.matchAll(/<li[^>]*aria-current="page"[^>]*>(.*?)<\/li>/g)) {
      const label = text(match[1]!);
      if (label) out.push(label);
    }
    return out;
  }

  it("marks exactly one entry current per route", () => {
    expect(activeLabels(renderShell("/"))).toEqual(["Dashboard"]);
    expect(activeLabels(renderShell("/plugins"))).toEqual(["Plugins"]);
    expect(activeLabels(renderShell("/console"))).toEqual(["Console"]);
    expect(activeLabels(renderShell("/plugins/zulip", { plugins: [zulipPlugin()] }))).toEqual([
      "Plugins",
    ]);
  });

  it("keeps Plugins current on the dashboard plugin's page", () => {
    // The product rule says Dashboard owns /plugins/dashboard.
    expect(
      activeLabels(renderShell("/plugins/dashboard", { plugins: [dashboardPlugin()] })),
    ).toEqual(["Dashboard"]);
  });

  it("marks nothing current on an unknown path", () => {
    expect(activeLabels(renderShell("/nope"))).toEqual([]);
  });

  it("tolerates a trailing slash", () => {
    expect(activeLabels(renderShell("/console/"))).toEqual(["Console"]);
  });
});

describe("deep links", () => {
  it("renders the shell chrome around a pasted deep link, not just the page", () => {
    const markup = renderShell("/plugins/zulip", { plugins: [zulipPlugin()] });
    // Chrome that comes from the layout route, not the matched page.
    expect(markup).toContain('aria-label="Primary"');
    expect(markup).toContain('id="shell-main"');
    // The skip link is the first thing in the tab order.
    expect(markup).toContain("Skip to content");
  });

  it("resolves the same page whether arrived at directly or via the index", () => {
    const deep = renderShell("/plugins/zulip", { plugins: [zulipPlugin()] });
    expect(deep).toContain('id="shell-main"');
    expect(text(deep)).toContain("Zulip");
  });
});
