// @loams/plugin-desktop-settings: Loams Desktop's Settings area. It owns the
// Settings nav entry, the `/settings` pages and the sub-navigation, and fills
// it with the sections that need only the `desktop` bridge: Local stacks,
// Updates and About. Other plugins add their section through the
// `console.settings.section` slot (Servers, Agent providers); this plugin
// does not know them.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { AboutSection } from './about.js';
import { SettingsPage } from './settings-page.js';
import { StacksSection } from './stacks.js';
import { UpdatesSection } from './updates.js';

export { AboutSection } from './about.js';
export { SettingsPage } from './settings-page.js';
export { StacksSection, useStacks } from './stacks.js';
export { UpdatesSection } from './updates.js';

const plugin: PluginModule = {
  name: 'desktop-settings',
  inject: ['desktop', 'router', 'slots'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    // The router takes the first match; neither path overlaps the other.
    ctx.effect(() =>
      router.page(
        { id: 'settings', path: '/settings', title: 'Settings', plugin: 'desktop-settings' },
        () => <SettingsPage />,
      ),
    );
    ctx.effect(() =>
      router.page(
        {
          id: 'settings-section',
          path: '/settings/:section',
          title: 'Settings',
          plugin: 'desktop-settings',
        },
        ({ params }) => <SettingsPage section={params.section} />,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'desktop-settings',
          order: 120,
          meta: {
            id: 'settings',
            label: 'Settings',
            href: '/settings',
            icon: 'settings',
            group: 'Organisation',
          },
        },
        () => null,
      ),
    );
    const sections = [
      {
        id: 'stacks',
        label: 'Local stacks',
        order: 20,
        node: () => <StacksSection desktop={desktop} />,
      },
      {
        id: 'updates',
        label: 'Updates',
        order: 30,
        node: () => <UpdatesSection desktop={desktop} />,
      },
      { id: 'about', label: 'About', order: 40, node: () => <AboutSection desktop={desktop} /> },
    ];
    for (const s of sections) {
      ctx.effect(() =>
        slots.register(
          {
            name: 'console.settings.section',
            plugin: 'desktop-settings',
            order: s.order,
            meta: { id: s.id, label: s.label },
          },
          s.node,
        ),
      );
    }
  },
};

export default plugin;
