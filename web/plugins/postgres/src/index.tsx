// @loams/plugin-postgres: the local Neon stack page (branches, connect, SQL).

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { PostgresPage } from './page.js';

export { Branches } from './branches.js';
export { PostgresPage } from './page.js';
export { flattenTimelines } from './tree.js';

const plugin: PluginModule = {
  name: 'postgres',
  inject: ['desktop', 'router', 'slots'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    ctx.effect(() =>
      router.page(
        { id: 'postgres', path: '/postgres', title: 'Postgres', plugin: 'postgres' },
        () => <PostgresPage desktop={desktop} />,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'postgres',
          order: 30,
          meta: {
            id: 'postgres',
            label: 'Postgres',
            href: '/postgres',
            icon: 'postgres',
            group: 'Data',
          },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
