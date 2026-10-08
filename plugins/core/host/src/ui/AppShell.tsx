/**
 * `CoreShell` -- the chrome the app opens into.
 *
 * The nav here is deliberately slim and quiet. The dashboard plugin brings its
 * own richer header (title, live badge, time range, actions) and putting a
 * second title above it would read as a duplicated heading, so the shell only
 * contributes a breadcrumb-quality bar and lets the page own its page head.
 *
 * Routing is react-router's: one `<BrowserRouter>` for the app, a nested
 * `<Route>` whose element is `ShellFrame` so the chrome renders once and each
 * page arrives through `<Outlet />`. Server state is TanStack Query, owned
 * higher up in `main.tsx` by its `QueryClientProvider`.
 */
import React from "react";
import {
  BrowserRouter,
  NavLink,
  Outlet,
  Route,
  Routes,
  useLocation,
  useParams,
} from "react-router";
import { PluginsProvider } from "./usePlugins.jsx";
import type { PluginStore } from "./usePlugins.jsx";
import { DashboardSlotContext } from "./DashboardSlot.js";
import { NAV_ITEMS, DASHBOARD_PLUGIN_ID, isNavActive } from "./nav.js";
import type { NavIcon } from "./nav.js";
import { cn } from "./cn.js";
import {
  BTN_BASE,
  BTN_OUTLINE,
  BTN_PRIMARY,
  EMPTY_STATE,
  EMPTY_STATE_ACTIONS,
  EMPTY_STATE_BODY,
  EMPTY_STATE_MARK,
  EMPTY_STATE_PATH,
  EMPTY_STATE_TITLE,
  PAGE,
} from "./uiClasses.js";
import { ConsolePage } from "./ConsolePage.js";
import { PluginsPage } from "./PluginsPage.js";
import { PluginPage } from "./PluginPage.js";
import { DashboardEntry } from "./pluginPages/DashboardPluginPage.js";
import { AlertIcon, GaugeIcon, LayersIcon, PlugIcon, ShellMarkIcon } from "./icons.js";
import { Separator } from "./primitives/index.js";

const NAV_ICONS: Record<NavIcon, React.ComponentType> = {
  dashboard: GaugeIcon,
  plugins: LayersIcon,
  console: PlugIcon,
};

/*
 * Stops NavLink from setting `aria-current` on the anchor, so the `<li>` can
 * carry it instead.
 *
 * NavLink derives its own value from prefix matching, which cannot express this
 * product's rule: on `/plugins/dashboard` it would mark BOTH "Dashboard" (as
 * `/` is a prefix) and "Plugins" (as `/plugins` is) as the current page. The
 * rule says Dashboard owns that URL, so exactly one element in the tree should
 * announce it -- the one `isNavActive` picks.
 *
 * There is no value that both type-checks and suppresses it: `undefined` hits
 * the destructuring default of `"page"`, and the prop is applied *after* the
 * `...rest` spread, so a caller cannot override it. Hence the cast. It is safe
 * at runtime because React omits any attribute whose value is `null`.
 */
const SUPPRESS_ARIA_CURRENT = { "aria-current": null } as unknown as { "aria-current": "page" };

function NotFoundPage() {
  const { pathname } = useLocation();
  return (
    <div className={PAGE}>
      <div className={EMPTY_STATE}>
        <div className={EMPTY_STATE_MARK} aria-hidden="true">
          <AlertIcon />
        </div>
        <h1 className={EMPTY_STATE_TITLE}>Nothing at this address</h1>
        <p className={EMPTY_STATE_BODY}>
          <code className={EMPTY_STATE_PATH}>{pathname}</code> does not match any page in the app.
        </p>
        <div className={EMPTY_STATE_ACTIONS}>
          <NavLink to="/" className={cn(BTN_BASE, BTN_OUTLINE)}>
            Go to the dashboard
          </NavLink>
          <NavLink to="/plugins" className={cn(BTN_BASE, BTN_PRIMARY)}>
            Browse plugins
          </NavLink>
        </div>
      </div>
    </div>
  );
}

/**
 * `/plugins/:id`.
 *
 * The id arrives through the router rather than a prop so the route table
 * stays declarative, and so a deep link straight to `/plugins/zulip` is a
 * normal navigation rather than a special case.
 */
function PluginRoute() {
  const { id } = useParams();
  return <PluginPage id={id} />;
}

