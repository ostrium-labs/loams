import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { type SessionService, ThemeController } from '@loams/console-host';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { SIDEBAR } from '../src/frame.js';
import { Layout } from '../src/layout.js';
import { HashRouter, type HashSource } from '../src/router.js';

function memory(start = '/'): HashSource {
  let path = start;
  return {
    get: () => path,
    set: (p) => {
      path = p;
    },
    listen: () => () => {},
  };
}
// One principal object: the account menu reads it through useSyncExternalStore.
const who = { displayName: 'Omar' };
const session = { principal: () => who, subscribe: () => () => {} } as unknown as SessionService;
const theme = new ThemeController({ read: () => 'dark', write: () => {} });

function setup(start = '/') {
  const slots = new SlotRegistry();
  const router = new HashRouter(slots, memory(start));
  const ui = render(
    <SlotProvider registry={slots}>
      <Layout router={router} session={session} theme={theme} />
    </SlotProvider>,
  );
  return { slots, router, ui };
}

beforeEach(() => {
  localStorage.clear();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('the frame (D1078)', () => {
  it('has a sidebar, a 44px top bar that is the drag region, and main', () => {
    setup();
    const bar = document.querySelector('.lc-topbar') as HTMLElement;
    expect(bar).toBeTruthy();
    expect(bar.dataset.dragRegion).toBe('');
    expect(screen.getByRole('main')).toBeTruthy();
    expect(screen.getByRole('navigation', { name: 'Sidebar' })).toBeTruthy();
    // jsdom does not load console.css, so the drag contract is checked in the stylesheet.
    // jsdom's URL is not Node's, so the path is resolved from the file's own path.
    const css = readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), '../../../apps/console/src/cordis/console.css'),
      'utf8',
    );
    expect(css).toMatch(
      /\.lc-topbar \{[^}]*height: var\(--op-topbar\);[^}]*-webkit-app-region: drag;/,
    );
    expect(css).toMatch(
      /\.lc-topbar\s+:is\(button, a, input, select, textarea, \[role="button"\], \[role="combobox"\], \[data-no-drag\]\) \{\s*-webkit-app-region: no-drag;/,
    );
  });

  it('collapses and restores the sidebar, and keeps its width within 200–320', () => {
    localStorage.setItem(SIDEBAR.key, JSON.stringify({ open: true, width: 900 }));
    setup();
    const nav = screen.getByRole('navigation', { name: 'Sidebar' });
    expect((nav.parentElement as HTMLElement).style.width).toBe('320px');
    fireEvent.click(screen.getByRole('button', { name: 'Hide sidebar' }));
    expect(screen.queryByRole('navigation', { name: 'Sidebar' })).toBeNull();
    expect(JSON.parse(localStorage.getItem(SIDEBAR.key) ?? '{}').open).toBe(false);
    fireEvent.click(screen.getByRole('button', { name: 'Show sidebar' }));
    expect(screen.getByRole('navigation', { name: 'Sidebar' })).toBeTruthy();
  });

  it('renders the server indicator in the sidebar and the top bar slots in order', () => {
    const { slots } = setup();
    act(() => {
      slots.register({ name: 'shell.sidebar.server', plugin: 't' }, () => (
        <span>Local engine</span>
      ));
      slots.register({ name: 'shell.topbar.left', plugin: 't' }, () => (
        <button type="button">acme</button>
      ));
      slots.register({ name: 'shell.topbar.right', plugin: 't' }, () => <span>right</span>);
    });
    expect(screen.getByRole('navigation', { name: 'Sidebar' }).textContent).toContain(
      'Local engine',
    );
    expect((document.querySelector('.lc-topbar') as HTMLElement).textContent).toMatch(
      /acme[\s\S]*right/,
    );
  });

  it('renders the home slot on the mode home and the page outlet elsewhere', () => {
    const { slots, router } = setup('/studio');
    act(() => {
      slots.register({ name: 'shell.home', plugin: 'studio' }, () => <p>composer</p>);
      router.page({ id: 'studio', path: '/studio', title: 'Studio' }, () => <p>old studio page</p>);
      router.page({ id: 'data', path: '/data', title: 'Data' }, () => <p>data page</p>);
    });
    // Without a mode service the home is Console's "/": /studio renders its page.
    expect(screen.getByText('old studio page')).toBeTruthy();
    act(() => router.navigate('/data'));
    expect(screen.getByText('data page')).toBeTruthy();
  });

  it('New thread asks Studio first, then falls back to its home', () => {
    const { router } = setup('/data');
    fireEvent.click(screen.getByRole('button', { name: /New thread/ }));
    expect(router.current().path).toBe('/studio');
  });

  it('toggles the sidebar with Mod+B, but rich text keeps it for bold (D1081)', () => {
    setup();
    // jsdom is not a Mac, so Mod is Ctrl and ⌘ does nothing.
    fireEvent.keyDown(window, { key: 'b', ctrlKey: true });
    expect(screen.queryByRole('navigation', { name: 'Sidebar' })).toBeNull();
    fireEvent.keyDown(window, { key: 'B', metaKey: true });
    expect(screen.queryByRole('navigation', { name: 'Sidebar' })).toBeNull();
    fireEvent.keyDown(window, { key: 'B', ctrlKey: true });
    expect(screen.getByRole('navigation', { name: 'Sidebar' })).toBeTruthy();
    const rich = document.createElement('div');
    rich.setAttribute('contenteditable', 'true');
    document.body.append(rich);
    fireEvent.keyDown(rich, { key: 'b', ctrlKey: true });
    expect(screen.getByRole('navigation', { name: 'Sidebar' })).toBeTruthy();
    rich.remove();
  });

  it('is a sheet below 960px, opened from the top bar', async () => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: query === '(max-width: 959px)',
      media: query,
      addEventListener() {},
      removeEventListener() {},
    }));
    setup();
    expect(screen.queryByRole('navigation', { name: 'Sidebar' })).toBeNull();
    const toggle = screen.getByRole('button', { name: 'Show sidebar' });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    fireEvent.click(toggle);
    expect(await screen.findByRole('navigation', { name: 'Sidebar' })).toBeTruthy();
    expect(screen.getByRole('dialog')).toBeTruthy();
    // The saved desktop state is untouched by the sheet.
    expect(localStorage.getItem(SIDEBAR.key)).toBeNull();
  });

  it('opens the palette from the top bar', async () => {
    setup();
    fireEvent.click(screen.getByRole('button', { name: 'Search or run a command' }));
    expect(await screen.findByRole('dialog')).toBeTruthy();
  });
});
