import { type SessionService, ThemeController } from '@loams/console-host';
import { type PanelTabProps, SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import { Layout } from '../src/layout.js';
import { DRAWER, INSPECTOR, PanelController } from '../src/panel.js';
import { tabShows } from '../src/regions.js';
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

function setup(start = '/') {
  const slots = new SlotRegistry();
  const router = new HashRouter(slots, memory(start));
  const inspector = new PanelController(INSPECTOR);
  const drawer = new PanelController(DRAWER);
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
  return { slots, router, inspector, drawer };
}

beforeEach(() => {
  localStorage.clear();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('PanelController', () => {
  it('opens a tab with props, toggles, clamps its size and persists all but the props', () => {
    const p = new PanelController(INSPECTOR);
    p.open({ tab: 'studio.diff', props: { file: 'a.ts' } });
    expect(p.state()).toMatchObject({
      open: true,
      tab: 'studio.diff',
      props: { file: 'a.ts' },
      size: 280,
    });
    p.toggle('studio.files');
    expect(p.state()).toMatchObject({ open: true, tab: 'studio.files', props: {} });
    p.toggle('studio.files');
    expect(p.state().open).toBe(false);
    p.resize(10_000);
    expect(p.state().size).toBe(640);
    p.resize(10);
    expect(p.state().size).toBe(240);
    const saved = JSON.parse(localStorage.getItem(INSPECTOR.key) ?? '{}');
    expect(saved).toEqual({ open: false, tab: 'studio.files', size: 240 });
  });

  it('survives a corrupt saved state', () => {
    localStorage.setItem(DRAWER.key, '{not json');
    expect(new PanelController(DRAWER).state()).toEqual({
      open: false,
      tab: undefined,
      props: {},
      size: 280,
    });
  });
});

describe('tab routes', () => {
  it('a tab with routes shows only on matching routes', () => {
    const entry = { meta: { routes: ['/studio/t/:threadId'] } };
    expect(tabShows(entry, '/studio/t/c_1')).toBe(true);
    expect(tabShows(entry, '/data')).toBe(false);
    expect(tabShows({ meta: {} }, '/data')).toBe(true);
  });
});

describe('the inspector (D1078)', () => {
  it('inspector.open({ tab, props }) shows that tab with its props, as "Open ↗" does', () => {
    const { slots, inspector } = setup('/data');
    act(() => {
      slots.register(
        { name: 'shell.inspector.tab', plugin: 't', meta: { id: 'details', label: 'Details' } },
        ({ props }: PanelTabProps) => <p>details of {String(props.table ?? 'nothing')}</p>,
      );
      slots.register(
        { name: 'shell.inspector.tab', plugin: 't', meta: { id: 'sql', label: 'SQL' } },
        () => <p>sql tab</p>,
      );
    });
    act(() => inspector.open({ tab: 'details', props: { table: 'orders' } }));
    const region = screen.getByRole('complementary', { name: 'Inspector' });
    expect(region.textContent).toContain('details of orders');
    fireEvent.click(screen.getByRole('tab', { name: 'SQL' }));
    expect(screen.getByText('sql tab')).toBeTruthy();
    expect(screen.getByRole('tabpanel', { name: 'SQL' }).textContent).toBe('sql tab');
  });

  it('hides tabs whose routes do not match, and its toggle when none apply', () => {
    const { slots, router } = setup('/data');
    act(() => {
      slots.register(
        {
          name: 'shell.inspector.tab',
          plugin: 'studio',
          meta: { id: 'studio.diff', label: 'Diff', routes: ['/studio/t/:threadId'] },
        },
        () => <p>diff</p>,
      );
    });
    expect(screen.queryByRole('button', { name: /Inspector/ })).toBeNull();
    act(() => router.navigate('/studio/t/c_1'));
    // A thread opens the inspector on Diff by itself (T9-6).
    expect(screen.getByRole('button', { name: 'Hide inspector' })).toBeTruthy();
    expect(screen.getByRole('tab', { name: 'Diff' })).toBeTruthy();
  });

  it('a narrow window does not open the thread inspector by itself (it would cover the thread)', () => {
    const p = new PanelController(INSPECTOR);
    p.route('/studio/t/c_1', { narrow: true });
    expect(p.state()).toMatchObject({ open: false, tab: 'studio.diff', size: 420 });
    p.route('/studio/t/c_1');
    expect(p.state().open).toBe(true);
  });

  it('a Studio thread opens the inspector on Diff at 420px, and keeps the person`s choice there (T9-6)', () => {
    const diffTab = (slots: SlotRegistry) =>
      act(() => {
        slots.register(
          {
            name: 'shell.inspector.tab',
            plugin: 'studio',
            meta: { id: 'studio.diff', label: 'Diff', routes: ['/studio/t/:threadId'] },
          },
          () => <p>diff</p>,
        );
        slots.register(
          { name: 'shell.inspector.tab', plugin: 'agent', meta: { id: 'agent', label: 'Agent' } },
          () => <p>agent</p>,
        );
      });
    const first = setup('/studio/t/c_1');
    diffTab(first.slots);
    expect(first.inspector.state()).toMatchObject({ open: true, tab: 'studio.diff', size: 420 });
    expect(screen.getByRole('tab', { name: 'Diff' }).getAttribute('aria-selected')).toBe('true');
    // Other routes keep the closed 280px default.
    act(() => first.router.navigate('/data'));
    expect(first.inspector.state()).toMatchObject({ open: false, size: 280 });
    act(() => first.inspector.open({ tab: 'agent' }));
    // Back on a thread: the thread profile again, not the page's choice.
    act(() => first.router.navigate('/studio/t/c_2'));
    expect(first.inspector.state()).toMatchObject({ open: true, tab: 'studio.diff', size: 420 });
    // The person resizes, then closes it on a thread.
    act(() => first.inspector.resize(500));
    act(() => first.inspector.close());
    cleanup();
    // Next time (a reload, this device): the thread keeps that choice, and the page keeps its own.
    const again = setup('/studio/t/c_1');
    diffTab(again.slots);
    expect(again.inspector.state()).toMatchObject({ open: false, size: 500 });
    act(() => again.router.navigate('/data'));
    expect(again.inspector.state()).toMatchObject({ open: true, tab: 'agent', size: 280 });
  });

  it("keeps the agent tab mounted, so the agent's chat state survives closing it", () => {
    const { slots, inspector } = setup('/');
    function Chat() {
      const [n, setN] = useState(0);
      return (
        <button type="button" onClick={() => setN(n + 1)}>
          sent {n}
        </button>
      );
    }
    act(() => {
      slots.register(
        { name: 'shell.inspector.tab', plugin: 'agent', meta: { id: 'agent', label: 'Agent' } },
        Chat,
      );
    });
    act(() => inspector.open({ tab: 'agent' }));
    fireEvent.click(screen.getByRole('button', { name: 'sent 0' }));
    act(() => inspector.close());
    act(() => inspector.open({ tab: 'agent' }));
    expect(screen.getByRole('button', { name: 'sent 1' })).toBeTruthy();
  });

  it('resizes with the keyboard within 240–640', () => {
    const { slots, inspector } = setup('/');
    act(() => {
      slots.register(
        { name: 'shell.inspector.tab', plugin: 't', meta: { id: 'x', label: 'X' } },
        () => <p>x</p>,
      );
    });
    act(() => inspector.open({ tab: 'x' }));
    const handle = screen.getByRole('separator', { name: 'Resize inspector' });
    fireEvent.keyDown(handle, { key: 'End' });
    expect(inspector.state().size).toBe(640);
  });
});

describe('the drawer (D1078)', () => {
  it('opens below main with its tabs', () => {
    const { slots, drawer } = setup('/');
    act(() => {
      slots.register(
        { name: 'shell.drawer.tab', plugin: 't', meta: { id: 'logs', label: 'Logs' } },
        () => <p>log lines</p>,
      );
    });
    fireEvent.click(screen.getByRole('button', { name: 'Show drawer' }));
    expect(drawer.state().open).toBe(true);
    expect(screen.getByRole('region', { name: 'Drawer' }).textContent).toContain('log lines');
  });
});

describe('panel keys and focus', () => {
  it('Mod+I and Mod+J toggle the panels, but rich text and a terminal keep them', () => {
    const { slots, inspector, drawer } = setup('/');
    act(() => {
      slots.register(
        { name: 'shell.inspector.tab', plugin: 't', meta: { id: 'x', label: 'X' } },
        () => <p>x</p>,
      );
      slots.register(
        { name: 'shell.drawer.tab', plugin: 't', meta: { id: 'y', label: 'Y' } },
        () => <p>y</p>,
      );
    });
    fireEvent.keyDown(window, { key: 'i', ctrlKey: true });
    expect(inspector.state().open).toBe(true);
    fireEvent.keyDown(window, { key: 'J', ctrlKey: true });
    expect(drawer.state().open).toBe(true);
    fireEvent.keyDown(window, { key: 'j' });
    expect(drawer.state().open).toBe(true);
    const rich = document.createElement('div');
    rich.setAttribute('contenteditable', 'true');
    const term = document.createElement('div');
    term.className = 'xterm';
    document.body.append(rich, term);
    fireEvent.keyDown(rich, { key: 'i', ctrlKey: true });
    fireEvent.keyDown(term, { key: 'j', ctrlKey: true });
    expect(inspector.state().open).toBe(true);
    expect(drawer.state().open).toBe(true);
    rich.remove();
    term.remove();
  });

  it("focuses the tab's marked field on open, and the toggle when it closes from inside", () => {
    const { slots, inspector } = setup('/');
    act(() => {
      slots.register(
        { name: 'shell.inspector.tab', plugin: 'agent', meta: { id: 'agent', label: 'Agent' } },
        () => <textarea data-panel-autofocus aria-label="Message" />,
      );
    });
    fireEvent.click(screen.getByRole('button', { name: 'Show inspector' }));
    expect(document.activeElement).toBe(screen.getByLabelText('Message'));
    act(() => inspector.close());
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Show inspector' }));
  });

  it('shows an old shell.dock.right entry as an inspector tab', () => {
    const { slots, inspector } = setup('/');
    act(() => {
      slots.register({ name: 'shell.dock.right', plugin: 'old' }, () => <p>old dock</p>);
    });
    act(() => inspector.open());
    expect(screen.getByRole('complementary', { name: 'Inspector' }).textContent).toContain(
      'old dock',
    );
  });
});