function ShellNav({ dashboardPluginId }: { dashboardPluginId: string }) {
  const { pathname } = useLocation();

  return (
    <nav className="min-w-0 overflow-x-auto" aria-label="Primary">
      <ul className="flex list-none items-center gap-[0.15rem]">
        {NAV_ITEMS.map((item) => {
          const Icon = NAV_ICONS[item.icon];
          const active = isNavActive(item, pathname, dashboardPluginId);
          return (
            /*
             * `aria-current` lives on the list item rather than the anchor --
             * see `SUPPRESS_ARIA_CURRENT` below for why.
             */
            <li key={item.to} aria-current={active ? "page" : undefined}>
              <NavLink
                to={item.to}
                {...SUPPRESS_ARIA_CURRENT}
                title={item.hint}
                className={({ isActive }) =>
                  cn(
                    "inline-flex items-center gap-[0.4rem] rounded border border-transparent px-[0.7rem] py-[0.3rem] text-[0.8rem] font-medium whitespace-nowrap text-ink-body no-underline transition-[background-color,color] duration-150 hover:bg-page hover:text-ink",
                    (active || isActive) &&
                      "border-primary-border bg-primary-subtle font-semibold text-primary-ink",
                  )
                }
              >
                <span
                  className="inline-flex items-center justify-center opacity-85"
                  aria-hidden="true"
                >
                  <Icon />
                </span>
                <span className="leading-none">{item.label}</span>
              </NavLink>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}

function ShellFrame({ dashboardPluginId }: { dashboardPluginId: string }) {
  const { pathname } = useLocation();

  return (
    <div className="flex min-h-screen flex-col bg-page">
      {/* Off-screen until focused, then pinned to the top-left. */}
      <a
        href="#shell-main"
        className="absolute top-0 left-[-9999px] z-3000 rounded border border-line bg-card px-[0.85rem] py-2 text-[0.8rem] font-semibold text-ink focus:top-3 focus:left-3"
      >
        Skip to content
      </a>

      <header className="sticky top-0 z-1000 flex h-[46px] items-center gap-6 border-b border-line bg-card px-6">
        <NavLink
          to="/"
          className="inline-flex shrink-0 items-center gap-2 text-ink no-underline"
          aria-label="Cordis home"
        >
          <span
            className="inline-flex size-6 items-center justify-center rounded-[5px] bg-primary-subtle text-primary-ink"
            aria-hidden="true"
          >
            <ShellMarkIcon />
          </span>
          <span className="text-[0.88rem] font-bold tracking-[-0.01em]">Cordis</span>
        </NavLink>

        <ShellNav dashboardPluginId={dashboardPluginId} />
      </header>

      <Separator />

      {/* The pathname changes per route, so it is a fresh region per page. */}
      <main className="flex min-w-0 flex-1 flex-col" id="shell-main" key={pathname}>
        <Outlet />
      </main>
    </div>
  );
}

/**
 * The route table.
 *
 * Exported separately from `CoreShell` so tests can mount it inside a
 * `MemoryRouter` at any initial entry without a `window`.
 */
export function ShellRoutes({ dashboardPluginId }: { dashboardPluginId: string }) {
  return (
    <Routes>
      {/* `ShellFrame` is the layout route: it owns the chrome and renders
          each match below it through `<Outlet />`. */}
      <Route element={<ShellFrame dashboardPluginId={dashboardPluginId} />}>
        <Route index element={<DashboardEntry />} />
        <Route path="plugins" element={<PluginsPage />} />
        <Route path="plugins/:id" element={<PluginRoute />} />
        <Route path="console" element={<ConsolePage />} />
        <Route path="*" element={<NotFoundPage />} />
      </Route>
    </Routes>
  );
}

export interface CoreShellProps {
  /**
   * The dashboard component, supplied by the host app.
   *
   * It belongs to `@loams-core/dashboard-ui`, which is above core in the graph, so it
   * is passed in rather than imported.
   */
  dashboard?: React.ComponentType;
  /** The plugin id the dashboard is registered under. */
  dashboardPluginId?: string;
}

/**
 * The plugin store and the dashboard slot.
 *
 * Deliberately router-free so a test can wrap `MemoryRouter` on either side of
 * it and `CoreShell` does not need a seam for injecting a router at all.
 */
export function ShellProviders({
  dashboard,
  store,
  children,
}: {
  dashboard?: React.ComponentType;
  /** Supply a fixed plugin store instead of fetching. Used by tests. */
  store?: PluginStore;
  children: React.ReactNode;
}) {
  return (
    <PluginsProvider store={store}>
      <DashboardSlotContext.Provider value={dashboard ?? null}>
        {children}
      </DashboardSlotContext.Provider>
    </PluginsProvider>
  );
}

export function CoreShell({ dashboard, dashboardPluginId = DASHBOARD_PLUGIN_ID }: CoreShellProps) {
  return (
    <BrowserRouter>
      <ShellProviders dashboard={dashboard}>
        <ShellRoutes dashboardPluginId={dashboardPluginId} />
      </ShellProviders>
    </BrowserRouter>
  );
}
