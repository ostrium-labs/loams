/**
 * Pure derivations for the plugin list.
 *
 * Nothing here touches React or the network -- every function is a plain
 * `PluginStatus[] -> something` derivation, which is what makes the list rules
 * testable without a renderer.
 *
 * The toggle *state machine* used to live here too (`togglePlugin` and its
 * `onChange`/`getCurrent`/`refresh` deps) and is now gone: TanStack Query owns
 * the list, so the optimistic write and its rollback live in `pluginQuery.ts`
 * against a `QueryClient`. The one piece that survived the move is
 * `optimisticToggle` below -- the cache surgery is an edit of an array, and
 * saying so in one pure function is what keeps the query layer honest about
 * *what* it writes optimistically.
 */
import type { PluginManifest, PluginStatus } from "../types.js";

/** Shown next to the dashboard's disabled toggle, and returned if it is forced. */
export const ALWAYS_ON_REASON = "Always on — this is the product itself";

export function isAlwaysOn(plugin: PluginManifest): boolean {
  return plugin.alwaysOn === true;
}

/** Human label for the server-reported load state. */
export function describeState(plugin: PluginStatus): string {
  switch (plugin.state) {
    case "loaded":
      return "Loaded";
    case "unloaded":
      return "Not loaded";
    case "error":
      return "Failed to load";
    default:
      return "Unknown";
  }
}

export function countSkills(plugin: PluginManifest): number {
  return plugin.agent?.skills?.length ?? 0;
}

/**
 * Where the plugins page should send the user when a card is clicked.
 *
 * `uiPath` comes from a manifest we do not control, so it is treated as
 * advisory: a trailing slash is trimmed, a missing leading slash is added, and
 * a blank/`"/"` value falls back to the route the shell can actually serve.
 */
export function pluginHref(plugin: PluginManifest): string {
  const raw = typeof plugin.uiPath === "string" ? plugin.uiPath.trim() : "";
  if (raw.length === 0 || raw === "/") {
    return `/plugins/${encodeURIComponent(plugin.id)}`;
  }
  const withLeadingSlash = raw.startsWith("/") ? raw : `/${raw}`;
  const trimmed = withLeadingSlash.replace(/\/+$/, "");
  return trimmed.length > 1 ? trimmed : trimmed;
}

export function findPlugin(plugins: PluginStatus[], id: string | undefined): PluginStatus | null {
  if (!id) return null;
  return plugins.find((plugin) => plugin.id === id) ?? null;
}

/**
 * Split the console into the pinned product row and the everything else.
 *
 * Order within each group is the server's, untouched.
 */
export function partitionPlugins(plugins: PluginStatus[]): {
  pinned: PluginStatus[];
  rest: PluginStatus[];
} {
  return {
    pinned: plugins.filter(isAlwaysOn),
    rest: plugins.filter((plugin) => !isAlwaysOn(plugin)),
  };
}

/** Enabled plugins only -- this is what the plugins page lists. */
export function enabledPlugins(plugins: PluginStatus[]): PluginStatus[] {
  return plugins.filter((plugin) => plugin.enabled);
}

/**
 * Apply the user's intent locally, before the server has agreed to it.
 *
 * This is the optimistic write `pluginQuery.applyOptimisticToggle` puts into the
 * `['plugins']` cache. It sets `state` as well as `enabled` because a switch that
 * reads "on" while the row still says "Not loaded" contradicts itself for the
 * duration of the request, and it clears `error` because turning a plugin on is
 * the user asking for a fresh attempt, not for the previous failure to stick.
 */
export function optimisticToggle(
  plugins: PluginStatus[],
  id: string,
  enabled: boolean,
): PluginStatus[] {
  return plugins.map((plugin) =>
    plugin.id === id
      ? {
          ...plugin,
          enabled,
          state: enabled ? "loaded" : "unloaded",
          error: undefined,
        }
      : plugin,
  );
}

/** Turn anything thrown into a sentence worth putting on screen. */
export function messageOf(error: unknown): string {
  if (error instanceof Error && error.message.length > 0) return error.message;
  if (typeof error === "string" && error.length > 0) return error;
  return "Something went wrong.";
}
