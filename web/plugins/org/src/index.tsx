// @loams/plugin-org: the org's pages on loams.admin.v1 (API2 Task 10, AP1a Task 5):
// its overview with every project (`/`), Teams, a team, Members and Settings, as the
// original React console had them, inside the cordis shell. The pages read the API
// with Connect Query over the console's transport; the plugin injects the `rpc.*`
// clients so it appears only where the instance serves loams.admin.v1 (Ruling 6).
//
// On the desktop, `/` and `/settings` are the desktop's own Overview and Settings,
// so the org's overview and settings move to `/org` and `/org/settings` there.

import { type PageProps, type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import type { ReactNode } from 'react';
import { Home } from './home.js';
import { MembersPage } from './members.js';
import { SettingsPage } from './settings.js';
import { TeamPage, TeamsPage } from './teams.js';

/** Where the org's overview and settings live: the root, except on the desktop. */
export function orgPaths(kind: 'web' | 'desktop') {
  return kind === 'desktop'
    ? { home: '/org', settings: '/org/settings' }
    : { home: '/', settings: '/settings' };
}

const plugin: PluginModule = {
  name: 'org',
  inject: [
    'router',
    'slots',
    'session',
    'flags',
    'platform',
    'rpc.org',
    'rpc.projects',
    'rpc.agents',
    'rpc.audit',
  ],
  apply(ctx: Context) {
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const session = service(ctx, 'session');
    const flags = service(ctx, 'flags');
    const paths = orgPaths(service(ctx, 'platform').kind);

    const page = (
      spec: { id: string; path: string; title: string },
      render: (params: Record<string, string>) => ReactNode,
    ) =>
      ctx.effect(() =>
        router.page({ ...spec, plugin: 'org' }, ({ params }: PageProps) => render(params)),
      );
    const nav = (id: string, label: string, href: string, icon: string, order: number) =>
      ctx.effect(() =>
        slots.register(
          {
            name: 'shell.nav.section',
            plugin: 'org',
            order,
            meta: { id, label, href, icon, group: 'Organisation' },
          },
          () => null,
        ),
      );

    page({ id: 'org-home', path: paths.home, title: 'Organization' }, () => (
      <Home router={router} session={session} auditPath="/audit" />
    ));
    page({ id: 'teams', path: '/teams', title: 'Teams' }, () => (
      <TeamsPage router={router} session={session} />
    ));
    page({ id: 'team', path: '/teams/:team', title: 'Team' }, (params) => (
      <TeamPage team={params.team ?? ''} router={router} session={session} />
    ));
    page({ id: 'members', path: '/members', title: 'Members' }, () => (
      <MembersPage router={router} session={session} />
    ));
    page({ id: 'org-settings', path: paths.settings, title: 'Settings' }, () => (
      <SettingsPage router={router} session={session} flags={flags} />
    ));

    // The original sidebar's organization half: All projects, Teams, Members, (the
    // audit plugin's Audit log), Settings.
    nav('org-home', 'All projects', paths.home, 'projects', 1);
    nav('teams', 'Teams', '/teams', 'teams', 2);
    nav('members', 'Members', '/members', 'members', 3);
    // Settings lists every settings page (D1080), so this one says whose it is.
    nav('org-settings', 'Organisation settings', paths.settings, 'settings', 90);
  },
};

export default plugin;
