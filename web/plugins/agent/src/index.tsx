// @loams/plugin-agent: the agent chat panel in the right dock (D675) and the
// "Agent providers" section of Settings. The loop, the tools and the keys
// live in the main process; this plugin is the panel.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { AgentPanel } from './panel.js';
import { ProvidersSection } from './providers.js';
import { AgentStore } from './store.js';

export { renderMarkdown, safeHref } from './markdown.js';
export { AgentPanel, ApprovalCard, Markdown, stopLabel, ToolCard } from './panel.js';
export { ProvidersSection } from './providers.js';
export { AgentStore, applyEvent, itemsFromView } from './store.js';

/** "The user is viewing Postgres. Route: /postgres/branches. Active namespace: default." */
export function contextHint(
  location: { path: string },
  title: string | undefined,
  namespace: string | undefined,
): string {
  const parts = [`The user is viewing ${title ? `${title} ` : ''}(route ${location.path}).`];
  if (namespace) parts.push(`Active namespace: ${namespace}.`);
  return parts.join(' ');
}

const plugin: PluginModule = {
  name: 'agent',
  inject: ['desktop', 'router', 'session', 'slots'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const session = service(ctx, 'session');
    const slots = service(ctx, 'slots');
    const store = new AgentStore({
      desktop,
      context: () => {
        const title = document.title.replace(/\s*·\s*Loams$/, '').trim();
        return contextHint(
          router.current(),
          title && title !== 'Loams' ? title : undefined,
          session.environment()?.namespace,
        );
      },
    });
    ctx.effect(() => () => store.dispose(), 'agent-store');
    ctx.effect(() =>
      slots.register({ name: 'shell.dock.right', plugin: 'agent' }, () => (
        <AgentPanel store={store} desktop={desktop} />
      )),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'console.settings.section',
          plugin: 'agent',
          order: 15,
          meta: { id: 'agent', label: 'Agent providers' },
        },
        () => <ProvidersSection desktop={desktop} store={store} />,
      ),
    );
  },
};

export default plugin;
