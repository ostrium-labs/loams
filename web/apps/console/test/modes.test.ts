// Console | Studio in the real console (SD1 Tasks 3 and 15): the oss catalog with
// the browser platform over the in-memory mock. The switch shows in every edition.

import type { ConsoleHandle } from '@loams/console-host';
import { MODE_KEY, type ModeService, parseCatalog, parsePatch } from '@loams/console-host';
import { createMockTransport } from '@loams/console-host/testing';
import { createWebPlatform } from '@loams/platform-web';
import { pageEntries } from '@loams/plugin-shell';
import { fireEvent, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import baseCatalog from '../catalog/base.yml?raw';
import desktopYml from '../catalog/desktop.yml?raw';
import { startConsole } from '../src/cordis/start.js';

let dispose: (() => Promise<void>) | undefined;
beforeEach(() => {
  localStorage.clear();
  window.location.hash = '';
  // The shell's sidebar scrolls in Base UI's scroll area, which jsdom cannot animate.
  Element.prototype.getAnimations ??= () => [];
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  window.matchMedia ??= ((query: string) => ({
    matches: query.includes('min-width'),
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
});
afterEach(async () => {
  await dispose?.();
  dispose = undefined;
  document.body.innerHTML = '';
  window.location.hash = '';
  localStorage.clear();
});

async function start(features: Record<string, boolean> = {}) {
  const root = document.createElement('div');
  document.body.append(root);
  const handle = await startConsole({
    platform: createWebPlatform({ transport: createMockTransport({ features }) }),
    root,
    base: '/ui/',
  });
  dispose = handle.dispose;
  const mode = handle.ctx.get('mode') as ModeService;
  return { root, handle, mode };
}

const go = (path: string) => {
  window.location.hash = `#${path}`;
  window.dispatchEvent(new HashChangeEvent('hashchange'));
};
/** Every page the plugins register (design 59 D1080: the sidebar pins only a few). */
const navLabels = (handle: { slots: ConsoleHandle['slots'] }) =>
  pageEntries(
    handle.slots.entries('shell.nav.section'),
    handle.slots.entries('console.nav'),
    undefined,
  ).map((p) => p.label);

describe('modes in the console', () => {
  it('console_routes_unchanged', async () => {
    const { root, handle, mode } = await start();
    await waitFor(() => expect(navLabels(handle)).toContain('Approvals'));
    for (const [path, text] of [
      ['/', 'Demo instance'],
      ['/namespaces', 'search-prod'],
      ['/approvals', 'Drop the collection docs in production'],
      ['/deployments', 'Deployments'],
    ] as const) {
      go(path);
      await waitFor(() => expect(root.textContent).toContain(text));
      expect(root.textContent).not.toContain('Nothing here');
      expect(mode.current).toBe('console');
    }
  }, 20_000);

  it('the switch shows in every edition, and Studio says how to connect an agentd', async () => {
    // No `studio` instance feature and no agentd host (owner feedback 2026-10-17).
    const { root, handle, mode } = await start();
    await waitFor(() => expect(navLabels(handle)).toContain('Approvals'));
    expect(mode.available).toEqual(['console', 'studio']);
    expect(handle.plugins().find((p) => p.id === 'studio')?.status).toBe('deferred');
    const studio = await waitFor(() => {
      const b = [...root.querySelectorAll('button')].find((x) => x.textContent === 'Studio');
      expect(b).toBeTruthy();
      return b as HTMLButtonElement;
    });
    expect(root.querySelector('[aria-label="Mode"]')).not.toBeNull();
    fireEvent.click(studio);
    expect(mode.current).toBe('studio');
    expect(localStorage.getItem(MODE_KEY)).toBe('studio');
    // Entering Studio imports its plugin (and its sidebar's dependencies) the first time.
    await waitFor(() => expect(navLabels(handle)).toContain('Studio settings'), {
      timeout: 10_000,
    });
    expect(window.location.hash).toBe('#/studio');
    await waitFor(
      () => expect(root.textContent).toContain('Studio needs a Loams server with agentd'),
      { timeout: 10_000 },
    );
    const link = [...root.querySelectorAll('a')].find((a) => a.textContent === 'Agentd hosts');
    expect(link?.getAttribute('href')).toBe('#/studio/settings');
    expect(handle.plugins().find((p) => p.id === 'studio')?.status).toBe('active');
    // A Console route switches back; Studio stays loaded.
    go('/approvals');
    await waitFor(() => expect(mode.current).toBe('console'));
    await waitFor(() => expect(navLabels(handle)).toContain('Approvals'));
    expect(handle.plugins().find((p) => p.id === 'studio')?.status).toBe('active');
  }, 20_000);

  it('a stored Studio mode opens Studio at boot', async () => {
    localStorage.setItem(MODE_KEY, 'studio');
    const { mode } = await start();
    expect(mode.current).toBe('studio');
  });

  it('a Studio deep link opens Studio at boot', async () => {
    window.location.hash = '#/studio/settings';
    const { root, mode } = await start();
    expect(mode.current).toBe('studio');
    await waitFor(() => expect(root.textContent).toContain('Studio settings'));
  });
});

describe('catalogs', () => {
  it('studio is in the base catalog, and the desktop row adds the bridge', () => {
    const base = parseCatalog(baseCatalog);
    expect(base.find((r) => r.id === 'studio')).toMatchObject({
      name: '@loams/plugin-studio',
      // Design 59 (Task 9): the shell's inspector and drawer hold Studio's thread tabs.
      inject: ['router', 'slots', 'mode', 'platform', 'flags', 'agentd', 'inspector', 'drawer'],
    });
    const desktop = parsePatch(desktopYml).find((r) => 'id' in r && r.id === 'studio');
    expect(desktop).toMatchObject({
      inject: expect.arrayContaining(['agentd', 'inspector', 'drawer', 'desktop']),
    });
  });
});
