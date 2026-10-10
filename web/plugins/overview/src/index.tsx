// @loams/plugin-overview: Loams Desktop's home page (`/`). A grid of honest
// status cards, each linking to its page, plus whatever other plugins put in
// the `environment.overview.card` slot. Read-only.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { OverviewPage } from './overview-page.js';

export * from './cards.js';
export * from './load.js';
export { OverviewPage } from './overview-page.js';

const plugin: PluginModule = {
  name: 'overview',
  inject: ['desktop', 'router', 'slots', 'platform', 'transport'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const platform = service(ctx, 'platform');
    const transport = service(ctx, 'transport');
    const net = {
      fetch: (...a: Parameters<typeof fetch>) => platform.fetch(...a),
      baseUrl: platform.baseUrl,
    };
    ctx.effect(() =>
      router.page({ id: 'overview', path: '/', title: 'Overview', plugin: 'overview' }, () => (
        <OverviewPage desktop={desktop} transport={transport} net={net} />
      )),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'overview',
          order: 10,
          meta: { id: 'overview', label: 'Overview', href: '/', icon: 'overview', group: 'Data' },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
