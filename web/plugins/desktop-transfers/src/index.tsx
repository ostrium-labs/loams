// @loams/plugin-desktop-transfers: Loams Desktop's Transfers page (design §85,
// DQ1 Task 6, preview). Resumable uploads and downloads of large files and
// remote job runs with live logs, through the desktop's transfer sidecar.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { TransfersPage } from './TransfersPage.js';

export * from './format.js';
export { JobRow, LogView, TransfersPage } from './TransfersPage.js';

const plugin: PluginModule = {
  name: 'desktop-transfers',
  inject: ['desktop', 'router', 'slots'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    ctx.effect(() =>
      router.page(
        { id: 'transfers', path: '/transfers', title: 'Transfers', plugin: 'desktop-transfers' },
        () => <TransfersPage desktop={desktop} />,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'desktop-transfers',
          order: 86,
          meta: {
            id: 'transfers',
            label: 'Transfers',
            href: '/transfers',
            icon: 'transfers',
            group: 'Compute',
          },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
