// @loams/plugin-shell: the layout (the `root` slot), the navigation (from
// `console.nav`), the page outlet (`console.page`) and the `router` service
// (§37 §5.4–§5.5). Components get props and closures, never `ctx`.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import type { SlotRegistry } from '@loams/slots';
import { Layout } from './layout.js';
import { HashRouter } from './router.js';

export { Layout, navSections, SECTIONS } from './layout.js';

export { compile, HashRouter, type HashSource, windowHash } from './router.js';

const plugin: PluginModule = {
  name: 'shell',
  inject: ['slots', 'session'],
  apply(ctx: Context) {
    const slots = service(ctx, 'slots') as SlotRegistry;
    const session = service(ctx, 'session');
    const router = new HashRouter(slots);
    ctx.effect(() => () => router.dispose(), 'router');
    ctx.provide('router', router);
    ctx.effect(() =>
      slots.register({ name: 'root', plugin: 'shell' }, () => (
        <Layout router={router} session={session} />
      )),
    );
  },
};

export default plugin;
