// @loams/plugin-shell: the layout (the `root` slot), the navigation (from
// `console.nav`), the page outlet (`console.page`), and the `router`, `inspector` and
// `drawer` services (§37 §5.4–§5.5, design 59 D1078). Components get props and closures, never `ctx`.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import type { SlotRegistry } from '@loams/slots';
import { AppearanceSection } from './appearance.js';
import { Layout } from './layout.js';
import { syncModeWithRoute } from './mode-switcher.js';
import { PREFERENCES_ID } from './pages.js';
import { DRAWER, INSPECTOR, PanelController } from './panel.js';
import { PreferencesPage } from './preferences.js';
import { HashRouter } from './router.js';

export {
  AppearanceMenu,
  AppearanceSection,
  readMotion,
  setMotion,
  useThemeState,
} from './appearance.js';
export {
  Crumbs,
  EnvironmentSwitcher,
  MainRegion,
  readSidebar,
  runShellCommand,
  SIDEBAR,
  type SidebarState,
  TopBar,
  useNarrow,
  useSidebarState,
} from './frame.js';
export { Layout } from './layout.js';
export {
  MODE_LABEL,
  ModeSwitcher,
  syncModeWithRoute,
  useMode,
  useModeShortcut,
} from './mode-switcher.js';

export { inMode, type ModeFilter, navSections, SECTIONS, STUDIO_SECTIONS } from './nav.js';
export {
  currentProject,
  DEFAULT_PINNED,
  isSettingsPage,
  PAGES_CHANGED_EVENT,
  type PageEntry,
  pageAt,
  pageEntries,
  pinnedPages,
  readPinned,
  SETTINGS_GROUPS,
  settingsHome,
  togglePinned,
  writePinned,
} from './pages.js';
export {
  isPaletteShortcut,
  type PaletteModes,
  type PaletteRow,
  paletteModeOf,
  type ScopedCommand,
  ShellPalette,
  scopePalette,
} from './palette.js';
export { DRAWER, INSPECTOR, type PanelConfig, PanelController } from './panel.js';
export { PreferencesPage } from './preferences.js';
export { PanelRegion, PanelToggles, tabShows } from './regions.js';
export { compile, HashRouter, type HashSource, windowHash } from './router.js';
export { ShellSidebar } from './sidebar.js';

const plugin: PluginModule = {
  name: 'shell',
  inject: ['slots', 'session', 'flags', 'mode', 'theme'],
  apply(ctx: Context) {
    const slots = service(ctx, 'slots') as SlotRegistry;
    const session = service(ctx, 'session');
    const flags = service(ctx, 'flags');
    const mode = service(ctx, 'mode');
    const theme = service(ctx, 'theme');
    const router = new HashRouter(slots);
    ctx.effect(() => () => router.dispose(), 'router');
    ctx.provide('router', router);
    // A route of the other mode switches the mode first (§53 §4.4).
    ctx.effect(() => syncModeWithRoute(router, mode), 'mode-route');
    // The right inspector and the bottom drawer (D1078): `inspector.open({ tab, props })`.
    const inspector = new PanelController(INSPECTOR);
    const drawer = new PanelController(DRAWER);
    ctx.provide('inspector', inspector);
    ctx.provide('drawer', drawer);
    ctx.effect(() =>
      slots.register({ name: 'root', plugin: 'shell' }, () => (
        <Layout
          router={router}
          session={session}
          flags={flags}
          mode={mode}
          theme={theme}
          inspector={inspector}
          drawer={drawer}
        />
      )),
    );
    // Preferences (T2-5): the settings sections where the desktop's Settings page is absent.
    ctx.effect(() =>
      router.page(
        { id: PREFERENCES_ID, path: '/preferences', title: 'Preferences', plugin: 'shell' },
        () => <PreferencesPage />,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'shell',
          order: 130,
          meta: {
            id: PREFERENCES_ID,
            label: 'Preferences',
            href: '/preferences',
            icon: 'settings',
            group: 'Settings',
          },
        },
        () => null,
      ),
    );
    // Settings › Appearance: the theme choice and panel motion (D1074, D1076).
    ctx.effect(() =>
      slots.register(
        {
          name: 'console.settings.section',
          plugin: 'shell',
          order: 5,
          meta: { id: 'appearance', label: 'Appearance' },
        },
        () => <AppearanceSection theme={theme} />,
      ),
    );
  },
};

export default plugin;
