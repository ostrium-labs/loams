// @loams/plugin-connectors: the connector catalog (generated at build time from
// connectors/registry and connectors/schemas), a detail page per connector, and a configure form
// generated from its JSON Schema that exports instance YAML. Secrets are never stored.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { CatalogPage } from './catalog-page.js';
import { DetailPage } from './detail-page.js';

export { CatalogPage } from './catalog-page.js';
export { ConfigureForm } from './configure-form.js';
export { DetailPage } from './detail-page.js';
export * from './form.js';

const plugin: PluginModule = {
  name: 'connectors',
  inject: ['desktop', 'router', 'slots'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const navigate = (to: string) => router.navigate(to);
    // The router takes the first match: the longer path goes first.
    ctx.effect(() =>
      router.page(
        { id: 'connector', path: '/connectors/:id', title: 'Connector', plugin: 'connectors' },
        ({ params }) => (
          <DetailPage desktop={desktop} id={params.id as string} navigate={navigate} />
        ),
      ),
    );
    ctx.effect(() =>
      router.page(
        { id: 'connectors', path: '/connectors', title: 'Connectors', plugin: 'connectors' },
        () => <CatalogPage desktop={desktop} navigate={navigate} />,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'connectors',
          order: 80,
          meta: {
            id: 'connectors',
            label: 'Connectors',
            href: '/connectors',
            icon: 'connectors',
            group: 'Integrate',
          },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
