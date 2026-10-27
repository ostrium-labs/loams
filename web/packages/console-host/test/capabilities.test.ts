import type { Transport } from '@connectrpc/connect';
import type { Context } from '@loams/cordis';
import type { SlotRegistry } from '@loams/slots';
import { describe, expect, it } from 'vite-plus/test';
import train from '../../../../release/upgrade-path.toml?raw';
import {
  boot,
  capabilities,
  compareVersions,
  type FlagsService,
  MIN_ENGINE,
  type ModuleTable,
  type PluginModule,
  service,
  validateManifest,
} from '../src/index.js';
import { createMockControl, MOCK_SERVER_VERSION } from '../src/testing/index.js';

function platformWith(transport: Transport): PluginModule {
  return {
    name: 'platform-test',
    apply(ctx: Context) {
      ctx.provide('platform', {
        kind: 'web',
        fetch: globalThis.fetch,
        baseUrl: 'mock:',
        openExternal: async () => {},
        notify: async () => {},
        clipboardWrite: async () => {},
      });
      ctx.provide('transport', transport);
    },
  };
}

function manifest(name: string, requires: Record<string, unknown>) {
  return {
    name,
    version: '0.1.0',
    loams: {
      plugin: {
        kind: 'console',
        entry: '.',
        tier: 'first-party',
        inject: ['slots'],
        slots: ['console.page', 'shell.nav.section'],
        requires: { console: '^0.1.0', ...requires },
      },
    },
  };
}

/** A plugin with a page and a nav item, like @loams/plugin-house. */
function pageWithNav(id: string): PluginModule {
  return {
    inject: ['slots'],
    apply(ctx) {
      const slots = service(ctx, 'slots') as SlotRegistry;
      ctx.effect(() =>
        slots.register(
          { name: 'console.page', key: id, plugin: id, meta: { id, href: `/${id}` } },
          () => null,
        ),
      );
      ctx.effect(() =>
        slots.register(
          { name: 'shell.nav.section', plugin: id, meta: { id, href: `/${id}` } },
          () => null,
        ),
      );
    },
  };
}

const modules: ModuleTable = {
  '@loams/plugin-analytics': async () => pageWithNav('analytics'),
  '@loams/plugin-newer': async () => pageWithNav('newer'),
  '@loams/plugin-plain': async () => pageWithNav('plain'),
};
const manifests = [
  manifest('@loams/plugin-analytics', { api: ['loams.house.v1'] }),
  manifest('@loams/plugin-newer', { engine: MOCK_SERVER_VERSION }),
  manifest('@loams/plugin-plain', {}),
];
const catalog = [
  { id: 'analytics', name: '@loams/plugin-analytics' },
  { id: 'newer', name: '@loams/plugin-newer' },
  { id: 'plain', name: '@loams/plugin-plain' },
];
const settle = () => new Promise((r) => setTimeout(r, 20));

/** The engine N (the newest release) and N−1. */
const ENGINES = {
  n: { serverVersion: MOCK_SERVER_VERSION, apiVersions: ['loams.instance.v1', 'loams.house.v1'] },
  previous: { serverVersion: MIN_ENGINE, apiVersions: ['loams.instance.v1'] },
};

async function bootOn(engine: { serverVersion: string; apiVersions: string[] }) {
  const mock = createMockControl(engine);
  const handle = await boot({
    catalog,
    manifests,
    modules,
    platform: platformWith(mock.transport),
  });
  await settle();
  const ids = (slot: 'console.page' | 'shell.nav.section') =>
    handle.slots
      .entries(slot)
      .map((e) => (e.meta as { id: string }).id)
      .sort();
  return { handle, pages: ids('console.page'), nav: ids('shell.nav.section') };
}

