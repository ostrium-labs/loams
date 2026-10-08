/**
 * `@loams-core/host/ui` -- the UI shell, the plugin console, and the plugins page.
 *
 * The host app renders `<CoreShell dashboard={App} />` and nothing else, inside
 * a `QueryClientProvider` (see `dashboard-ui/src/main.tsx`). Routing is
 * react-router's and lives inside `CoreShell`; server state is TanStack Query's
 * and lives one level above it.
 */
export { CoreShell, ShellProviders, ShellRoutes } from "./AppShell.js";
export type { CoreShellProps } from "./AppShell.js";

export { cn } from "./cn.js";
export type { ClassValue } from "./cn.js";

/*
 * The t3code primitives.
 *
 * Exported from here rather than kept in the host app because the shell itself
 * uses them -- the plugin console's switch is one of them -- and `dashboard-ui`
 * sits *above* `@loams-core/host` in the graph. `dashboard-ui/src/components/*.tsx`
 * imports every one of these from `@loams-core/host/ui`.
 */
export { Alert, AlertAction, AlertDescription, AlertTitle } from "./primitives/index.js";
export { Badge, badgeVariants } from "./primitives/index.js";
export { Button, buttonVariants, InlineButton } from "./primitives/index.js";
export type { ButtonSize, ButtonVariant } from "./primitives/index.js";
export {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "./primitives/index.js";
export { Kbd, KbdGroup } from "./primitives/index.js";
export { Label } from "./primitives/index.js";
export { Separator } from "./primitives/index.js";
export { Skeleton } from "./primitives/index.js";
export { Switch } from "./primitives/index.js";

export { NAV_ITEMS, isNavActive, DASHBOARD_PLUGIN_ID, toSegments, safeDecode } from "./nav.js";
export type { NavIcon, NavItem } from "./nav.js";

export { ConsolePage } from "./ConsolePage.js";
export { PluginsPage } from "./PluginsPage.js";
export { PluginPage } from "./PluginPage.js";

export { PluginsProvider, usePlugins } from "./usePlugins.jsx";
export type { FeedbackKind, PluginFeedback, PluginStore, ToggleResult } from "./usePlugins.jsx";

export {
  PLUGINS_QUERY_KEY,
  applyOptimisticToggle,
  readPlugins,
  rollbackToggle,
} from "./pluginQuery.js";
export type { ToggleContext, ToggleVariables } from "./pluginQuery.js";

export {
  ApiError,
  fetchPlugin,
  fetchPluginAgentCard,
  fetchPlugins,
  setPluginEnabled,
} from "./api.js";
export type { AgentCard } from "./api.js";

export { fallbackPluginPage, pluginPages, resolvePluginPage } from "./pluginPages/registry.js";
