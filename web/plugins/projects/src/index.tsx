// @loams/plugin-projects: a project and its environments on loams.admin.v1 (API2
// Task 10, AP1a Task 5), as the original React console had them: the project's
// overview, an environment (stats, 30-day activity, collections, API keys and the
// Connect snippet) and the project's access; the top bar's project pill
// (`shell.topbar.left`); and the project's Overview, Agents and Access as pages
// (`shell.nav.section`, design 59 W20).

import { type PageProps, type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import type { ReactNode } from 'react';
import { AccessPage } from './access.js';
import { ProjectPill } from './context.js';
import { EnvironmentPage } from './environment.js';
import { ProjectPage } from './project.js';

export { LAST_PROJECT_KEY, projectOf } from './context.js';
export { connectSnippet } from './environment.js';

const plugin: PluginModule = {
  name: 'projects',
  inject: [
    'router',
    'slots',
    'session',
    'flags',
    'platform',
    'rpc.org',
    'rpc.projects',
    'rpc.agents',
    'rpc.keys',
    'rpc.audit',
  ],
  apply(ctx: Context) {
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const session = service(ctx, 'session');
    const platform = service(ctx, 'platform');
    const page = (
      spec: { id: string; path: string; title: string },
      render: (params: Record<string, string>) => ReactNode,
    ) =>
      ctx.effect(() =>
        router.page({ ...spec, plugin: 'projects' }, ({ params }: PageProps) => render(params)),
      );

    page({ id: 'project', path: '/projects/:project', title: 'Project' }, (params) => (
      <ProjectPage
        project={params.project ?? ''}
        router={router}
        session={session}
        auditPath="/audit"
      />
    ));
    page(
      {
        id: 'environment',
        path: '/projects/:project/environments/:environment',
        title: 'Environment',
      },
      (params) => (
        <EnvironmentPage
          project={params.project ?? ''}
          environment={params.environment ?? ''}
          router={router}
          session={session}
          server={platform.serverUrl ?? platform.baseUrl}
        />
      ),
    );
    page({ id: 'access', path: '/projects/:project/access', title: 'Access' }, (params) => (
      <AccessPage project={params.project ?? ''} router={router} session={session} />
    ));
    // W20: the switcher is the top bar's project pill, and the project's pages are pages.
    ctx.effect(() =>
      slots.register(
        { name: 'shell.topbar.left', plugin: 'projects', order: 0, meta: { mode: 'console' } },
        () => <ProjectPill router={router} />,
      ),
    );
    const nav = (
      id: string,
      label: string,
      href: string,
      icon: string,
      group: string,
      order: number,
    ) =>
      ctx.effect(() =>
        slots.register(
          {
            name: 'shell.nav.section',
            plugin: 'projects',
            order,
            meta: { id, label, href, icon, group },
          },
          () => null,
        ),
      );
    nav('project-overview', 'Project overview', '/projects/:project', 'overview', 'Project', 1);
    nav('agents', 'Agents', '/projects/:project/agents', 'agents', 'Project', 2);
    nav('access', 'Access', '/projects/:project/access', 'access', 'Organisation', 3);
  },
};

export default plugin;
