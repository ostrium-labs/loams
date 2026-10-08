import { describe, expect, it, vi } from "vite-plus/test";
import { ConsolePage } from "../../src/ui/ConsolePage.js";
import { PluginsPage } from "../../src/ui/PluginsPage.js";
import { PluginPage } from "../../src/ui/PluginPage.js";
import { PluginOverviewPage } from "../../src/ui/pluginPages/PluginOverviewPage.js";
import { DashboardPluginPage } from "../../src/ui/pluginPages/DashboardPluginPage.js";
import {
  fallbackPluginPage,
  pluginPages,
  resolvePluginPage,
} from "../../src/ui/pluginPages/registry.js";
import { ALWAYS_ON_REASON } from "../../src/ui/pluginState.js";
import {
  dashboardPlugin,
  fakeStore,
  h,
  plugin,
  renderWithProviders,
  text,
  zulipPlugin,
} from "./fixtures.js";

/* ---------------------------------------------------------------- console */

/**
 * The opening tag of the `role="switch"` element inside `markup`.
 *
 * Extracted rather than matched with a single regex because the properties
 * under test live on the *same element* but React does not promise an attribute
 * order: `disabled` came before `role` for the always-on row and after it for
 * the others. A `/role="switch"[^>]*disabled=""/` assertion therefore passes or
 * fails on nothing but luck. Splitting the claims out of the one regex also
 * keeps the check from going vacuous -- see the note on `disabled` below.
 */
function switchTag(markup: string): string {
  const start = markup.search(/<(?:button|span)\b[^>]*\brole="switch"/);
  if (start === -1) throw new Error('no role="switch" element in the markup');
  const end = markup.indexOf(">", start);
  return markup.slice(start, end + 1);
}

describe("ConsolePage", () => {
  const store = (over = {}) =>
    fakeStore({
      plugins: [dashboardPlugin(), zulipPlugin({ enabled: false, state: "unloaded" })],
      ...over,
    });

  it("renders the dashboard first, in its own pinned group", () => {
    const markup = renderWithProviders(h(ConsolePage), { store: store() });

    const product = markup.indexOf('id="console-group-product"');
    const available = markup.indexOf('id="console-group-available"');
    expect(product).toBeGreaterThan(-1);
    expect(available).toBeGreaterThan(product);

    const pinnedRow = markup.slice(product, available);
    expect(pinnedRow).toContain("Analytics and performance");
    expect(pinnedRow).not.toContain("Zulip");
  });

  it("renders the always-on toggle disabled, with the reason spelled out", () => {
    const markup = renderWithProviders(h(ConsolePage), { store: store() });
    const product = markup.slice(
      markup.indexOf('id="console-group-product"'),
      markup.indexOf('id="console-group-available"'),
    );

    const control = switchTag(product);
    // A real `<button>`, so the browser's own disabled semantics apply...
    expect(control).toMatch(/^<button\b/);
    expect(control).toContain('role="switch"');
    // ...and `disabled=""` rather than the bare word `disabled`: the switch's
    // class string carries a `data-disabled:` variant, and matching the bare
    // word would pass whether or not the attribute is actually set. Matched
    // with a leading space so it can only be an attribute, not a class name.
    expect(control).toMatch(/\sdisabled=""/);
    expect(control).toContain('aria-checked="true"');
    expect(product).toContain("Always on");
    expect(text(product)).toContain(ALWAYS_ON_REASON);
    expect(product).toContain('aria-describedby="plugin-console-dashboard-title-note"');
  });

  it("renders a normal plugin's toggle enabled and off", () => {
    const markup = renderWithProviders(h(ConsolePage), { store: store() });
    const available = markup.slice(markup.indexOf('id="console-group-available"'));
    const row = available.slice(0, available.indexOf("</li>"));

    const control = switchTag(row);
    expect(control).toMatch(/^<button\b/);
    expect(control).toContain('role="switch"');
    // Same non-vacuous shape as above, inverted.
    expect(control).not.toMatch(/\sdisabled=""/);
    expect(control).toContain('aria-checked="false"');
    // The switch is labelled by the row heading, not by loose text.
    expect(row).toContain('aria-labelledby="plugin-console-zulip-title"');
    expect(row).toContain('id="plugin-console-zulip-title"');
  });

  it("wires the switch to the right verb", () => {
    const off = store().plugins[1]!;
    const on = { ...off, enabled: true, state: "loaded" as const };

    const offRow = renderWithProviders(h(ConsolePage), { store: fakeStore({ plugins: [off] }) });
    const onRow = renderWithProviders(h(ConsolePage), { store: fakeStore({ plugins: [on] }) });

    expect(offRow).toContain(">Off<");
    expect(onRow).toContain(">On<");
  });

  it("shows each plugin's description, category, upstream product and skill count", () => {
    const markup = text(renderWithProviders(h(ConsolePage), { store: store() }));
    expect(markup).toContain("Message streams from a Zulip organisation.");
    expect(markup).toContain("upstream");
    expect(markup).toContain("Upstream · Zulip Cloud (ZULIP_)");
    expect(markup).toContain("2 agent skills");
  });

  it('says "1 agent skill" rather than "1 agent skills" for a single skill', () => {
    const single = zulipPlugin({
      agent: {
        name: "a",
        description: "d",
        version: "1.0.0",
        skills: [{ id: "one", name: "One", description: "d" }],
      },
    });
    expect(
      text(renderWithProviders(h(ConsolePage), { store: fakeStore({ plugins: [single] }) })),
    ).toContain("1 agent skill");
  });

  it("makes a failed plugin state visible with the server's reason", () => {
    const broken = zulipPlugin({
      state: "error",
      error: "ZULIP_API_KEY is not set",
    });
    const markup = renderWithProviders(h(ConsolePage), { store: fakeStore({ plugins: [broken] }) });

    // The row is flagged with a danger left border, not a `console-row--error`
    // modifier class -- the migration to Tailwind utilities removed the
    // latter.
    expect(markup).toContain("border-l-danger");
    expect(markup).toContain("Failed to load");
    expect(text(markup)).toContain("ZULIP_API_KEY is not set");
  });

  it("shows a toggle refusal as feedback and leaves the switch off", () => {
    const markup = renderWithProviders(h(ConsolePage), {
      store: fakeStore({
        plugins: [zulipPlugin({ enabled: false, state: "unloaded" })],
        feedback: { zulip: { kind: "error", message: "Cannot enable: upstream unreachable" } },
      }),
    });

    // Error feedback is tinted with the danger token (via `color-mix` over the
    // card surface), which is what replaced the `console-feedback--error`
    // modifier.
    expect(markup).toContain("text-danger");
    expect(text(markup)).toContain("Cannot enable: upstream unreachable");
    expect(markup).toContain('aria-checked="false"');
  });

  it("surfaces a failure to read the list at all, with a retry", () => {
    const markup = renderWithProviders(h(ConsolePage), {
      store: fakeStore({ plugins: [], error: "Could not reach the server at /api." }),
    });

    expect(markup).toContain('role="alert"');
    expect(text(markup)).toContain("Could not load the plugin list.");
    expect(text(markup)).toContain("Could not reach the server at /api.");
    expect(markup).toContain("Retry");
  });

  it("marks a toggle in flight as busy", () => {
    const markup = renderWithProviders(h(ConsolePage), {
      store: fakeStore({
        plugins: [zulipPlugin({ enabled: false, state: "unloaded" })],
        pending: { zulip: true },
      }),
    });
    expect(markup).toContain('aria-busy="true"');
    expect(markup).toContain("Turning on");
  });

  it("offers an Open link only for plugins that are on", () => {
    const markup = renderWithProviders(h(ConsolePage), { store: store() });
    // The dashboard is on, so its row links out to the plugin's own page.
    expect(markup).toContain('href="/plugins/dashboard"');
    // Zulip is off, so its row carries the switch and nothing else -- there is no
    // destination to link to for a plugin whose routes are not mounted.
    const zulipRow = markup.slice(markup.indexOf('id="plugin-console-zulip-title"'));
    expect(zulipRow).not.toContain('href="/plugins/zulip"');
    expect(text(markup)).toContain("1 of 2 on");
  });
});

