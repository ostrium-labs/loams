// @loams/plugin-durable: Loams Desktop's durable execution page. It talks to
// the local engine's Resonate server through `platform.fetch` on `/durable/`
// (the desktop protocol proxy routes that prefix to the engine).

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { createDurableApi, envelope } from './envelope.js';
import { DurablePage } from './pages.js';

export { createDurableApi, decodeValue, EnvelopeError, envelope } from './envelope.js';
export { DurablePage } from './pages.js';
export { buildRuns } from './runs.js';

const plugin: PluginModule = {
  name: 'durable',
  inject: ['desktop', 'router', 'slots', 'platform'],
  apply(ctx: Context) {
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const platform = service(ctx, 'platform');
    const api = createDurableApi(envelope((...a) => platform.fetch(...a), platform.baseUrl));
    ctx.effect(() =>
      router.page({ id: 'durable', path: '/durable', title: 'Durable', plugin: 'durable' }, () => (
        <DurablePage api={api} />
      )),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'durable',
          order: 60,
          meta: {
            id: 'durable',
            label: 'Durable',
            href: '/durable',
            icon: 'durable',
            group: 'Compute',
          },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
