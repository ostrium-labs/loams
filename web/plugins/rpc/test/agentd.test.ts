// `agentd` and `rpc.agentd.*` (plan SD1 Task 6, §53 §6.2, §8.3, D918): provided
// only while a daemon is reachable, never to a third-party plugin, to a first-party
// one only with `agentd:use`, and with git writes only under `agentd:git-write`.

import { Code, ConnectError, createRouterTransport } from '@connectrpc/connect';
import {
  type AgentdReach,
  type AgentdService,
  boot,
  type PluginModule,
  service,
} from '@loams/console-host';
import { createMockControl } from '@loams/console-host/testing';
import type { Context } from '@loams/cordis';
import { Context as Root } from '@loams/cordis';
import { GitService, ProjectService } from '@loams/proto/agentd';
import { describe, expect, it, vi } from 'vite-plus/test';
import rpc, { AGENTD_SERVICES } from '../src/index.js';

/** A daemon's GitService and ProjectService, recording what reaches it. */
function daemon() {
  const calls: string[] = [];
  const transport = createRouterTransport(({ service: s }) => {
    s(ProjectService, {
      listRepos: () => {
        calls.push('ListRepos');
        return { repos: [] };
      },
    });
    s(GitService, {
      listBranches: () => {
        calls.push('ListBranches');
        return { branches: ['main'] };
      },
      commitWorktree: () => {
        calls.push('CommitWorktree');
        return { sha: 'abc', summary: 'parity' };
      },
    });
  });
  return { transport, calls };
}

/** A platform's `agentd.endpoint` whose reach the test drives. */
function endpoint(transport = daemon().transport) {
  let reach: AgentdReach = { state: 'stopped' };
  const listeners = new Set<() => void>();
  const value: AgentdService = {
    kind: 'desktop',
    reach: () => reach,
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    transport: () => (reach.state === 'ready' ? transport : undefined),
    host: () => undefined,
  };
  const set = (next: AgentdReach) => {
    reach = next;
    for (const l of [...listeners]) l();
  };
  return { value, set };
}

const agentdNames = () => ['agentd.transport', ...AGENTD_SERVICES.map((s) => s.name)];

async function rpcWith(deps: (ctx: Context) => void) {
  const root = new Root();
  await root.plugin({ name: 'deps', apply: deps });
  await root.plugin({ name: 'rpc', inject: rpc.inject, apply: rpc.apply });
  return root;
}

const has = (root: Root, name: string) => root.get(name, true) !== undefined;

