/**
 * Per-plugin page registry.
 *
 * An upstream plugin that wants a real screen adds one entry here and nothing
 * else -- no change to the router, the shell, or the pages. Anything absent
 * from this map falls through to the overview page.
 */
import type React from "react";
import type { PluginStatus } from "../../types.js";
import { isAlwaysOn } from "../pluginState.js";
import { DashboardPluginPage } from "./DashboardPluginPage.js";
import { PluginOverviewPage } from "./PluginOverviewPage.js";

export const pluginPages: Record<string, React.ComponentType<{ plugin: PluginStatus }>> = {
  dashboard: DashboardPluginPage,
  // upstream plugin pages get added here by later work
};

/** The page used when a plugin id has no entry above. */
export const fallbackPluginPage = PluginOverviewPage;

/**
 * Look up the page for a plugin.
 *
 * The manifest's own id is tried first; the always-on slot is checked second so
 * the dashboard renders its real UI even if the host registers it under a
 * different id (`alwaysOn` is what the contract actually guarantees).
 */
export function resolvePluginPage(
  plugin: PluginStatus,
): React.ComponentType<{ plugin: PluginStatus }> {
  return (
    pluginPages[plugin.id] ??
    (isAlwaysOn(plugin) ? pluginPages.dashboard : undefined) ??
    fallbackPluginPage
  );
}
