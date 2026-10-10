// @loams/plugin-wesql: the local WeSQL (MySQL) dev stack page (schemas, connect, SQL).

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { WesqlPage } from './page.js';

export { WesqlPage } from './page.js';
export { Schemas } from './schemas.js';

const plugin: PluginModule = {
  name: 'wesql',
  inject: ['desktop', 'router', 'slots'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    ctx.effect(() =>
      router.page({ id: 'wesql', path: '/wesql', title: 'WeSQL', plugin: 'wesql' }, () => (
        <WesqlPage desktop={desktop} />
      )),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'wesql',
          order: 40,
          meta: {
            id: 'wesql',
            label: 'WeSQL',
            href: '/wesql',
            icon: 'wesql',
            group: 'Data',
          },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
