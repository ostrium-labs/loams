// @loams/plugin-engines: managed RisingWave, Flink SQL and Spark (design 81, preview).
// The pages talk to the namespace-scoped engines API (`/v1/namespaces/{ns}/engines`, served by
// crates/loams-engines) over the active server's `platform.fetch`. When the server does not
// serve it (`GET /v1/engines` fails), the pages run on the sample client and say so.

import {
  type PageProps,
  type PlatformService,
  type PluginModule,
  service,
} from '@loams/console-host';
import type { Context } from '@loams/cordis';
import { SkeletonList } from '@loams/ui';
import { useEffect, useState } from 'react';
import { createEnginesClient, type EnginesClient } from './client.js';
import { createMockEnginesClient } from './mock.js';
import { CreatePage } from './pages/create.js';
import { DetailPage } from './pages/detail.js';
import { ListPage } from './pages/list.js';
import { SparkPage } from './pages/spark.js';
import { SqlPage } from './pages/sql.js';

export * from './client.js';
export {
  createMockEnginesClient,
  type MockOptions,
  sampleCatalog,
  sampleInstances,
} from './mock.js';
export { CreatePage } from './pages/create.js';
export { DetailPage, UsageView } from './pages/detail.js';
export { ListPage } from './pages/list.js';
export { SparkPage } from './pages/spark.js';
export { SQL_MAX_ROWS, SqlPage, toOutcome } from './pages/sql.js';
export { ENGINES_PACKAGE, PreviewBadge } from './shared.js';

type View = 'list' | 'new' | 'detail' | 'sql' | 'spark';

const detected = new WeakMap<object, Promise<EnginesClient>>();

/** The server's client when it serves the engines API, else the sample client. */
export function detectClient(
  platform: Pick<PlatformService, 'fetch' | 'baseUrl'>,
): Promise<EnginesClient> {
  let p = detected.get(platform);
  if (!p) {
    const real = createEnginesClient(platform.fetch, platform.baseUrl);
    p = real.catalog().then(
      (c) => (Array.isArray(c?.engines) ? real : createMockEnginesClient()),
      () => createMockEnginesClient(),
    );
    detected.set(platform, p);
  }
  return p;
}

export function EnginesRoute({
  platform,
  navigate,
  params,
  view,
}: {
  platform: Pick<PlatformService, 'fetch' | 'baseUrl' | 'clipboardWrite'>;
  navigate: (to: string) => void;
  params: PageProps['params'];
  view: View;
}) {
  const [client, setClient] = useState<EnginesClient | undefined>();
  useEffect(() => {
    let live = true;
    detectClient(platform).then((c) => live && setClient(c));
    return () => {
      live = false;
    };
  }, [platform]);
  if (!client) {
    return (
      <div className="lc-page">
        <SkeletonList label="Asking the server about managed engines" rows={3} />
      </div>
    );
  }
  const namespace = params.ns ?? 'default';
  const id = params.id ?? '';
  switch (view) {
    case 'new':
      return <CreatePage client={client} navigate={navigate} />;
    case 'detail':
      return (
        <DetailPage
          key={`${namespace}/${id}`}
          client={client}
          navigate={navigate}
          namespace={namespace}
          id={id}
          copy={(t) => platform.clipboardWrite(t)}
        />
      );
    case 'sql':
      return (
        <SqlPage
          key={`${namespace}/${id}`}
          client={client}
          navigate={navigate}
          namespace={namespace}
          id={id}
        />
      );
    case 'spark':
      return (
        <SparkPage
          key={`${namespace}/${id}`}
          client={client}
          navigate={navigate}
          namespace={namespace}
          id={id}
        />
      );
    default:
      return <ListPage client={client} navigate={navigate} />;
  }
}

const plugin: PluginModule = {
  name: 'engines',
  inject: ['router', 'slots', 'platform'],
  apply(ctx: Context) {
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const platform = service(ctx, 'platform');
    const navigate = (to: string) => router.navigate(to);
    const routes: { id: string; path: string; title: string; view: View }[] = [
      { id: 'engines', path: '/engines', title: 'Engines', view: 'list' },
      { id: 'engines-new', path: '/engines/new', title: 'New engine instance', view: 'new' },
      { id: 'engines-detail', path: '/engines/:ns/:id', title: 'Engine', view: 'detail' },
      { id: 'engines-sql', path: '/engines/:ns/:id/sql', title: 'Engine SQL', view: 'sql' },
      {
        id: 'engines-spark',
        path: '/engines/:ns/:id/spark',
        title: 'Spark sessions and jobs',
        view: 'spark',
      },
    ];
    for (const r of routes) {
      ctx.effect(() =>
        router.page({ id: r.id, path: r.path, title: r.title, plugin: 'engines' }, ({ params }) => (
          <EnginesRoute platform={platform} navigate={navigate} params={params} view={r.view} />
        )),
      );
    }
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'engines',
          order: 72,
          meta: {
            id: 'engines',
            label: 'Engines',
            href: '/engines',
            icon: 'engines',
            group: 'Compute',
          },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
