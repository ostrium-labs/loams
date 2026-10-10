/*
 * The shell's primary navigation.
 *
 * Kept apart from `AppShell` so the active-state rule is a plain function over
 * a pathname that can be tested without rendering anything. `AppShell` feeds it
 * react-router's location, but nothing here imports react-router -- that keeps
 * the rule honest as a rule about paths rather than about the router in use.
 */

/** The plugin id the dashboard is registered under by default. */
export const DASHBOARD_PLUGIN_ID = "dashboard";

export type NavIcon = "dashboard" | "plugins" | "console";

export interface NavItem {
  to: string;
  label: string;
  icon: NavIcon;
  hint: string;
}

export const NAV_ITEMS: NavItem[] = [
  {
    to: "/",
    label: "Dashboard",
    icon: "dashboard",
    hint: "Analytics & performance",
  },
  { to: "/plugins", label: "Plugins", icon: "plugins", hint: "What is switched on" },
  { to: "/console", label: "Console", icon: "console", hint: "Turn plugins on and off" },
];

/**
 * Split a pathname into non-empty segments.
 *
 * Filtering empties is what makes a trailing slash (`/plugins/`), a doubled
 * slash (`/plugins//`) and the root (`/`) all normalise for free, and makes a
 * *missing* plugin id (`/plugins/`) land on the plugins index rather than on a
 * plugin page with `id === ""`.
 */
export function toSegments(pathname: string): string[] {
  const withoutQuery = pathname.split(/[?#]/)[0] ?? "";
  return withoutQuery.split("/").filter((segment) => segment.length > 0);
}

/**
 * Decode one path segment.
 *
 * A malformed percent-escape is not worth throwing over, so the raw segment is
 * used as-is; the page then renders its "unknown plugin" state.
 */
export function safeDecode(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

/**
 * Which nav entry is lit for the current path.
 *
 * Exactly one entry is ever lit. "Plugins" is the active section for the index
 * *and* for a plugin's own page, so a deep link into e.g. Zulip still shows
 * where you are -- except on the dashboard plugin's page, which lives under
 * `/plugins/` but belongs to the Dashboard section instead.
 *
 * This deliberately does not just defer to react-router's `NavLink`, whose
 * prefix matching would light both "Dashboard" and "Plugins" on
 * `/plugins/dashboard`. The exception is the product's rule, so it is written
 * out here and `AppShell` renders `aria-current` from the result.
 */
export function isNavActive(
  item: NavItem,
  pathname: string,
  dashboardPluginId: string = DASHBOARD_PLUGIN_ID,
): boolean {
  const segments = toSegments(pathname);

  const onRoot = segments.length === 0;
  const onPluginsIndex = segments.length === 1 && segments[0] === "plugins";
  const onConsole = segments.length === 1 && segments[0] === "console";
  const onPluginPage = segments.length === 2 && segments[0] === "plugins";
  const pluginId = onPluginPage ? safeDecode(segments[1]!) : undefined;

  switch (item.to) {
    case "/":
      return onRoot || (onPluginPage && pluginId === dashboardPluginId);
    case "/plugins":
      return onPluginsIndex || (onPluginPage && pluginId !== dashboardPluginId);
    case "/console":
      return onConsole;
    default:
      return false;
  }
}
