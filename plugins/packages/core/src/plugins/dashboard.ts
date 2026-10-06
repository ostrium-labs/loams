/**
 * The dashboard, as a plugin.
 *
 * The dashboard is not special-cased anywhere else in the platform: it is a
 * manifest with `alwaysOn`, pinned to the top of the console, and its skills
 * read the same `ctx.store` / `ctx.dashboard` services the REST routes read.
 * That is deliberate — if the dashboard's skills were implemented by a
 * separate path, they would drift from what the dashboard actually does.
 *
 * `alwaysOn` is load-bearing, not cosmetic: it is what makes the host load it
 * at boot, makes the registry refuse to disable it, and disables its toggle.
 */

import type { PluginAgentSkill, PluginLoader, PluginManifest } from "../types.js";
import type { Context } from "cordis";

// Side-effect imports: these augment cordis `Context` with the keys the skills
// use. Declaring them locally would collide with the owning packages.
import "@loams-plugins/plugin-store";
import "@loams-plugins/plugin-dashboard-spec";

export const DASHBOARD_SKILLS: PluginAgentSkill[] = [
  {
    id: "listDashboards",
    name: "List dashboards",
    description: "List every stored dashboard with its id, title and version.",
    tags: ["dashboard", "read"],
    examples: ["listDashboards"],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "getDashboard",
    name: "Get dashboard",
    description: "Load one dashboard spec by id.",
    tags: ["dashboard", "read"],
    examples: ['getDashboard {"dashboardId":"..."}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
  {
    id: "describeWidget",
    name: "Describe widget",
    description:
      "Describe one widget of a dashboard: its type, layout box, data source and chart encoding.",
    tags: ["dashboard", "read"],
    examples: ['describeWidget {"dashboardId":"...","widgetId":"widget-revenue-trend"}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];

export const DASHBOARD_MANIFEST: PluginManifest = {
  id: "dashboard",
  name: "Dashboard",
  description: "The dashboard itself: saved dashboards, their widgets and their data.",
  version: "1.0.0",
  category: "core",
  // The dashboard is the platform, not a page under /plugins.
  uiPath: "/",
  order: 0,
  alwaysOn: true,
  defaultEnabled: true,
  agent: {
    name: "Dashboard Agent",
    description: "Reads dashboards and widgets out of the platform store.",
    version: "1.0.0",
    skills: DASHBOARD_SKILLS,
  },
};

/**
 * The `store`/`dashboard` context keys, read defensively.
 *
 * Defensive for a reason that is easy to miss: cordis resolves an inherited
 * context key only on a context that declared it in `inject`, so reading
 * `ctx.dashboard` from the host's context THROWS when the dashboard-spec service
 * is not reachable -- it does not return `undefined`. Every skill below already
 * has a `store` fallback written for the case where `dashboard` is absent, so the
 * fallback is useless unless the probe cannot throw. Each read is therefore
 * guarded independently: `dashboard` missing degrades that skill to the store,
 * it does not take the whole skill down.
 */
interface DashboardContext {
  store?: {
    listDashboards(): Promise<unknown>;
    getDashboard(id: string): Promise<any>;
  };
  dashboard?: {
    load(id: string): Promise<any>;
  };
}

function probe<T>(ctx: Context, key: string): T | undefined {
  try {
    return (ctx as unknown as Record<string, unknown>)[key] as T;
  } catch {
    return undefined;
  }
}

function dashboardService(ctx: Context): DashboardContext["store"] {
  return probe<DashboardContext["store"]>(ctx, "store");
}

function specService(ctx: Context): DashboardContext["dashboard"] {
  return probe<DashboardContext["dashboard"]>(ctx, "dashboard");
}

function requireString(value: unknown, field: string, skill: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${skill}: "${field}" is required and must be a non-empty string`);
  }
  return value;
}

export const dashboardLoader: PluginLoader = {
  skills: () => [
    {
      id: "listDashboards",
      // `api.ctx` rather than `this`: a skill handler is a plain method on an
      // object literal, so `this` is the handler record, not the plugin's
      // context. The host passes the context in on every invocation.
      handle: async (_params, api) => {
        const store = dashboardService(api.ctx);
        // Delegated to ctx.store rather than ctx.dashboard because
        // DashboardSpecService caches only the last-loaded spec; the list is a
        // store concern.
        return (await store?.listDashboards()) ?? [];
      },
    },
    {
      id: "getDashboard",
      handle: async (params, api) => {
        const id = requireString(params.dashboardId ?? params.id, "dashboardId", "getDashboard");
        const dashboard = specService(api.ctx);
        const store = dashboardService(api.ctx);
        if (dashboard) return dashboard.load(id);
        if (!store) throw new Error("getDashboard: dashboard service is not available");
        return store.getDashboard(id);
      },
    },
    {
      id: "describeWidget",
      handle: async (params, api) => {
        const dashboardId = requireString(
          params.dashboardId ?? params.id,
          "dashboardId",
          "describeWidget",
        );
        const widgetId = requireString(params.widgetId, "widgetId", "describeWidget");
        const dashboard = specService(api.ctx);
        const store = dashboardService(api.ctx);
        const spec = dashboard
          ? await dashboard.load(dashboardId)
          : store
            ? await store.getDashboard(dashboardId)
            : undefined;
        if (!spec) throw new Error("describeWidget: dashboard service is not available");
        const widget = spec.widgets?.[widgetId];
        if (!widget) {
          throw new Error(`describeWidget: widget "${widgetId}" not found in "${dashboardId}"`);
        }
        const layoutEntry = (spec.layout ?? []).find((entry: any) => entry.id === widgetId);
        return {
          dashboardId,
          dashboardTitle: spec.title,
          dashboardVersion: spec.version,
          widgetId,
          type: widget.type,
          layout: layoutEntry ?? null,
          data: widget.data ?? null,
          chart: widget.chart ?? null,
          interactions: widget.interactions ?? [],
        };
      },
    },
  ],
};
