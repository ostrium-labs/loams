import { createClient, createRouterTransport } from '@connectrpc/connect';
import { boot, type PlatformService, type PluginModule } from '@loams/console-host';
import { createMockControl } from '@loams/console-host/testing';
import { createWebPlatform } from '@loams/platform-web';
import { instance } from '@loams/proto';
import { describe, expect, it, vi } from 'vitest';
import type { LoamsDesktopApi } from './index.js';
import {
  createElectronPlatform,
  desktopFetch,
  instanceFeaturesInterceptor,
  wireDeepLinks,
} from './index.js';

type Kind = 'local' | 'remote' | 'demo';

function fakeApi(kind: Kind = 'remote'): LoamsDesktopApi & { calls: string[] } {
  const calls: string[] = [];
  const api = {
    version: 'test',
    platform: 'linux',
    calls,
    servers: {
      list: async () => ({
        servers: [{ id: 's1', name: 's', kind, url: 'http://x' }],
        activeId: 's1',
      }),
    },
    shell: {
      openExternal: vi.fn(async (url: string) => {
        calls.push(`open:${url}`);
        return { ok: true as const, value: undefined };
      }),
      notify: vi.fn(async () => {}),
      clipboardWrite: vi.fn(async () => {}),
      onNavigate: vi.fn(() => () => {}),
      takePendingNavigation: vi.fn(async () => null),
    },
  };
  return api as unknown as LoamsDesktopApi & { calls: string[] };
}

async function provided(api: LoamsDesktopApi) {
  const out: Record<string, unknown> = {};
  const ctx = {
    provide: (name: string, value: unknown) => {
      out[name] = value;
    },
  };
  await createElectronPlatform(api).apply(ctx as never, undefined);
  return out as { platform: PlatformService; transport: unknown; desktop: unknown };
}

const settle = () => new Promise((r) => setTimeout(r, 30));

describe('platform-electron', () => {
  it('platform_electron_provides_platform_transport_desktop', async () => {
    const api = fakeApi();
    const out = await provided(api);
    expect(out.platform.kind).toBe('desktop');
    expect(out.platform.baseUrl).toBe(globalThis.location.origin);
    expect(out.transport).toBeTruthy();
    expect(out.desktop).toBe(api);
  });

  it('openExternal_delegates_to_bridge', async () => {
    const api = fakeApi();
    const { platform } = await provided(api);
    await platform.openExternal('https://loams.dev');
    await platform.notify({ title: 't', body: 'b' });
    await platform.clipboardWrite('c');
    expect(api.calls).toEqual(['open:https://loams.dev']);
    expect(api.shell.notify).toHaveBeenCalledWith('t', 'b');
    expect(api.shell.clipboardWrite).toHaveBeenCalledWith('c');
  });

  it('openExternal_rejects_when_the_bridge_refuses', async () => {
    const api = fakeApi();
    vi.mocked(api.shell.openExternal).mockResolvedValueOnce({
      ok: false,
      code: 'x',
      message: 'no',
    });
    const { platform } = await provided(api);
    await expect(platform.openExternal('file:///etc/passwd')).rejects.toThrow('no');
  });

  it.each([
    ['demo', 'Bearer mock-access-usr_omar'],
    ['local', null],
    ['remote', null],
  ] as const)('fetch_bearer_for_%s', async (kind, expected) => {
    const seen: (string | null)[] = [];
    const spy = vi.spyOn(globalThis, 'fetch').mockImplementation(async (_i, init) => {
      seen.push(new Headers(init?.headers).get('authorization'));
      return new Response('{}');
    });
    await desktopFetch(fakeApi(kind))('http://x/y', { headers: { 'x-a': '1' } });
    spy.mockRestore();
    expect(seen).toEqual([expected]);
  });

  it.each([
    ['local', { desktop: true, local: true, keep: true }],
    ['remote', { desktop: true, keep: true }],
  ] as const)('instance_features_for_%s', async (kind, features) => {
    const api = fakeApi(kind);
    const list = api.servers.list;
    const transport = createRouterTransport(
      ({ service }) => {
        service(instance.InstanceService, {
          getInstance: () => ({ name: 'n', features: { keep: true } }),
        });
      },
      {
        transport: {
          interceptors: [instanceFeaturesInterceptor(async () => (await list()).servers[0]?.kind)],
        },
      },
    );
    const info = await createClient(instance.InstanceService, transport).getInstance({});
    expect({ ...info.features }).toEqual(features);
  });

  it('deep_links_subscribe_before_taking_the_pending_one', async () => {
    const api = fakeApi();
    const order: string[] = [];
    vi.mocked(api.shell.onNavigate).mockImplementation(() => {
      order.push('subscribe');
      return () => {};
    });
    vi.mocked(api.shell.takePendingNavigation).mockImplementation(async () => {
      order.push('take');
      return '/servers';
    });
    const go = vi.fn();
    await wireDeepLinks(api, go);
    expect(order).toEqual(['subscribe', 'take']);
    expect(go).toHaveBeenCalledWith('/servers');
  });

  it('browser_build_never_loads_desktop_plugins', async () => {
    // A test-only desktop plugin: editions ['desktop'], inject ['desktop'].
    let applied = false;
    const fixture: PluginModule = {
      inject: ['desktop'],
      apply() {
        applied = true;
      },
    };
    const manifest = {
      name: '@loams/plugin-test-desktop',
      version: '0.1.0',
      loams: {
        plugin: {
          kind: 'console',
          entry: '.',
          tier: 'first-party',
          inject: ['desktop'],
          editions: ['desktop'],
        },
      },
    };
    const handle = await boot({
      catalog: [{ id: 'test-desktop', name: '@loams/plugin-test-desktop' }],
      manifests: [manifest],
      modules: { '@loams/plugin-test-desktop': async () => fixture },
      platform: createWebPlatform({ baseUrl: 'mock:', transport: createMockControl().transport }),
    });
    await settle();
    expect(applied).toBe(false);
    expect(handle.pending()).toContainEqual({
      id: 'test-desktop',
      waitingFor: ['desktop'],
      silent: false,
    });
    await handle.dispose();
  });

  it('desktop_plugins_load_with_platform_electron', async () => {
    let applied = false;
    const fixture: PluginModule = {
      inject: ['desktop'],
      apply() {
        applied = true;
      },
    };
    const manifest = {
      name: '@loams/plugin-test-desktop',
      version: '0.1.0',
      loams: {
        plugin: {
          kind: 'console',
          entry: '.',
          tier: 'first-party',
          inject: ['desktop'],
          editions: ['desktop'],
        },
      },
    };
    const handle = await boot({
      catalog: [{ id: 'test-desktop', name: '@loams/plugin-test-desktop' }],
      manifests: [manifest],
      modules: { '@loams/plugin-test-desktop': async () => fixture },
      platform: createElectronPlatform(fakeApi('local')),
    });
    await settle();
    expect(applied).toBe(true);
    await handle.dispose();
  });
});