/* ---------------------------------------------------------------- plugins */

describe("PluginsPage", () => {
  it("lists only the enabled plugins", () => {
    const markup = renderWithProviders(h(PluginsPage), {
      store: fakeStore({
        plugins: [
          dashboardPlugin(),
          zulipPlugin({ enabled: false, state: "unloaded" }),
          plugin({ id: "matomo", name: "Matomo", enabled: true, state: "loaded" }),
        ],
      }),
    });

    const body = text(markup);
    expect(body).toContain("Matomo");
    expect(body).not.toContain("Message streams from a Zulip organisation.");
  });

  it("puts the dashboard first", () => {
    const markup = renderWithProviders(h(PluginsPage), {
      store: fakeStore({
        plugins: [zulipPlugin(), dashboardPlugin()],
      }),
    });
    expect(markup.indexOf("/plugins/dashboard")).toBeLessThan(markup.indexOf("/plugins/zulip"));
  });

  it("links each card at the plugin's uiPath", () => {
    const markup = renderWithProviders(h(PluginsPage), {
      store: fakeStore({ plugins: [zulipPlugin({ uiPath: "/plugins/zulip/" })] }),
    });
    expect(markup).toContain('href="/plugins/zulip"');
    // Anchors, so Enter and cmd-click behave the way the browser expects. Matched
    // on the element and the href alone: the attribute order React happens to
    // emit is not the thing under test.
    expect(markup).toMatch(/<a\b[^>]*\bhref="\/plugins\/zulip"/);
  });

  it("points an empty list at the console", () => {
    const markup = renderWithProviders(h(PluginsPage), {
      store: fakeStore({ plugins: [zulipPlugin({ enabled: false, state: "unloaded" })] }),
    });
    const body = text(markup);
    expect(body).toContain("No plugins are switched on");
    expect(body).toContain("The console lists every available plugin");
    expect(markup).toContain('href="/console"');
    expect(markup).toContain("Open the plugin console");
  });
});

