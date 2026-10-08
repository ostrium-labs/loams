// @loams/plugin-desktop-servers: Loams Desktop's server switcher (header),
// the Servers page under Settings, and the local engine card.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { OverviewEngineCard } from './overview-card.js';
import { ServersPage } from './servers-page.js';
import { Switcher } from './switcher.js';

export { EngineCard } from './engine-card.js';
export { OverviewEngineCard } from './overview-card.js';
export { ServersPage } from './servers-page.js';
export { Switcher } from './switcher.js';

const plugin: PluginModule = {
  name: 'desktop-servers',
  inject: ['desktop', 'router', 'slots'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    ctx.effect(() =>
      router.page(
        {
          id: 'settings-servers',
          path: '/settings/servers',
          title: 'Servers',
          plugin: 'desktop-servers',
        },
        () => <ServersPage desktop={desktop} />,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'desktop-servers',
          order: 120,
          meta: {
            id: 'settings',
            label: 'Settings',
            href: '/settings/servers',
            icon: 'settings',
            group: 'Organisation',
          },
        },
        () => null,
      ),
    );
    ctx.effect(() =>
      slots.register({ name: 'shell.header.server', plugin: 'desktop-servers' }, () => (
        <Switcher desktop={desktop} navigate={(p) => router.navigate(p)} />
      )),
    );
    ctx.effect(() =>
      slots.register(
        { name: 'environment.overview.card', plugin: 'desktop-servers', order: 5 },
        () => <OverviewEngineCard desktop={desktop} />,
      ),
    );
  },
};

export default plugin;
