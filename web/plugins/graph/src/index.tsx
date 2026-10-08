// @loams/plugin-graph: the Graph page. Today an empty state (Loams Graph is GQL over
// loams.graph.v1, served by the loams engine behind its `graph` feature); it notices when the
// connected instance lists the API in GetInstance and says the editor is coming in GR1a.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { GraphPage } from './graph-page.js';

export { GRAPH_API, GraphPage } from './graph-page.js';

const plugin: PluginModule = {
  name: 'graph',
  inject: ['desktop', 'router', 'slots', 'flags'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const flags = service(ctx, 'flags');
    ctx.effect(() =>
      router.page({ id: 'graph', path: '/graph', title: 'Graph', plugin: 'graph' }, () => (
        <GraphPage desktop={desktop} flags={flags} />
      )),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'graph',
          order: 90,
          meta: { id: 'graph', label: 'Graph', href: '/graph', icon: 'graph', group: 'Integrate' },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
