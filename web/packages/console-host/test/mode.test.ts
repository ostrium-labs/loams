// The `mode` service and lazy loading by mode (SD1 Task 3, design §53 §4.2–§4.3).

import type { Transport } from '@connectrpc/connect';
import type { Context } from '@loams/cordis';
import { describe, expect, it } from 'vite-plus/test';
import {
  boot,
  chooseBootMode,
  defaultMode,
  MODE_HOME,
  MODE_KEY,
  type ModeService,
  type ModuleTable,
  modeOfPath,
  type PluginModule,
  service,
  watch,
} from '../src/index.js';
import { createMockControl } from '../src/testing/index.js';

function platform(kind: 'web' | 'desktop', transport: Transport): PluginModule {
  return {
    name: 'platform-test',
    apply(ctx: Context) {
      ctx.provide('platform', {
        kind,
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

function manifest(name: string, extra: Record<string, unknown> = {}) {
  return {
    name,
    version: '0.1.0',
    loams: {
      plugin: { kind: 'console', entry: '.', tier: 'first-party', inject: [], ...extra },
    },
  };
}

/** Storage that remembers, or that throws on every access. */
function memoryStorage(initial: Record<string, string> = {}) {
  const data = new Map(Object.entries(initial));
  return {
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => void data.set(k, v),
    data,
  };
}
const brokenStorage = {
  getItem: (): string | null => {
    throw new Error('SecurityError');
  },
  setItem: () => {
    throw new Error('SecurityError');
  },
};

const settle = () => new Promise((r) => setTimeout(r, 20));

interface Setup {
  kind?: 'web' | 'desktop';
  path?: string;
  storage?: ReturnType<typeof memoryStorage> | typeof brokenStorage;
  features?: Record<string, boolean>;
  modeFlags?: Partial<Record<'console' | 'studio', string>>;
  withStudio?: boolean;
}

async function start(o: Setup = {}) {
  const applied: string[] = [];
  const disposed: string[] = [];
  const navigated: string[] = [];
  const plugin = (id: string): PluginModule => ({
    apply(ctx) {
      applied.push(id);
      ctx.effect(() => () => disposed.push(id));
    },
  });
  const modules: ModuleTable = {
    '@loams/plugin-shell': async () => plugin('shell'),
    '@loams/plugin-pages': async () => plugin('pages'),
    '@loams/plugin-studio': async () => plugin('studio'),
    '@loams/plugin-both': async () => plugin('both'),
  };
  const manifests = [
    manifest('@loams/plugin-shell', { tier: 'core' }),
    manifest('@loams/plugin-pages'),
    manifest('@loams/plugin-studio', { modes: ['studio'] }),
    manifest('@loams/plugin-both', { modes: ['console', 'studio'] }),
  ];
  const catalog = [
    { id: 'shell', name: '@loams/plugin-shell' },
    { id: 'pages', name: '@loams/plugin-pages' },
    ...(o.withStudio === false ? [] : [{ id: 'studio', name: '@loams/plugin-studio' }]),
    { id: 'both', name: '@loams/plugin-both' },
  ];
  const control = createMockControl({ features: o.features ?? {} });
  const handle = await boot({
    catalog,
    manifests,
    modules,
    platform: platform(o.kind ?? 'web', control.transport),
    storage: o.storage ?? memoryStorage(),
    initialPath: () => o.path ?? '/',
    navigate: (to) => navigated.push(to),
    modeFlags: o.modeFlags,
  });
  await settle();
  const mode = handle.ctx.get('mode', true) as ModeService;
  return { handle, mode, applied, disposed, navigated };
}

describe('mode helpers', () => {
  it('modeOfPath reads the /studio prefix only', () => {
    expect(modeOfPath('/studio')).toBe('studio');
    expect(modeOfPath('/studio/t/c_1')).toBe('studio');
    expect(modeOfPath('/studio?host=h1')).toBe('studio');
    expect(modeOfPath('/studio/t/c_1?panel=diff')).toBe('studio');
    expect(modeOfPath('/')).toBe('console');
    expect(modeOfPath('/studios')).toBe('console');
    expect(modeOfPath('/data/studio')).toBe('console');
  });

  it('the edition defaults are Console on the web and Studio on the desktop (D902, Q801)', () => {
    expect(defaultMode('web')).toBe('console');
    expect(defaultMode('desktop')).toBe('studio');
    expect(MODE_HOME).toEqual({ console: '/', studio: '/studio' });
  });

  it('boot_mode_from_path_then_storage_then_edition', () => {
    const both = ['console', 'studio'] as const;
    // The path wins over everything.
    expect(
      chooseBootMode({ path: '/studio/t/1', stored: 'console', kind: 'web', available: both }),
    ).toBe('studio');
    expect(
      chooseBootMode({ path: '/approvals', stored: 'studio', kind: 'desktop', available: both }),
    ).toBe('console');
    // At a mode's home that is also the other's ('/'), storage decides, then the edition.
    expect(chooseBootMode({ path: '/', stored: 'studio', kind: 'web', available: both })).toBe(
      'studio',
    );
    expect(chooseBootMode({ path: '/', stored: undefined, kind: 'desktop', available: both })).toBe(
      'studio',
    );
    expect(chooseBootMode({ path: '/', stored: undefined, kind: 'web', available: both })).toBe(
      'console',
    );
    // A mode that is not available never wins.
    expect(
      chooseBootMode({
        path: '/studio',
        stored: 'studio',
        kind: 'desktop',
        available: ['console'],
      }),
    ).toBe('console');
  });
});

describe('the mode service', () => {
  it('boots the stored mode and computes `available` from the catalog', async () => {
    const { mode, applied } = await start({ storage: memoryStorage({ [MODE_KEY]: 'studio' }) });
    expect(mode.available).toEqual(['console', 'studio']);
    expect(mode.current).toBe('studio');
    // Design 59 (W10): Console's plugins load at boot too, after Studio's.
    expect(applied.sort()).toEqual(['both', 'pages', 'shell', 'studio']);
  });

  it('desktop_opens_studio_on_first_run', async () => {
    const { mode } = await start({ kind: 'desktop' });
    expect(mode.current).toBe('studio');
  });

  it('storage_failure_falls_back_to_edition_default', async () => {
    const web = await start({ storage: brokenStorage });
    expect(web.mode.current).toBe('console');
    const desktop = await start({ kind: 'desktop', storage: brokenStorage });
    expect(desktop.mode.current).toBe('studio');
    // A failed write does not stop the switch.
    desktop.mode.set('console');
    expect(desktop.mode.current).toBe('console');
  });

  it('a path of the other mode boots that mode', async () => {
    const { mode } = await start({
      path: '/studio/t/c_1',
      storage: memoryStorage({ [MODE_KEY]: 'console' }),
    });
    expect(mode.current).toBe('studio');
  });

  it('an unknown stored value is ignored', async () => {
    const { mode } = await start({ storage: memoryStorage({ [MODE_KEY]: 'admin' }) });
    expect(mode.current).toBe('console');
  });

  it('without a studio plugin only console is available', async () => {
    const { mode } = await start({ kind: 'desktop', withStudio: false });
    // plugin-both still declares studio.
    expect(mode.available).toEqual(['console', 'studio']);
    const none = await start({
      kind: 'desktop',
      withStudio: false,
      modeFlags: { studio: 'studio' },
    });
    expect(none.mode.available).toEqual(['console']);
    expect(none.mode.current).toBe('console');
  });

  it('a mode flag hides the mode until GetInstance names the feature', async () => {
    const off = await start({ modeFlags: { studio: 'studio' }, path: '/studio' });
    expect(off.mode.available).toEqual(['console']);
    expect(off.mode.current).toBe('console');
    expect(off.applied).not.toContain('studio');
    off.mode.set('studio');
    expect(off.mode.current).toBe('console');

    const on = await start({ modeFlags: { studio: 'studio' }, features: { studio: true } });
    expect(on.mode.available).toEqual(['console', 'studio']);
  });

  it('set persists, navigates home or to opts.navigate, and notifies', async () => {
    const storage = memoryStorage();
    const { mode, navigated } = await start({ storage });
    const seen: string[] = [];
    const off = mode.onChange((m) => seen.push(m));
    mode.set('studio');
    expect(storage.data.get(MODE_KEY)).toBe('studio');
    expect(navigated).toEqual(['/studio']);
    mode.set('console', { navigate: '/approvals' });
    expect(navigated).toEqual(['/studio', '/approvals']);
    // Setting the current mode changes nothing and does not navigate.
    mode.set('console');
    expect(navigated).toHaveLength(2);
    off();
    mode.set('studio');
    expect(seen).toEqual(['studio', 'console']);
  });

  it('modesOf reports each plugin row`s modes', async () => {
    const { mode } = await start();
    expect(mode.modesOf('pages')).toEqual(['console']);
    expect(mode.modesOf('studio')).toEqual(['studio']);
    expect(mode.modesOf('both')).toEqual(['console', 'studio']);
    expect(mode.modesOf('unknown')).toEqual(['console']);
  });
});

describe('lazy loading by mode', () => {
  it('other_mode_plugins_load_at_boot_after_the_boot_mode (design 59, W10)', async () => {
    const { handle, mode, applied, navigated } = await start();
    expect(mode.current).toBe('console');
    await settle();
    // Studio's plugins load in Console too (its threads show in the shell's sidebar), without
    // switching the mode or navigating.
    expect(applied.sort()).toEqual(['both', 'pages', 'shell', 'studio']);
    expect(mode.current).toBe('console');
    expect(navigated).toEqual([]);
    expect(handle.plugins().find((p) => p.id === 'studio')?.status).toBe('active');
    expect(handle.pending().map((p) => p.id)).not.toContain('studio');

    // Later switches neither dispose nor re-apply anything.
    mode.set('studio');
    mode.set('console');
    mode.set('studio');
    await settle();
    expect(applied.filter((a) => a === 'studio')).toHaveLength(1);
    expect(handle.plugins().find((p) => p.id === 'pages')?.status).toBe('active');
  });

  it('switch_keeps_fibers', async () => {
    let open = 0;
    let aborted = 0;
    const streamer: PluginModule = {
      inject: ['mode'],
      apply(ctx) {
        watch(
          ctx,
          (signal) =>
            (async function* () {
              open++;
              signal.addEventListener('abort', () => aborted++);
              await new Promise(() => {});
              yield 1;
            })(),
          () => {},
        );
        expect(service(ctx, 'mode').current).toBe('studio');
      },
    };
    const control = createMockControl();
    const handle = await boot({
      catalog: [
        { id: 'pages', name: '@loams/plugin-pages' },
        { id: 'studio', name: '@loams/plugin-studio' },
      ],
      manifests: [
        manifest('@loams/plugin-pages'),
        manifest('@loams/plugin-studio', { modes: ['studio'], inject: ['mode'] }),
      ],
      modules: {
        '@loams/plugin-pages': async () => ({ apply() {} }),
        '@loams/plugin-studio': async () => streamer,
      },
      platform: platform('desktop', control.transport),
      storage: memoryStorage(),
      initialPath: () => '/',
      navigate: () => {},
    });
    await settle();
    const mode = handle.ctx.get('mode') as ModeService;
    expect(mode.current).toBe('studio');
    expect(open).toBe(1);
    mode.set('console');
    mode.set('studio');
    mode.set('console');
    await settle();
    expect(open).toBe(1);
    expect(aborted).toBe(0);
    expect(handle.plugins().find((p) => p.id === 'studio')?.status).toBe('active');
    await handle.dispose();
    expect(aborted).toBe(1);
  });
});
