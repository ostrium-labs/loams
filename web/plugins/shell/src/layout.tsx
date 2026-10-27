// The console layout (design 59, D1078): the frame (frame.tsx) holds the sidebar
// (sidebar.tsx), the top bar, which is the window's drag region, and the main region, with
// the inspector on its right and the drawer below it (regions.tsx). The agent is the
// inspector's "Agent" tab (W10). Below 960px the sidebar and the inspector are sheets.
// Components get props and closures, never `ctx`.

import {
  capabilities,
  type FlagsService,
  localThemeStore,
  type Mode,
  type ModeService,
  type PanelService,
  type SessionService,
  ThemeController,
  type ThemeService,
} from '@loams/console-host';
import { useSlot, useSlotRegistry } from '@loams/slots';
import { Toaster, ToastProvider, TooltipProvider } from '@loams/ui';
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
  useSyncExternalStore,
} from 'react';
import { AppearanceMenu } from './appearance.js';
import { EngineTooOld } from './engine.js';
import { MainRegion, Outlet, TopBar, UserMenu, useNarrow, useSidebarState } from './frame.js';
import { useShellKeys } from './keyboard.js';
import { isMacPlatform, keyLabel } from './keys.js';
import { MODE_LABEL, ModeSwitcher, useMode, useModeShortcut } from './mode-switcher.js';
import { useLocation } from './nav.js';
import {
  currentProject,
  isSettingsPage,
  pageEntries,
  readPinned,
  settingsHome,
  togglePinned,
} from './pages.js';
import { type ScopedCommand, ShellPalette } from './palette.js';
import { DRAWER, INSPECTOR, PanelController } from './panel.js';
import { PanelRegion, PanelToggles } from './regions.js';
import type { HashRouter } from './router.js';
import { ShellSidebar } from './sidebar.js';

export {
  inMode,
  LEGACY,
  type ModeFilter,
  type NavItem,
  navSections,
  SECTIONS,
  STUDIO_SECTIONS,
} from './nav.js';

/**
 * Sends a visitor without a session to sign-in (or a fresh install to setup), as the
 * original console's session gate did. Public pages (sign-in, setup) stay put.
 */
function useGate(router: HashRouter, session: SessionService) {
  const location = useLocation(router);
  const gate = useSyncExternalStore(session.subscribe, () => session.gate?.() ?? 'none');
  const isPublic = router.spec()?.public === true;
  const pending =
    location.pageId !== undefined && !isPublic && (gate === 'signed-out' || gate === 'setup');
  useEffect(() => {
    if (!pending) return;
    if (gate === 'setup') return router.navigate('/setup');
    const here = `${location.path}${location.query ? `?${new URLSearchParams(location.query)}` : ''}`;
    router.navigate(here === '/' ? '/sign-in' : `/sign-in?next=${encodeURIComponent(here)}`);
  }, [pending, gate, location, router]);
  return pending;
}

/**
 * The palette with the shell's own rows (SD1 Task 24, D1080): "Go to" for every page, the
 * pin commands, and the general commands (switch mode, theme, agent panel).
 */
