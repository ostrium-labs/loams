// @loams/plugin-factory: Loams Desktop's Software Factory hub. A grid of the
// factory apps, a configure form per app, and native panels fed by
// `desktop.factory.query`.
//
// It lives in Studio (SD1 Task 27; §53 §5.9, D916): its pages are under
// `/studio/factory`, its nav entry is in Studio's Factory group, and the old
// `/factory` paths redirect there. The plugin is in both modes only for its Console
// overview card (`meta.mode: "console"`) and for the redirects, which must answer
// in Console too. Desktop-only until SF1's web half (D922).

import { type PluginModule, type RouterService, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import type { FactoryAppId } from '@loams/desktop/contracts';
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

const plugin: PluginModule = {
  name: 'factory',
  inject: ['desktop', 'router', 'slots', 'transport'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
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
      'factory-embedded': (p) => <EmbeddedApp desktop={desktop} app={app(p)} navigate={navigate} />,
      'factory-configure': (p) => (
        <ConfigurePage desktop={desktop} app={app(p)} navigate={navigate} />
      ),
      'factory-app': (p) => <PanelsPage desktop={desktop} app={app(p)} navigate={navigate} />,
      factory: () => <FactoryHome desktop={desktop} navigate={navigate} />,
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