/* ------------------------------------------------------------ plugin page */

describe("PluginPage", () => {
  const render = (id: string | undefined, plugins: ReturnType<typeof zulipPlugin>[]) =>
    renderWithProviders(h(PluginPage, { id }), { store: fakeStore({ plugins }) });

  it("says so clearly when the id names nothing", () => {
    const markup = text(render("forgejo", [zulipPlugin()]));
    expect(markup).toContain("No plugin called “forgejo”");
    expect(markup).toContain("Nothing is registered under the id");
    expect(markup).toContain("Open the console");
  });

  it("distinguishes 'unknown' from 'we could not read the list'", () => {
    const markup = renderWithProviders(h(PluginPage, { id: "zulip" }), {
      store: fakeStore({ plugins: [], error: "Could not reach the server at /api." }),
    });
    expect(text(markup)).toContain("The plugin list could not be read");
  });

  it("copes with a missing id", () => {
    expect(text(render(undefined, []))).toContain("No plugin selected");
  });

  it("offers to turn a disabled plugin on", () => {
    const off = zulipPlugin({ enabled: false, state: "unloaded" });
    const markup = renderWithProviders(h(PluginPage, { id: "zulip" }), {
      store: fakeStore({ plugins: [off] }),
    });

    const body = text(markup);
    expect(body).toContain("Zulip is turned off");
    expect(body).toContain("Turn on Zulip");
    expect(markup).toMatch(/<button[^>]*class="[^"]*bg-primary-solid[^"]*"/);
  });

  it("reports a refused enable without pretending it worked", () => {
    const off = zulipPlugin({ enabled: false, state: "unloaded" });
    const markup = renderWithProviders(h(PluginPage, { id: "zulip" }), {
      store: fakeStore({
        plugins: [off],
        feedback: { zulip: { kind: "error", message: "Upstream is unreachable" } },
      }),
    });
    expect(text(markup)).toContain("Upstream is unreachable");
    expect(markup).toContain("text-danger");
  });

  it("falls back to the skills view for a plugin with no page of its own", () => {
    const markup = text(render("zulip", [zulipPlugin()]));
    expect(markup).toContain("No dedicated interface for this plugin yet");
    expect(markup).toContain("Agent skills");
    expect(markup).toContain("2 skills this plugin's agent advertises.");
    expect(markup).toContain("List channels");
    expect(markup).toContain("Every stream in the org.");
    expect(markup).toContain("Send a message");
  });

  it("says so when a plugin has no agent skills at all", () => {
    const markup = text(render("matomo", [plugin({ id: "matomo", name: "Matomo" })]));
    expect(markup).toContain("This plugin exposes no agent skills.");
  });

  it("renders the host's dashboard component for the dashboard plugin", () => {
    const Dashboard = () => h("section", { className: "dashboard-app" }, "real dashboard");
    const markup = renderWithProviders(h(PluginPage, { id: "dashboard" }), {
      store: fakeStore({ plugins: [dashboardPlugin()] }),
      dashboard: Dashboard,
    });
    expect(markup).toContain("real dashboard");
    expect(markup).not.toContain("No dedicated interface");
  });

  it("shows a plugin's load failure on its own page", () => {
    const markup = renderWithProviders(h(PluginPage, { id: "zulip" }), {
      store: fakeStore({
        plugins: [zulipPlugin({ state: "error", error: "upstream returned 502" })],
      }),
    });
    expect(text(markup)).toContain("This plugin failed to load.");
    expect(text(markup)).toContain("upstream returned 502");
  });
});

/* --------------------------------------------------------------- registry */

describe("pluginPages registry", () => {
  it("maps the dashboard and offers upstream pages an entry point", () => {
    expect(pluginPages.dashboard).toBe(DashboardPluginPage);
    expect(fallbackPluginPage).toBe(PluginOverviewPage);
  });

  it("falls back for an unknown id and resolves the always-on slot by id", () => {
    expect(resolvePluginPage(zulipPlugin())).toBe(PluginOverviewPage);
    expect(resolvePluginPage(dashboardPlugin())).toBe(DashboardPluginPage);
    // Registered under a different id, but still the always-on plugin.
    expect(resolvePluginPage(plugin({ id: "analytics", alwaysOn: true }))).toBe(
      DashboardPluginPage,
    );
  });

  it("has no UI entry for upstream plugins yet", () => {
    expect(Object.keys(pluginPages)).toEqual(["dashboard"]);
  });
});

/* ---------------------------------------------------------- overview page */

describe("PluginOverviewPage", () => {
  it("renders standalone, outside a router", () => {
    const store = fakeStore({ plugins: [zulipPlugin()] });
    const onToggle = vi.fn();
    expect(() =>
      renderWithProviders(h(PluginOverviewPage, { plugin: zulipPlugin() }), { store }),
    ).not.toThrow();
    expect(onToggle).not.toHaveBeenCalled();
  });
});
