// connect-query against loams-agentd (plan SD1 Task 6): `useAgentdQuery` reads the
// daemon over the agentd transport scope, through the console's one QueryClient.

import { createRouterTransport } from '@connectrpc/connect';
import { useQuery } from '@connectrpc/connect-query';
import { instance } from '@loams/proto';
import { ProjectService } from '@loams/proto/agentd';
import { useQueryClient } from '@tanstack/react-query';
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import type { AgentdReach, AgentdService } from '../src/agentd.js';
import {
  AgentdScope,
  createQueryClient,
  ServerStateProvider,
  useAgentdQuery,
} from '../src/query.js';
import { createMockTransport } from '../src/testing';

afterEach(cleanup);

function daemon(name: string) {
  const calls: string[] = [];
  const transport = createRouterTransport(({ service }) => {
    service(ProjectService, {
      listRepos: () => {
        calls.push('ListRepos');
        return { repos: [{ path: `/srv/${name}`, name }] };
      },
    });
  });
  return { transport, calls };
}

function Repos() {
  const repos = useAgentdQuery(ProjectService.method.listRepos, {});
  return <p>{repos.data ? repos.data.repos.map((r) => r.name).join(',') : repos.status}</p>;
}

describe('useAgentdQuery', () => {
  it('use_agentd_query_shares_query_client', async () => {
    const client = createQueryClient();
    const console = createMockTransport();
    const agentd = daemon('loams');
    let seen: unknown;
    function Both() {
      seen = useQueryClient();
      // The console's own transport still serves plain connect-query.
      const info = useQuery(instance.InstanceService.method.getInstance, {});
      return <p>{info.data ? `instance ${info.data.name}` : 'instance…'}</p>;
    }
    render(
      <ServerStateProvider client={client} transport={console} agentd={agentd.transport}>
        <Both />
        <Repos />
      </ServerStateProvider>,
    );
    expect(await screen.findByText('loams')).toBeTruthy();
    expect(await screen.findByText(/^instance /)).toBeTruthy();
    expect(seen).toBe(client);
    expect(agentd.calls).toEqual(['ListRepos']);
    // Both answers sit in the one cache, under different keys.
    const keys = client
      .getQueryCache()
      .getAll()
      .map((q) => JSON.stringify(q.queryKey));
    expect(keys.some((k) => k.includes('"methodName":"ListRepos"'))).toBe(true);
    expect(keys.some((k) => k.includes('"methodName":"GetInstance"'))).toBe(true);
  });

  it('waits for a daemon, and follows `agentd` as it comes', async () => {
    const d = daemon('site');
    let reach: AgentdReach = { state: 'stopped' };
    const listeners = new Set<() => void>();
    const agentd: AgentdService = {
      kind: 'desktop',
      reach: () => reach,
      subscribe: (l) => {
        listeners.add(l);
        return () => listeners.delete(l);
      },
      transport: () => (reach.state === 'ready' ? d.transport : undefined),
      host: () => undefined,
    };
    render(
      <ServerStateProvider client={createQueryClient()}>
        <AgentdScope agentd={agentd}>
          <Repos />
        </AgentdScope>
      </ServerStateProvider>,
    );
    // Skipped, not failed, while no daemon is reachable.
    expect(await screen.findByText('pending')).toBeTruthy();
    expect(d.calls).toEqual([]);
    act(() => {
      reach = { state: 'ready' };
      for (const l of [...listeners]) l();
    });
    expect(await screen.findByText('site')).toBeTruthy();
  });
});
