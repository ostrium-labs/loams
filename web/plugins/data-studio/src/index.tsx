// @loams/plugin-data-studio: Loams Desktop's data browser. Namespaces,
// collections, documents, search, SQL and ingest, against any active server
// that serves loams.collection.v1.

import type { Transport } from '@connectrpc/connect';
import {
  type FlagsService,
  type PageProps,
  type PlatformService,
  type PluginModule,
  service,
} from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { Empty } from '@loams/ui';
import { useMemo } from 'react';
import { createDataClient } from './client.js';
import { CollectionPage } from './pages/collection.js';
import { CollectionsPage } from './pages/collections.js';
import { SqlPage } from './pages/sql.js';
import type { Tab } from './shared.js';

export { createDataClient } from './client.js';

export const DATA_PLANE_API = 'loams.collection.v1';

type View = 'collections' | 'sql' | Exclude<Tab, 'sql'>;

export function DataRoute({
  transport,
  platform,
  flags,
  navigate,
  params,
  view,
}: {
  transport: Transport;
  platform: Pick<PlatformService, 'fetch' | 'baseUrl'>;
  flags: Pick<FlagsService, 'has'>;
  navigate: (to: string) => void;
  params: PageProps['params'];
  view: View;
}) {
  const client = useMemo(
    () => createDataClient(transport, platform.fetch, platform.baseUrl),
    [transport, platform],
  );
  if (!flags.has(DATA_PLANE_API)) {
    return (
      <div className="lc-page ds-page">
        <Empty title="This server has no data plane.">
          Switch to a server that serves collections, such as This computer.
        </Empty>
      </div>
    );
  }
  const ns = params.ns || 'default';
  if (view === 'collections')
    return <CollectionsPage client={client} ns={ns} navigate={navigate} />;
  if (view === 'sql') return <SqlPage client={client} ns={ns} navigate={navigate} />;
  return (
    <CollectionPage
      client={client}
      ns={ns}
      coll={params.coll ?? ''}
      tab={view}
      navigate={navigate}
    />
  );
}

const plugin: PluginModule = {
  name: 'data-studio',
  inject: ['desktop', 'router', 'slots', 'transport', 'platform', 'flags'],
  apply(ctx: Context) {
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const transport = service(ctx, 'transport');
    const platform = service(ctx, 'platform');
    const flags = service(ctx, 'flags');
    const navigate = (to: string) => router.navigate(to);
    // The static `/sql` route is registered before `/:coll`, because the router
    // takes the first match: a collection named "sql" is not reachable by URL.
    const routes: { id: string; path: string; title: string; view: View }[] = [
      { id: 'data', path: '/data', title: 'Data', view: 'collections' },
      { id: 'data-ns', path: '/data/:ns', title: 'Data', view: 'collections' },
      { id: 'data-sql', path: '/data/:ns/sql', title: 'SQL', view: 'sql' },
      { id: 'data-coll', path: '/data/:ns/:coll', title: 'Collection', view: 'documents' },
      { id: 'data-search', path: '/data/:ns/:coll/search', title: 'Search', view: 'search' },
      { id: 'data-ingest', path: '/data/:ns/:coll/ingest', title: 'Ingest', view: 'ingest' },
      { id: 'data-schema', path: '/data/:ns/:coll/schema', title: 'Schema', view: 'schema' },
    ];
    for (const r of routes) {
      ctx.effect(() =>
        router.page(
          { id: r.id, path: r.path, title: r.title, plugin: 'data-studio' },
          ({ params }) => (
            <DataRoute
              transport={transport}
              platform={platform}
              flags={flags}
              navigate={navigate}
              params={params}
              view={r.view}
            />
          ),
        ),
      );
    }
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'data-studio',
          order: 20,
          meta: { id: 'data', label: 'Data', href: '/data', icon: 'data', group: 'Data' },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
