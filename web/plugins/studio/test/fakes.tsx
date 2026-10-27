// Fakes for Studio's tests (SD1 Task 6): a daemon's `ProjectService` as an
// in-memory Connect router, behind the desktop's real `agentd` endpoint (over the
// mock desktop bridge) or a web host.

import { create } from '@bufbuild/protobuf';
import { Code, ConnectError, createRouterTransport, type Transport } from '@connectrpc/connect';
import type { AgentdReach, AgentdService } from '@loams/console-host';
import { createQueryClient, ServerStateProvider } from '@loams/console-host/query';
import { desktopAgentdEndpoint } from '@loams/platform-electron';
import { createMockAgentdApi } from '@loams/plugin-desktop-agentd';
import { ProjectService, type Repo, RepoSchema } from '@loams/proto/agentd';
import type { ReactNode } from 'react';
import { vi } from 'vite-plus/test';
import { type StudioAgentd, studioAgentd } from '../src/agentd.js';

export function wrap(children: ReactNode) {
  const client = createQueryClient();
  client.setDefaultOptions({ queries: { retry: false } });
  return <ServerStateProvider client={client}>{children}</ServerStateProvider>;
}

type RepoInit = { path: string; name: string; defaultBranch?: string };

/** A daemon's `ProjectService`; `calls` records each RPC. */
export function fakeDaemon(initial: RepoInit[] = sample()) {
  const repos: Repo[] = initial.map((r) => create(RepoSchema, r));
  const calls: string[] = [];
  const transport: Transport = createRouterTransport(({ service }) => {
    service(ProjectService, {
      listRepos: () => {
        calls.push('ListRepos');
        return { repos };
      },
      addRepo: ({ path }) => {
        calls.push(`AddRepo:${path}`);
        const repo = create(RepoSchema, {
          path,
          name: path.split('/').pop() ?? path,
          defaultBranch: 'main',
        });
        repos.push(repo);
        return { repo };
      },
      cloneRepo: ({ url }) => {
        calls.push(`CloneRepo:${url}`);
        if (url.startsWith('-')) throw new ConnectError('invalid_url', Code.InvalidArgument);
        const name =
          url
            .replace(/\.git$/, '')
            .split(/[/:]/)
            .pop() ?? url;
        const repo = create(RepoSchema, { path: `/srv/src/${name}`, name, defaultBranch: 'main' });
        repos.push(repo);
        return { repo };
      },
    });
  });
  return { transport, calls, repos };
}

function sample(): RepoInit[] {
  return [
    { path: '~/src/loams', name: 'loams', defaultBranch: 'dev' },
    { path: '~/src/acme-site', name: 'acme-site', defaultBranch: 'main' },
  ];
}

/** Studio on the desktop: the real endpoint over the mock bridge and a fake daemon. */
export function desktopFake(
  o: { running?: boolean; folder?: string | null; repos?: RepoInit[] } = {},
) {
  const mock = createMockAgentdApi({ running: o.running ?? true, background: 'off' });
  const daemon = fakeDaemon(o.repos);
  const pickFolder = vi.fn(async () => (o.folder === undefined ? '/home/omar/src/site' : o.folder));
  const agentd = studioAgentd(desktopAgentdEndpoint({ agentd: mock.api }, daemon.transport), {
    pickFolder,
  });
  return { agentd, mock, daemon, pickFolder };
}

/** Studio in a browser: no host, or a connected one. */
export function webFake(o: { host?: { id: string; name: string }; repos?: RepoInit[] } = {}) {
  const daemon = fakeDaemon(o.repos);
  const reach: AgentdReach = o.host ? { state: 'ready' } : { state: 'no-host' };
  const service: AgentdService = {
    kind: 'web',
    reach: () => reach,
    subscribe: () => () => {},
    transport: () => (o.host ? daemon.transport : undefined),
    host: () => o.host,
  };
  const agentd: StudioAgentd = studioAgentd(service);
  return { agentd, daemon };
}
