// @loams/plugin-streams: Loams Desktop's streams and links page. Create and
// describe streams and links, produce a test record, tail a partition and
// watch a link's lag, against the active server's native REST routes.

import {
  type PageProps,
  type PlatformService,
  type PluginModule,
  service,
} from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { useMemo } from 'react';
import { createStreamsClient } from './client.js';
import { LinkPage } from './pages/link.js';
import { ListPage, type ListTab } from './pages/list.js';
import { StreamPage } from './pages/stream.js';

export { createStreamsClient } from './client.js';

type View = ListTab | 'stream' | 'link';

export function StreamsRoute({
  platform,
  navigate,
  params,
  view,
}: {
  platform: Pick<PlatformService, 'fetch' | 'baseUrl'>;
  navigate: (to: string) => void;
  params: PageProps['params'];
  view: View;
}) {
  const client = useMemo(() => createStreamsClient(platform.fetch, platform.baseUrl), [platform]);
  const ns = params.ns || 'default';
  if (view === 'stream')
    return <StreamPage client={client} ns={ns} name={params.name ?? ''} navigate={navigate} />;
  if (view === 'link')
    return <LinkPage client={client} ns={ns} name={params.name ?? ''} navigate={navigate} />;
  return <ListPage key={`${ns}/${view}`} client={client} ns={ns} tab={view} navigate={navigate} />;
}

const plugin: PluginModule = {
  name: 'streams',
  inject: ['desktop', 'router', 'slots', 'platform'],
  apply(ctx: Context) {
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const platform = service(ctx, 'platform');
    const navigate = (to: string) => router.navigate(to);
    const routes: { id: string; path: string; title: string; view: View }[] = [
      { id: 'streams', path: '/streams', title: 'Streams & Links', view: 'streams' },
      { id: 'streams-ns', path: '/streams/:ns', title: 'Streams & Links', view: 'streams' },
      { id: 'streams-links', path: '/streams/:ns/links', title: 'Links', view: 'links' },
      { id: 'streams-stream', path: '/streams/:ns/stream/:name', title: 'Stream', view: 'stream' },
      { id: 'streams-link', path: '/streams/:ns/link/:name', title: 'Link', view: 'link' },
    ];
    for (const r of routes) {
      ctx.effect(() =>
        router.page({ id: r.id, path: r.path, title: r.title, plugin: 'streams' }, ({ params }) => (
          <StreamsRoute platform={platform} navigate={navigate} params={params} view={r.view} />
        )),
      );
    }
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'streams',
          order: 70,
          meta: {
            id: 'streams',
            label: 'Streams & Links',
            href: '/streams',
            icon: 'streams',
            group: 'Compute',
          },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
