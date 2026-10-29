import { type SessionService, SHELL_COMMAND_EVENT, ThemeController } from '@loams/console-host';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { Layout } from '../src/layout.js';
import { DRAWER, INSPECTOR, PanelController } from '../src/panel.js';
import { HashRouter, type HashSource } from '../src/router.js';

function memory(start = '/'): HashSource {
  let path = start;
  const listeners = new Set<() => void>();
  return {
    get: () => path,
    set: (p) => {
      path = p;
      for (const l of listeners) l();
    },
    listen: (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  };
}
const session = { principal: () => null, subscribe: () => () => {} } as unknown as SessionService;
const theme = new ThemeController({ read: () => 'dark', write: () => {} });

function setup(start = '/data') {
  const slots = new SlotRegistry();
  const router = new HashRouter(slots, memory(start));
  const inspector = new PanelController(INSPECTOR);
  const drawer = new PanelController(DRAWER);
  slots.register(
    {
      name: 'shell.nav.section',
      plugin: 'test',
      order: 1,
      meta: { id: 'settings', label: 'App settings', group: 'Settings', href: '/settings' },
    },
    () => null,
  );
  render(
    <SlotProvider registry={slots}>
      <Layout
        router={router}
        session={session}
        theme={theme}
        inspector={inspector}
        drawer={drawer}
      />
    </SlotProvider>,
  );
  return { router, inspector, drawer };
}

/** What `wireShellCommands` sends when a menu item is clicked; true if something took it. */
function menu(command: string): boolean {
  const event = new CustomEvent(SHELL_COMMAND_EVENT, { detail: command, cancelable: true });
  act(() => {
    window.dispatchEvent(event);
  });
  return event.defaultPrevented;
}

beforeEach(() => {
  localStorage.clear();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(cleanup);

describe('the desktop menu runs the shell (CF1 Task 14, D1084)', () => {
  it('toggles the sidebar, the inspector and the drawer', () => {
    const { inspector, drawer } = setup();
    expect(screen.getByRole('navigation', { name: 'Sidebar' })).toBeTruthy();
    expect(menu('sidebar.toggle')).toBe(true);
    expect(screen.queryByRole('navigation', { name: 'Sidebar' })).toBeNull();
    menu('inspector.toggle');
    expect(inspector.state().open).toBe(true);
    menu('drawer.toggle');
    expect(drawer.state().open).toBe(true);
    menu('drawer.toggle');
    expect(drawer.state().open).toBe(false);
  });

  it('opens and closes the command palette', () => {
    setup();
    expect(screen.queryByRole('dialog')).toBeNull();
    menu('palette.toggle');
    expect(screen.getByRole('dialog')).toBeTruthy();
  });

  it('opens Settings where the sidebar foot does', () => {
    const { router } = setup();
    expect(menu('settings.open')).toBe(true);
    expect(router.current().path).toBe('/settings');
  });

  it('leaves Studio`s commands and one already taken alone', () => {
    const { router } = setup();
    expect(menu('chat.new')).toBe(false);
    expect(menu('thread.next')).toBe(false);
    const taken = (e: Event) => e.preventDefault();
    window.addEventListener(SHELL_COMMAND_EVENT, taken, { capture: true });
    menu('settings.open');
    window.removeEventListener(SHELL_COMMAND_EVENT, taken, { capture: true });
    expect(router.current().path).toBe('/data');
  });

  it('goes back and forward through the history', () => {
    const back = vi.spyOn(globalThis.history, 'back').mockImplementation(() => {});
    const forward = vi.spyOn(globalThis.history, 'forward').mockImplementation(() => {});
    setup();
    menu('navigation.back');
    menu('navigation.forward');
    expect(back).toHaveBeenCalledOnce();
    expect(forward).toHaveBeenCalledOnce();
    back.mockRestore();
    forward.mockRestore();
  });

  it('reopens the thread left last (W12), through the keymap`s run', () => {
    const { router } = setup('/studio/t/a');
    act(() => router.navigate('/data'));
    expect(menu('thread.reopen')).toBe(true);
    expect(router.current().path).toBe('/studio/t/a');
  });
});
