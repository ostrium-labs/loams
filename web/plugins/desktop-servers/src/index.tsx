// @loams/plugin-desktop-servers: Loams Desktop's server switcher (header) and
// the Servers section of Settings with the local engine card.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { ServersPage } from './servers-page.js';
import { Switcher } from './switcher.js';

export { EngineCard } from './engine-card.js';
export { ServersPage } from './servers-page.js';
export { Switcher } from './switcher.js';

const plugin: PluginModule = {
  name: 'desktop-servers',
  inject: ['desktop', 'router', 'slots'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    // The Servers section of Settings (the Settings area is @loams/plugin-desktop-settings).
    ctx.effect(() =>
      slots.register(
        {
          name: 'console.settings.section',
          plugin: 'desktop-servers',
          order: 10,
          meta: { id: 'servers', label: 'Servers' },
        },
        () => <ServersPage desktop={desktop} embedded />,
      ),
    );
    ctx.effect(() =>
      slots.register({ name: 'shell.header.server', plugin: 'desktop-servers' }, () => (
        <Switcher desktop={desktop} navigate={(p) => router.navigate(p)} />
      )),
    );
  },
};

export default plugin;