describe('capabilities', () => {
  it('compares versions in semver order', () => {
    expect(compareVersions('0.1.0-preview.2', '0.1.0-preview.3')).toBeLessThan(0);
    expect(compareVersions('0.1.0-preview.10', '0.1.0-preview.9')).toBeGreaterThan(0);
    expect(compareVersions('0.1.0', '0.1.0-preview.9')).toBeGreaterThan(0);
    expect(compareVersions('v0.2.0', '0.1.9')).toBeGreaterThan(0);
    expect(compareVersions('0.1.0-preview.3', '0.1.0-preview.3')).toBe(0);
    expect(compareVersions('dev', '0.1.0')).toBeUndefined();
  });

  it('min_engine_is_the_newest_release_source', () => {
    // N−1 is the `from` of the newest release in the release train.
    const newest = /\[\[release\]\][^[]*?from\s*=\s*"([^"]+)"/.exec(train)?.[1];
    // A minor line (`0.1`) starts at its first release.
    const n1 = newest && /^\d+\.\d+$/.test(newest) ? `${newest}.0` : newest;
    expect(n1).toBe(MIN_ENGINE);
  });

  it('console_supports_previous_engine', async () => {
    const n = await bootOn(ENGINES.n);
    expect(n.pages).toEqual(['analytics', 'newer', 'plain']);
    await n.handle.dispose();
    // N−1: no route of a service it lacks, and nothing that needs N.
    const previous = await bootOn(ENGINES.previous);
    expect(previous.pages).toEqual(['plain']);
    const reasons = Object.fromEntries(previous.handle.plugins().map((p) => [p.id, p.reason]));
    expect(reasons.analytics).toBe('the engine does not serve loams.house.v1');
    expect(reasons.newer).toBe(
      `needs engine ${MOCK_SERVER_VERSION} or later (the engine is ${MIN_ENGINE})`,
    );
    expect(reasons.plain).toBeUndefined();
    await previous.handle.dispose();
  });

  it('missing_service_hides_nav_item', async () => {
    const previous = await bootOn(ENGINES.previous);
    expect(previous.nav).toEqual(['plain']);
    await previous.handle.dispose();
    const n = await bootOn(ENGINES.n);
    expect(n.nav).toContain('analytics');
    await n.handle.dispose();
  });

  it('an engine older than N−1, or without GetInstance, is not supported', () => {
    const flags = (serverVersion: string, extra: Partial<FlagsService> = {}): FlagsService => ({
      edition: 'oss',
      instanceName: 'x',
      serverVersion,
      features: {},
      apiVersions: ['loams.house.v1'],
      has: (api) => api === 'loams.house.v1',
      ...extra,
    });
    expect(capabilities(flags(MIN_ENGINE)).supported).toBe(true);
    const old = capabilities(flags('0.1.0-preview.1'));
    expect(old).toMatchObject({ supported: false, reason: 'too-old' });
    expect(old.has('loams.house.v1')).toBe(false);
    expect(capabilities(flags('', { instanceError: 'unimplemented' })).reason).toBe('no-instance');
    expect(capabilities(undefined).reason).toBe('no-instance');
    // An engine that cannot be reached is not called old.
    expect(capabilities(flags('', { instanceError: 'unreachable' })).supported).toBe(true);
    // A local build's version is not refused.
    expect(capabilities(flags('dev')).supported).toBe(true);
    expect(capabilities(flags(MIN_ENGINE)).has('loams.house.v1', MOCK_SERVER_VERSION)).toBe(false);
  });

  it('an engine without GetInstance is flagged unimplemented', async () => {
    const mock = createMockControl({ noInstance: true });
    const handle = await boot({
      catalog,
      manifests,
      modules,
      platform: platformWith(mock.transport),
    });
    await settle();
    const flags = handle.ctx.get('flags', true) as FlagsService;
    expect(flags.instanceError).toBe('unimplemented');
    expect(capabilities(flags).reason).toBe('no-instance');
    await handle.dispose();
  });

  it('requires.engine is validated', () => {
    const ok = validateManifest(manifest('@loams/plugin-x', { engine: '0.1.0-preview.3' }));
    expect(ok.requires.engine).toBe('0.1.0-preview.3');
    expect(() => validateManifest(manifest('@loams/plugin-x', { engine: 'latest' }))).toThrow(
      /requires.engine/,
    );
  });
});
