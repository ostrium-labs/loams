/**
 * Shared fixtures for the UI tests.
 *
 * There is no DOM in this workspace (`jsdom`/`happy-dom` are not installed and
 * the root `vitest.config.ts` is not ours to edit), so the pages are exercised
 * through `react-dom/server`'s static renderer against a fixed plugin store and
 * a react-router `MemoryRouter`. That covers everything the shell decides in
 * markup -- which route matched, what is listed, in what order, which controls
 * are disabled, what each page says when it cannot do the usual thing.
 *
 * Behaviour that only exists in a handler (the optimistic toggle and its
 * rollback) lives in `pluginQuery.ts` and is tested there directly, against a
 * real `QueryClient`, with no renderer involved.
 */
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryRouter } from "react-router";
import type { PluginManifest, PluginStatus } from "../../src/types.js";
import { DashboardSlotContext } from "../../src/ui/DashboardSlot.js";
import { PluginsProvider } from "../../src/ui/usePlugins.jsx";
import type { PluginStore } from "../../src/ui/usePlugins.jsx";

export const h = React.createElement;

/* ------------------------------------------------------------- plugins --- */

export function manifest(overrides: Partial<PluginManifest> & { id: string }): PluginManifest {
  return {
    name: overrides.id,
    description: `The ${overrides.id} plugin`,
    version: "1.0.0",
    uiPath: `/plugins/${overrides.id}`,
    ...overrides,
  };
}

export function plugin(overrides: Partial<PluginStatus> & { id: string }): PluginStatus {
  return {
    enabled: true,
    state: "loaded",
    ...manifest(overrides),
    ...overrides,
  };
}

export function dashboardPlugin(overrides: Partial<PluginStatus> = {}): PluginStatus {
  return plugin({
    id: "dashboard",
    name: "Dashboard",
    description: "Analytics and performance, powered by Superset and Flint.",
    uiPath: "/plugins/dashboard",
    alwaysOn: true,
    order: 0,
    enabled: true,
    state: "loaded",
    category: "product",
    ...overrides,
  });
}

export function zulipPlugin(overrides: Partial<PluginStatus> = {}): PluginStatus {
  return plugin({
    id: "zulip",
    name: "Zulip",
    description: "Message streams from a Zulip organisation.",
    uiPath: "/plugins/zulip",
    order: 10,
    category: "upstream",
    upstream: { product: "Zulip Cloud", envPrefix: "ZULIP_" },
    agent: {
      name: "zulip-agent",
      description: "Reads and writes Zulip messages.",
      version: "1.0.0",
      skills: [
        { id: "listChannels", name: "List channels", description: "Every stream in the org." },
        { id: "sendMessage", name: "Send a message", description: "Post to a stream." },
      ],
    },
    ...overrides,
  });
}

/* --------------------------------------------------------------- store --- */

/**
 * A `PluginStore` with no network behind it. `setEnabled` is supplied by the
 * test when the toggle itself is under test.
 */
export function fakeStore(overrides: Partial<PluginStore> = {}): PluginStore {
  return {
    plugins: [],
    loading: false,
    error: null,
    refresh: () => Promise.resolve(overrides.plugins ?? []),
    setEnabled: () => Promise.resolve({ ok: true }),
    pending: {},
    feedback: {},
    dismissFeedback: () => undefined,
    ...overrides,
  };
}

/* ------------------------------------------------------------- render --- */

export interface RenderOptions {
  store?: PluginStore;
  /** The URL the router starts at. Mirrors `MemoryRouter`'s `initialEntries`. */
  pathname?: string;
  dashboard?: React.ComponentType | null;
}

/** Render a page inside the same providers `CoreShell` supplies. */
export function renderWithProviders(node: React.ReactElement, options: RenderOptions = {}): string {
  // `children` goes in the props object rather than into `h`'s variadic third
  // argument: `MemoryRouter` and `PluginsProvider` both declare `children` as
  // REQUIRED, and React's variadic-children overload only applies to an optional
  // one. Nested on purpose rather than composed in JSX -- this is a `.ts` file.
  const router = h(MemoryRouter, {
    initialEntries: [options.pathname ?? "/"],
    children: node,
  });
  const slot = h(DashboardSlotContext.Provider, {
    value: options.dashboard ?? null,
    children: router,
  });
  const store = h(PluginsProvider, { store: options.store ?? fakeStore(), children: slot });
  return renderToStaticMarkup(store);
}

/** Collapse the markup so assertions can be about text, not indentation. */
export function text(markup: string): string {
  return markup
    .replace(/<[^>]+>/g, " ")
    .replace(/&#x27;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ")
    .trim();
}
