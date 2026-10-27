import { createMockControl } from '@loams/console-host/testing';
import type { Context } from '@loams/cordis';
import { Context as Root } from '@loams/cordis';
import { describe, expect, it } from 'vite-plus/test';
import rpc, { RPC_SERVICES } from '../src/index.js';

describe('@loams/plugin-rpc', () => {
  it('rpc_service_provided_only_when_api_listed', async () => {
    const mock = createMockControl({ apiVersions: ['loams.instance.v1', 'loams.approvals.v1'] });
    const root = new Root();
    await root.plugin({
      name: 'deps',
      apply(ctx: Context) {
        ctx.provide('transport', mock.transport);
        ctx.provide('flags', { has: (api: string) => ['loams.approvals.v1'].includes(api) });
      },
    });
    await root.plugin({ name: 'rpc', inject: rpc.inject, apply: rpc.apply });
    const provided = RPC_SERVICES.map((s) => s.name).filter((n) => root.get(n) !== undefined);
    expect(provided).toEqual(['rpc.instance', 'rpc.approvals']);
    const client = root.get('rpc.approvals') as {
      listApprovals(r: object): Promise<{ approvals: unknown[] }>;
    };
    expect((await client.listApprovals({})).approvals.length).toBe(2);
  });

  it('console_api_clients_follow_auth_and_admin', async () => {
    const mock = createMockControl({});
    const root = new Root();
    const served = ['loams.auth.v1', 'loams.admin.v1'];
    await root.plugin({
      name: 'deps',
      apply(ctx: Context) {
        ctx.provide('transport', mock.transport);
        ctx.provide('flags', { has: (api: string) => served.includes(api) });
      },
    });
    await root.plugin({ name: 'rpc', inject: rpc.inject, apply: rpc.apply });
    const provided = RPC_SERVICES.map((s) => s.name).filter((n) => root.get(n) !== undefined);
    expect(provided).toEqual([
      'rpc.instance',
      'rpc.auth',
      'rpc.org',
      'rpc.projects',
      'rpc.agents',
      'rpc.keys',
      'rpc.audit',
    ]);
  });
});
