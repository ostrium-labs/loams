// The shell's side of the modes (SD1 Task 3, §53 §4): the switcher, the (deprecated) nav
// model by mode, Pages the same in both modes (D1080), and a route of the other mode
// switching the mode first.

import {
  MODE_HOME,
  type Mode,
  ModeController,
  type ModeService,
  type SessionService,
} from '@loams/console-host';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import { Layout, navSections } from '../src/layout.js';
import { syncModeWithRoute } from '../src/mode-switcher.js';
import { writePinned } from '../src/pages.js';
import { HashRouter, type HashSource } from '../src/router.js';

function memory(start = '/'): HashSource & { go(path: string): void } {
  let path = start;
  const listeners = new Set<() => void>();
  return {
    get: () => path,
    set: (p) => {
      path = p;
    },
    listen: (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    go(p) {
      path = p;
      for (const l of listeners) l();
    },
  };
}

const session = {
  principal: () => undefined,
  subscribe: () => () => {},
} as unknown as SessionService;

function modeService(
  available: readonly Mode[] = ['console', 'studio'],
  current: Mode = 'console',
  navigate: (to: string) => void = () => {},
) {
  return new ModeController({
    current,
    available,
    modes: new Map<string, readonly Mode[]>([
      ['pages', ['console']],
      ['studio', ['studio']],
      ['approvals', ['console', 'studio']],
    ]),
    navigate,
    enter: () => {},
  });
}

function setup(mode: ModeService, start = '/') {
  const slots = new SlotRegistry();
  const source = memory(start);
  const router = new HashRouter(slots, source);
  render(
    <SlotProvider registry={slots}>
      <Layout router={router} session={session} mode={mode} />
    </SlotProvider>,
  );
  return { slots, router, source };
}

const entry = (slots: SlotRegistry, plugin: string, label: string, group: string, mode?: Mode) =>
  slots.register(
    {
      name: 'shell.nav.section',
      plugin,
      order: 10,
      meta: { id: label, label, group, href: `/${label.toLowerCase()}`, mode },
    },
    () => null,
  );

const navLabels = () => [...document.querySelectorAll('.lc-pages ul a')].map((a) => a.textContent);

beforeEach(() => {
  localStorage.clear();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('nav by mode', () => {
  it('the pinned Pages are the same in both modes (D1080)', () => {
    const mode = modeService();
    const { slots } = setup(mode);
    act(() => {
      writePinned(undefined, ['Postgres', 'Projects']);
      entry(slots, 'pages', 'Postgres', 'Data');
      entry(slots, 'studio', 'Projects', 'Studio');
    });
    expect(navLabels()).toEqual(['Postgres', 'Projects']);
    act(() => mode.set('studio'));
    expect(navLabels()).toEqual(['Postgres', 'Projects']);
  });

  it('marks only the most specific entry current', () => {
    const mode = modeService(['console', 'studio'], 'studio');
    const { slots } = setup(mode, '/studio/runs/r1');
    act(() => {
      writePinned(undefined, ['Runs', 'Run']);
      for (const [label, href] of [
        ['Runs', '/studio/runs'],
        ['Run', '/studio/runs/r1'],
      ]) {
        slots.register(
          {
            name: 'shell.nav.section',
            plugin: 'studio',
            meta: { id: label, label, href, group: 'Studio', mode: 'studio' },
          },
          () => null,
        );
      }
    });
    const current = [...document.querySelectorAll('.lc-pages a[aria-current="page"]')];
    expect(current.map((a) => a.textContent)).toEqual(['Run']);
  });

  it('meta_mode_overrides_plugin_modes', () => {
    const slots = new SlotRegistry();
    entry(slots, 'approvals', 'Approvals', 'Organisation', 'console');
    entry(slots, 'approvals', 'Agent', 'Studio', 'studio');
    entry(slots, 'approvals', 'Inbox', 'Organisation');
    const sections = slots.entries('shell.nav.section');
    const labels = (m: Mode) =>
      navSections(sections, [], { mode: m, modesOf: modeService().modesOf.bind(modeService()) })
        .flatMap(([, items]) => items)
        .map((i) => i.label);
    expect(labels('console')).toEqual(['Approvals', 'Inbox']);
    expect(labels('studio').sort()).toEqual(['Agent', 'Inbox']);
  });

  it('studio_groups_keep_their_order', () => {
    // The factory's group can register before Studio's own (§53 §5.9, SD1 Task 27).
    const slots = new SlotRegistry();
    const add = (plugin: string, label: string, href: string, group: string) =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin,
          order: 0,
          meta: { id: label, label, href, group, mode: 'studio' },
        },
        () => null,
      );
    add('factory', 'Software Factory', '/studio/factory', 'Factory');
    add('studio', 'Home', '/studio', 'Studio');
    const modes = modeService();
    const groups = navSections(slots.entries('shell.nav.section'), [], {
      mode: 'studio',
      modesOf: (id: string) => modes.modesOf(id),
    }).map(([g]) => g);
    expect(groups).toEqual(['Studio', 'Factory']);
  });

  it('a legacy page under /studio is Studio`s', () => {
    const slots = new SlotRegistry();
    slots.register(
      { name: 'console.nav', plugin: 'shell', order: 1, meta: { label: 'Home', href: '/studio' } },
      () => null,
    );
    const legacy = slots.entries('console.nav');
    const modes = modeService();
    const filter = (m: Mode) => ({ mode: m, modesOf: (id: string) => modes.modesOf(id) });
    expect(navSections([], legacy, filter('console'))).toEqual([]);
    expect(
      navSections([], legacy, filter('studio')).flatMap(([, i]) => i.map((x) => x.label)),
    ).toEqual(['Home']);
  });
});

describe('breadcrumbs in Studio', () => {
  it('start at Studio and repeat neither the section nor the home page', () => {
    const mode = modeService(['console', 'studio'], 'studio');
    const { slots, router, source } = setup(mode, '/studio/t/c_1');
    act(() => {
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'studio',
          order: 0,
          meta: { id: 'h', label: 'Home', group: 'Studio', href: '/studio', mode: 'studio' },
        },
        () => null,
      );
      router.page({ id: 'studio', path: '/studio', title: 'Studio', plugin: 'studio' }, () => null);
      router.page(
        { id: 't', path: '/studio/t/:id', title: 'Thread', plugin: 'studio' },
        () => null,
      );
    });
    const crumbs = () =>
      [...screen.getByRole('navigation', { name: 'Breadcrumb' }).querySelectorAll('li')].map(
        (li) => li.textContent,
      );
    expect(crumbs()).toEqual(['Studio', 'Thread']);
    act(() => source.go('/studio'));
    expect(crumbs()).toEqual(['Studio']);
  });
});

