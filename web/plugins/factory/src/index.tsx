// @loams/plugin-factory: Loams Desktop's Software Factory hub. A grid of the
// factory apps, a configure form per app, and native panels fed by
// `desktop.factory.query`.
//
// It lives in Studio (SD1 Task 27; §53 §5.9, D916): its pages are under
// `/studio/factory`, its nav entry is in Studio's Factory group, and the old
// `/factory` paths redirect there. The plugin is in both modes only for its Console
// overview card (`meta.mode: "console"`) and for the redirects, which must answer
// in Console too.
//
// On the desktop it reads through `desktop.factory` (Electron main, a client of
// loams-agentd) and embeds the apps (D678). In a browser it reads through
// `studio.factory`, Studio's bridge to the selected agentd host: the panels work,
// an app opens in a new tab, and credentials are configured on the desktop (SF1
// amendment F1-F5, amending D922).

import { type PluginModule, type RouterService, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import type { FactoryAppId, IpcResult, LoamsDesktopApi } from '@loams/desktop/contracts';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { type ReactNode, useEffect } from 'react';
import { ConfigurePage } from './configure.js';
import { EmbeddedApp } from './embedded.js';
import { FactoryHome } from './home.js';
import { PanelsPage } from './panels-page.js';
import { FACTORY_HOME, LEGACY_PATHS, legacyTarget, PAGES, RUNS_PATH } from './paths.js';
import { RunsPage } from './runs/runs-page.js';
import { liveRunsSource, type RunsSource } from './runs/source.js';
import { SummaryCard } from './summary-card.js';

export { ConfigurePage } from './configure.js';
export {
  branchFor,
  draftFromGlitchtipIssue,
  draftFromPlaneIssue,
  type ThreadDraft,
  threadDraftHref,
} from './drafts.js';
export { EmbeddedApp } from './embedded.js';
export { FactoryHome } from './home.js';
export { PanelsPage } from './panels-page.js';
export { FACTORY_HOME, factoryPath, legacyTarget, RUNS_PATH } from './paths.js';
export { POLL_MS, RunDetail, RunsPage } from './runs/runs-page.js';
export { FACTORY_LIVE_APP, liveRunsSource, type RunRow, type RunsSource } from './runs/source.js';
export { SummaryCard } from './summary-card.js';

/** An old `/factory…` route: replaces itself with its `/studio/factory…` route. */
function Redirect({ router }: { router: RouterService }) {
  useEffect(() => {
    const { path, query } = router.current();
    const to = legacyTarget(path, query);
    if (to) router.navigate(to, { replace: true });
  }, [router]);
  return null;
}

/** `studio.factory`: Studio's bridge to the agentd host's FactoryService (web). */
type WebBridge = Pick<
  LoamsDesktopApi['factory'],
  'list' | 'configure' | 'test' | 'remove' | 'query' | 'openApp'
>;

const unsupported: IpcResult<void> = {
  ok: false,
  code: 'unsupported',
  message: 'A browser opens an app in a new tab',
};

/** The browser's bridge in the desktop bridge's shape: no embedding, new tabs. */
function webDesktop(
  bridge: WebBridge,
  openExternal: (url: string) => Promise<void>,
): LoamsDesktopApi {
  return {
    factory: {
      ...bridge,
      closeApp: async () => {},
      showEmbedded: async () => unsupported,
      hideEmbedded: async () => {},
      reloadEmbedded: async () => {},
      popOut: (app: FactoryAppId) => bridge.openApp(app),
    },
    shell: {
      openExternal: async (url: string) => {
        await openExternal(url);
        return { ok: true, value: undefined };
      },
    },
  } as unknown as LoamsDesktopApi;
}

const plugin: PluginModule = {
  name: 'factory',
  // The catalog narrows this per edition: `desktop` on the desktop, `studio.factory`
  // in a browser.
  inject: ['desktop', 'studio.factory', 'platform', 'router', 'slots', 'transport'],
  apply(ctx: Context) {
    const platform = service(ctx, 'platform');
    const embeds = platform.kind === 'desktop';
    const desktop = embeds
      ? service(ctx, 'desktop')
      : webDesktop(
          (ctx as unknown as Record<string, WebBridge>)['studio.factory'] as WebBridge,
          (url) => platform.openExternal(url),
        );
    // The run record is in the Live app `factory`, read through the engine transport.
    let runs: RunsSource | undefined;
    const runsSource = (): RunsSource => {
      runs ??= liveRunsSource(service(ctx, 'transport'));
      return runs;
    };
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const navigate = (to: string) => router.navigate(to);
    const app = (params: Record<string, string>) => params.app as FactoryAppId;
    const view: Record<(typeof PAGES)[number][1], (p: Record<string, string>) => ReactNode> = {
      'factory-embedded': (p) => (
        <EmbeddedApp desktop={desktop} app={app(p)} navigate={navigate} embeds={embeds} />
      ),
      'factory-configure': (p) => (
        <ConfigurePage desktop={desktop} app={app(p)} navigate={navigate} embeds={embeds} />
      ),
      'factory-app': (p) => (
        <PanelsPage desktop={desktop} app={app(p)} navigate={navigate} embeds={embeds} />
      ),
      factory: () => <FactoryHome desktop={desktop} navigate={navigate} embeds={embeds} />,
      'factory-runs': () => <RunsPage source={runsSource()} />,
    };
    // The router takes the first match: PAGES lists the longer paths first.
    for (const [suffix, id, title] of PAGES) {
      ctx.effect(() =>
        router.page(
          { id, path: `${FACTORY_HOME}${suffix}`, title, plugin: 'factory' },
          ({ params }) => view[id](params),
        ),
      );
    }
    LEGACY_PATHS.forEach((path, i) => {
      ctx.effect(() =>
        router.page(
          { id: `factory-legacy-${i}`, path, title: 'Software Factory', plugin: 'factory' },
          () => <Redirect router={router} />,
        ),
      );
    });
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'factory',
          order: 100,
          meta: {
            id: 'factory',
            label: 'Software Factory',
            href: FACTORY_HOME,
            icon: 'factory',
            group: 'Factory',
            mode: 'studio',
          },
        },
        () => null,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'factory',
          order: 101,
          meta: {
            id: 'factory-runs',
            label: 'Runs',
            href: RUNS_PATH,
            icon: 'pipelines',
            group: 'Factory',
            mode: 'studio',
          },
        },
        () => null,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'environment.overview.card',
          plugin: 'factory',
          order: 50,
          meta: { mode: 'console' },
        },
        () => <SummaryCard desktop={desktop} />,
      ),
    );
  },
};

export default plugin;