function Palette({
  router,
  mode,
  theme,
  inspector,
  open,
  onOpenChange,
}: {
  router: HashRouter;
  mode?: ModeService;
  theme: ThemeService;
  inspector: PanelService;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const sections = useSlot('shell.nav.section');
  const agent = useSlot('shell.inspector.tab').some((e) => e.meta?.id === 'agent');
  const legacy = useSlot('console.nav');
  const current = useMode(mode);
  const modesOf = (id: string) => mode?.modesOf(id) ?? ['console'];
  const location = useLocation(router);
  const project = currentProject(location.path);
  const all = pageEntries(sections, legacy, project);
  // Every page in every mode (D1080): Settings pages under "Settings", the rest under "Pages".
  // A typed query also offers to pin or unpin each page for the open project.
  const nav = (m: Mode, query: string): ScopedCommand[] => {
    if (m !== current) return [];
    const pinned = query.trim() ? readPinned(project) : [];
    return all.flatMap((p): ScopedCommand[] => {
      const settings = isSettingsPage(p);
      const go: ScopedCommand = {
        id: `go:${p.href}`,
        title: `Go to ${p.label}`,
        group: settings ? 'Settings' : 'Pages',
        keywords: [p.group, p.href, p.id],
        mode: m,
        run: () => router.navigate(p.href),
      };
      if (settings || !query.trim()) return [go];
      const on = pinned.includes(p.id);
      return [
        go,
        {
          id: `pin:${p.id}`,
          title: on ? `Unpin ${p.label} from the sidebar` : `Pin ${p.label} to the sidebar`,
          group: 'Sidebar',
          keywords: ['pin', 'unpin', 'sidebar', p.id],
          secondary: true,
          mode: m,
          run: () => {
            togglePinned(project, p.id);
          },
        },
      ];
    });
  };
  const other = mode?.available.find((m) => m !== current);
  const general: ScopedCommand[] = [
    ...(mode && other
      ? [
          {
            id: 'mode',
            title: `Switch to ${MODE_LABEL[other]}`,
            group: 'General',
            keywords: ['mode'],
            shortcut: keyLabel('mod+shift+m', isMacPlatform()),
            mode: current,
            run: () => mode.set(other),
          },
        ]
      : []),
    ...(['system', 'dark', 'light'] as const).map((c) => ({
      id: `theme:${c}`,
      title: `Theme: ${c === 'system' ? 'System' : c === 'dark' ? 'Dark' : 'Light'}`,
      group: 'Appearance',
      keywords: ['appearance', 'theme', 'colour', 'dark mode'],
      mode: current,
      run: () => theme.set(c),
    })),
    ...(agent
      ? [
          {
            id: 'agent',
            title: 'Open the agent',
            group: 'General',
            keywords: ['agent', 'assistant', 'inspector'],
            mode: current,
            run: () => inspector.open({ tab: 'agent' }),
          },
        ]
      : []),
  ];
  return (
    <ShellPalette
      modes={mode ? { current, available: mode.available, modesOf } : undefined}
      nav={nav}
      general={general}
      open={open}
      onOpenChange={onOpenChange}
    />
  );
}

export function Layout({
  router,
  session,
  flags,
  mode,
  theme: given,
  inspector: givenInspector,
  drawer: givenDrawer,
}: {
  router: HashRouter;
  session: SessionService;
  /** GetInstance's answer: the instance name, edition and version for the chrome. */
  flags?: FlagsService;
  /** Console or Studio (§53 §4.2): filters the nav and the header, and drives the switcher. */
  mode?: ModeService;
  /** System, Dark or Light (D1074); the platform provides it, and tests may leave it out. */
  theme?: ThemeService;
  /** The right inspector and the bottom drawer (D1078); the shell's plugin provides them. */
  inspector?: PanelService;
  drawer?: PanelService;
}) {
  const current = useMode(mode);
  useModeShortcut(mode);
  const theme = useMemo(() => given ?? new ThemeController(localThemeStore()), [given]);
  const inspector = useMemo(
    () => givenInspector ?? new PanelController(INSPECTOR),
    [givenInspector],
  );
  const drawer = useMemo(() => givenDrawer ?? new PanelController(DRAWER), [givenDrawer]);
  const [side, setSide] = useSidebarState();
  const narrow = useNarrow();
  const [sheetOpen, setSheetOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const sidebarOpen = narrow ? sheetOpen : side.open;
  const toggleSidebar = useCallback(
    () => (narrow ? setSheetOpen((v) => !v) : setSide({ open: !side.open })),
    [narrow, side.open, setSide],
  );
  const location = useLocation(router);
  // A route profile (T9-6: a Studio thread opens the inspector on Diff at 420px) applies
  // before the paint, so the thread never flashes without its panel. Below 960px the
  // inspector is a sheet over the thread, so the profile leaves it closed there.
  useLayoutEffect(() => {
    for (const panel of [inspector, drawer]) {
      if (panel instanceof PanelController) panel.route(location.path, { narrow });
    }
  }, [inspector, drawer, location.path, narrow]);
  // oxlint-disable-next-line react/exhaustive-deps -- close the sheet per route
  useEffect(() => setSheetOpen(false), [location.path]);

  // The one keymap (D1081): every shell chord, with the focused editor or terminal keeping
  // its own keys (Review Focus 1).
  const slots = useSlotRegistry();
  useShellKeys({
    router,
    inspector,
    drawer,
    toggleSidebar,
    togglePalette: () => setPaletteOpen((v) => !v),
    openSettings: () => {
      const home = settingsHome(
        pageEntries(
          slots.entries('shell.nav.section'),
          slots.entries('console.nav'),
          currentProject(router.current().path),
        ),
      );
      if (home) router.navigate(home.href);
    },
    closeOverlay: () => {
      if (narrow && sheetOpen) {
        setSheetOpen(false);
        return true;
      }
      if (narrow && inspector.state().open) {
        inspector.close();
        return true;
      }
      return false;
    },
  });

  const gated = useGate(router, session);
  // An engine older than N−1, or without GetInstance, gets the upgrade page
  // instead of pages that would fail (§54 §13.1).
  const caps = flags ? capabilities(flags) : undefined;
  if (caps && !caps.supported) return <EngineTooOld caps={caps} />;
  if (gated) return <div className="lc-boot" aria-busy="true" />;
  if (router.spec()?.bare) {
    return (
      <ToastProvider>
        <main className="lc-bare" id="lc-main">
          <Outlet router={router} />
        </main>
        <Toaster />
      </ToastProvider>
    );
  }
  return (
    <ToastProvider>
      <TooltipProvider>
        <div
          className="lc-shell"
          data-mode={current}
          data-sidebar={sidebarOpen ? 'open' : 'closed'}
        >
          <ShellSidebar
            router={router}
            flags={flags}
            mode={mode}
            width={side.width}
            onWidth={(w) => setSide({ width: w })}
            open={sidebarOpen}
            narrow={narrow}
            onClose={() => (narrow ? setSheetOpen(false) : setSide({ open: false }))}
          />
          <div className="lc-body">
            <TopBar
              router={router}
              flags={flags}
              mode={mode}
              session={session}
              sidebarOpen={sidebarOpen}
              onToggleSidebar={toggleSidebar}
              onOpenPalette={() => setPaletteOpen(true)}
              tools={
                <>
                  {mode && <ModeSwitcher mode={mode} />}
                  <AppearanceMenu theme={theme} />
                  <UserMenu session={session} router={router} local={flags?.features.local} />
                </>
              }
              trailing={<PanelToggles inspector={inspector} drawer={drawer} router={router} />}
            />
            <div className="lc-work">
              <div className="lc-work-row">
                <main className="lc-main" id="lc-main">
                  <MainRegion router={router} mode={mode} />
                </main>
                <PanelRegion kind="inspector" panel={inspector} router={router} />
              </div>
              <PanelRegion kind="drawer" panel={drawer} router={router} />
            </div>
          </div>
        </div>
        <Palette
          router={router}
          mode={mode}
          theme={theme}
          inspector={inspector}
          open={paletteOpen}
          onOpenChange={setPaletteOpen}
        />
        <Toaster />
      </TooltipProvider>
    </ToastProvider>
  );
}