describe('rpc.agentd', () => {
  it('rpc_agentd_absent_without_daemon', async () => {
    const mock = createMockControl({});
    const e = endpoint();
    const root = await rpcWith((ctx) => {
      ctx.provide('transport', mock.transport);
      ctx.provide('flags', { has: () => false });
      ctx.provide('agentd.endpoint', e.value);
    });
    // `agentd` is always there, so a page can offer to start the daemon.
    expect((root.get('agentd') as AgentdService).reach()).toEqual({ state: 'stopped' });
    expect(agentdNames().filter((n) => has(root, n))).toEqual([]);
    // The daemon comes up: every client appears.
    e.set({ state: 'ready' });
    expect(agentdNames().filter((n) => has(root, n))).toEqual(agentdNames());
    const projects = root.get('rpc.agentd.projects') as {
      listRepos(r: object): Promise<{ repos: unknown[] }>;
    };
    expect((await projects.listRepos({})).repos).toEqual([]);
    // And goes away again.
    e.set({ state: 'unavailable', reason: 'crashed' });
    expect(agentdNames().filter((n) => has(root, n))).toEqual([]);
  });

  it('the web has no host until SD1 Task 7', async () => {
    const mock = createMockControl({});
    const root = await rpcWith((ctx) => {
      ctx.provide('transport', mock.transport);
      ctx.provide('flags', { has: () => false });
    });
    const agentd = root.get('agentd') as AgentdService;
    expect(agentd.reach()).toEqual({ state: 'no-host' });
    expect(agentd.transport()).toBeUndefined();
    expect(agentdNames().filter((n) => has(root, n))).toEqual([]);
  });

  it('rpc_agentd_not_provided_to_third_party', async () => {
    const control = createMockControl();
    const d = daemon();
    const e = endpoint(d.transport);
    e.set({ state: 'ready' });
    const seen: Record<string, { git?: unknown; transport?: unknown }> = {};
    const probe = (id: string): PluginModule => ({
      apply(ctx: Context) {
        seen[id] = {
          git: service(ctx, 'rpc.agentd.git'),
          transport: service(ctx, 'agentd').transport(),
        };
      },
    });
    const pkg = (name: string, plugin: Record<string, unknown>) => ({
      name,
      version: '0.1.0',
      loams: { plugin: { kind: 'console', entry: '.', tier: 'first-party', ...plugin } },
    });
    const inject = ['agentd', 'rpc.agentd.git'];
    const handle = await boot({
      catalog: [
        { id: 'rpc', name: '@loams/plugin-rpc' },
        { id: 'evil', name: '@acme/plugin-evil' },
        { id: 'nope', name: '@loams/plugin-nope' },
        { id: 'reader', name: '@loams/plugin-reader' },
        { id: 'writer', name: '@loams/plugin-writer' },
      ],
      manifests: [
        pkg('@loams/plugin-rpc', { tier: 'core', inject: ['transport', 'flags'] }),
        pkg('@acme/plugin-evil', { tier: 'third-party', inject }),
        pkg('@loams/plugin-nope', { inject }),
        pkg('@loams/plugin-reader', { inject, permissions: ['agentd:use'] }),
        pkg('@loams/plugin-writer', {
          inject,
          permissions: ['agentd:use', 'agentd:git-write'],
        }),
      ],
      modules: {
        '@loams/plugin-rpc': async () => rpc,
        '@loams/plugin-nope': async () => probe('nope'),
        '@loams/plugin-reader': async () => probe('reader'),
        '@loams/plugin-writer': async () => probe('writer'),
      },
      platform: {
        name: 'platform-test',
        apply(ctx) {
          ctx.provide('platform', {
            kind: 'desktop',
            fetch: globalThis.fetch,
            baseUrl: 'mock:',
            openExternal: async () => {},
            notify: async () => {},
            clipboardWrite: async () => {},
          });
          ctx.provide('transport', control.transport);
          ctx.provide('agentd.endpoint', e.value);
        },
      },
      sandboxScripts: { '@acme/plugin-evil': '/plugins/evil.js' },
      grant: (m) => m.permissions,
      storage: { getItem: () => null, setItem: () => {} },
      initialPath: () => '/',
      navigate: () => {},
    });
    const status = (id: string) => handle.plugins().find((p) => p.id === id);
    expect(status('evil')?.status).toBe('failed');
    expect(status('evil')?.reason).toMatch(
      /third-party plugins never get agentd, rpc\.agentd\.git/,
    );
    expect(status('nope')?.status).toBe('failed');
    expect(status('nope')?.reason).toMatch(/needs the agentd:use permission/);
    expect(seen.nope).toBeUndefined();
    await vi.waitFor(() => {
      expect(status('reader')?.status).toBe('active');
      expect(status('writer')?.status).toBe('active');
    });

    // agentd:git-write: the reader may read git but never write to it.
    type Git = {
      listBranches(r: object): Promise<{ branches: string[] }>;
      commitWorktree(r: object): Promise<{ sha: string }>;
    };
    type Unary = { unary: (m: unknown, ...rest: unknown[]) => Promise<unknown> };
    const got = seen as Record<'reader' | 'writer', { git: Git; transport: Unary }>;
    const reader = got.reader.git;
    expect((await reader.listBranches({})).branches).toEqual(['main']);
    const refused = await reader.commitWorktree({}).catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(ConnectError);
    expect((refused as ConnectError).code).toBe(Code.PermissionDenied);
    expect(String((refused as ConnectError).message)).toMatch(/agentd:git-write/);
    // Its transport (for connect-query) refuses the same.
    const viaTransport = await got.reader.transport
      .unary(GitService.method.commitWorktree, undefined, undefined, undefined, {})
      .catch((e: unknown) => e);
    expect((viaTransport as ConnectError).code).toBe(Code.PermissionDenied);
    expect(d.calls).toEqual(['ListBranches']);
    // The writer's call reaches the daemon.
    expect((await got.writer.git.commitWorktree({})).sha).toBe('abc');
    expect(d.calls).toEqual(['ListBranches', 'CommitWorktree']);
    await handle.dispose();
  });
});