describe('the switcher', () => {
  it('switcher_hidden_with_one_mode', () => {
    setup(modeService(['console']));
    expect(screen.queryByRole('group', { name: 'Mode' })).toBeNull();
  });

  it('switches from the header and shows the current mode', () => {
    const navigated: string[] = [];
    const mode = modeService(['console', 'studio'], 'console', (to) => navigated.push(to));
    setup(mode);
    const group = screen.getByRole('group', { name: 'Mode' });
    const studio = screen.getByRole('button', { name: 'Studio' });
    expect(group.getAttribute('aria-keyshortcuts')).toBe('Control+Shift+M Meta+Shift+M');
    fireEvent.click(studio);
    expect(mode.current).toBe('studio');
    expect(navigated).toEqual([MODE_HOME.studio]);
    expect(studio.getAttribute('aria-pressed')).toBe('true');
    expect(document.querySelector('.lc-shell')?.getAttribute('data-mode')).toBe('studio');
  });

  it('switcher_keyboard_shortcut', () => {
    const mode = modeService();
    setup(mode);
    fireEvent.keyDown(window, { key: 'M', ctrlKey: true, shiftKey: true });
    expect(mode.current).toBe('studio');
    fireEvent.keyDown(window, { key: 'm', metaKey: true, shiftKey: true });
    expect(mode.current).toBe('console');
    // Without Shift, or with Alt, it is not the shortcut.
    fireEvent.keyDown(window, { key: 'm', ctrlKey: true });
    fireEvent.keyDown(window, { key: 'm', ctrlKey: true, shiftKey: true, altKey: true });
    expect(mode.current).toBe('console');
  });

  it('the shortcut does nothing with one mode', () => {
    const mode = modeService(['console']);
    setup(mode);
    fireEvent.keyDown(window, { key: 'm', ctrlKey: true, shiftKey: true });
    expect(mode.current).toBe('console');
  });
});

describe('routes and modes', () => {
  it('studio_route_switches_mode', () => {
    const navigated: string[] = [];
    const mode = modeService(['console', 'studio'], 'console', (to) => navigated.push(to));
    const slots = new SlotRegistry();
    const source = memory('/');
    const router = new HashRouter(slots, source);
    const off = syncModeWithRoute(router, mode);
    act(() => source.go('/studio/t/c_1?panel=diff'));
    expect(mode.current).toBe('studio');
    // The switch keeps the route it came from.
    expect(navigated).toEqual(['/studio/t/c_1?panel=diff']);
    act(() => source.go('/approvals'));
    expect(mode.current).toBe('console');
    off();
  });

  it('a studio route without Studio stays in Console', () => {
    const mode = modeService(['console']);
    const slots = new SlotRegistry();
    const source = memory('/');
    const router = new HashRouter(slots, source);
    syncModeWithRoute(router, mode);
    source.go('/studio');
    expect(mode.current).toBe('console');
  });

  it('a Console route leaves Studio', () => {
    const mode = modeService(['console', 'studio'], 'studio');
    const slots = new SlotRegistry();
    const source = memory('/studio');
    const router = new HashRouter(slots, source);
    syncModeWithRoute(router, mode);
    source.go('/approvals');
    expect(mode.current).toBe('console');
  });
});
