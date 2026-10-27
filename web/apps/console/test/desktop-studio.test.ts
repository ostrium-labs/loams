// Loams Studio on the desktop edition (SD1 Task 28): the real desktop console over the
// preview's fake bridge. It opens in Studio (Q801), and the application menu's
// commands switch modes.

import type { ModeService } from '@loams/console-host';
import type { LoamsDesktopApi, ShellCommand } from '@loams/desktop/contracts';
import { waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import { startDesktop } from '../src/cordis/desktop.js';
import { createFakeDesktop } from '../src/cordis/fake-desktop.js';

let dispose: (() => Promise<void>) | undefined;
beforeEach(() => {
  localStorage.clear();
  window.location.hash = '';
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

async function start() {
  const api: LoamsDesktopApi = createFakeDesktop();
  let send: (c: ShellCommand) => void = () => {};
  api.shell.onCommand = (cb) => {
    send = cb;
    return () => {};
  };
  const root = document.createElement('div');
  document.body.append(root);
  const handle = await startDesktop(api, root);
  dispose = handle.dispose;
  const mode = handle.ctx.get('mode') as ModeService;
  return { mode, send: (c: ShellCommand) => send(c) };
}

describe('the desktop edition', () => {
  it('desktop_default_mode_studio', async () => {
    const { mode } = await start();
    expect(mode.available).toEqual(['console', 'studio']);
    expect(mode.current).toBe('studio');
    await waitFor(() => expect(window.location.hash).toBe('#/studio'));
  }, 20_000);

  it('menu_switches_mode', async () => {
    const { mode, send } = await start();
    send('mode.console');
    expect(mode.current).toBe('console');
    await waitFor(() => expect(window.location.hash).toBe('#/'));
    send('mode.toggle');
    expect(mode.current).toBe('studio');
    send('mode.console');
    send('mode.studio');
    expect(mode.current).toBe('studio');
    await waitFor(() => expect(window.location.hash).toBe('#/studio'));
  }, 20_000);
});
