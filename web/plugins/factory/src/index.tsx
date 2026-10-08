// @loams/plugin-factory: Loams Desktop's Software Factory hub. A grid of the
// factory apps, a configure form per app, and native panels fed by
// `desktop.factory.query`.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import type { FactoryAppId } from '@loams/desktop/contracts';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { ConfigurePage } from './configure.js';
import { FactoryHome } from './home.js';
import { PanelsPage } from './panels-page.js';
import { SummaryCard } from './summary-card.js';

export { ConfigurePage } from './configure.js';
export { FactoryHome } from './home.js';
export { PanelsPage } from './panels-page.js';
export { SummaryCard } from './summary-card.js';

const plugin: PluginModule = {
  name: 'factory',
  inject: ['desktop', 'router', 'slots'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const navigate = (to: string) => router.navigate(to);
    // The router takes the first match: the longer path goes first.
    ctx.effect(() =>
      router.page(
        {
          id: 'factory-configure',
          path: '/factory/:app/configure',
          title: 'Configure',
          plugin: 'factory',
        },
        ({ params }) => (
          <ConfigurePage desktop={desktop} app={params.app as FactoryAppId} navigate={navigate} />
        ),
      ),
    );
    ctx.effect(() =>
      router.page(
        { id: 'factory-app', path: '/factory/:app', title: 'Software Factory', plugin: 'factory' },
        ({ params }) => (
          <PanelsPage desktop={desktop} app={params.app as FactoryAppId} navigate={navigate} />
        ),
      ),
    );
    ctx.effect(() =>
      router.page(
        { id: 'factory', path: '/factory', title: 'Software Factory', plugin: 'factory' },
        () => <FactoryHome desktop={desktop} navigate={navigate} />,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'factory',
          order: 100,
          meta: {
            id: 'factory',
            label: 'Software Factory',
            href: '/factory',
            icon: 'factory',
            group: 'Integrate',
          },
        },
        () => null,
      ),
    );
    ctx.effect(() =>
      slots.register({ name: 'environment.overview.card', plugin: 'factory', order: 50 }, () => (
        <SummaryCard desktop={desktop} />
      )),
    );
  },
};

export default plugin;
