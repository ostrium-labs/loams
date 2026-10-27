// @loams/plugin-durable: the durable execution page. It talks to the engine's
// Resonate server through `platform.fetch` on `/durable/`. In Loams Desktop the
// protocol proxy routes that prefix to the local engine's durable listener on This
// computer, and to the active server's main port elsewhere; in a browser it is the
// engine's main port. The main port serves the same API under `/durable/` behind
// its authentication (B12). An engine without it answers 404, which the page shows.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import { createDurableApi, envelope } from './envelope.js';
import { mockRunSource } from './graph/sample.js';
import { promiseRunSource, withSamples } from './graph/source.js';
import { DurablePage } from './pages.js';

export { createDurableApi, decodeValue, EnvelopeError, envelope } from './envelope.js';
export { ExecutionGraph } from './graph/ExecutionGraph.js';
export { GraphTab } from './graph/GraphTab.js';
export {
  applyEvent,
  fromPromises,
  type RunEvent,
  type RunGraph,
  type Step,
} from './graph/model.js';
export { mockMetricStream, mockRunSource } from './graph/sample.js';
export { promiseRunSource, type RunGraphSource, withSamples } from './graph/source.js';
export { DurablePage } from './pages.js';
export { buildRuns } from './runs.js';

const plugin: PluginModule = {
  name: 'durable',
  inject: ['router', 'slots', 'platform'],
  apply(ctx: Context) {
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const platform = service(ctx, 'platform');
    const api = createDurableApi(envelope((...a) => platform.fetch(...a), platform.baseUrl));
    // The engine's runs, plus fixture-backed sample runs for what it does not serve yet.
    const graph = withSamples(promiseRunSource(api), mockRunSource());
    ctx.effect(() =>
      router.page({ id: 'durable', path: '/durable', title: 'Durable', plugin: 'durable' }, () => (
        <DurablePage api={api} graph={graph} />
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
