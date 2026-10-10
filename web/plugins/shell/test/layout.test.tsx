import type { SessionService } from '@loams/console-host';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { useEffect } from 'react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { DOCK_DEFAULT, DOCK_KEY, DOCK_MAX, DOCK_MIN, Layout } from '../src/layout.js';
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

const who = { displayName: 'Omar Haddad' };
const session = {
  principal: () => who,
  subscribe: () => () => {},
} as unknown as SessionService;

function setup(start = '/') {
  const slots = new SlotRegistry();
  const router = new HashRouter(slots, memory(start));
  const ui = render(
    <SlotProvider registry={slots}>
      <Layout router={router} session={session} />
    </SlotProvider>,
  );
  return { slots, router, ui };
}

const section = (
  slots: SlotRegistry,
  label: string,
  group: string,
  order: number,
  href = `/${label.toLowerCase()}`,
) =>
  slots.register(
    { name: 'shell.nav.section', plugin: 'test', order, meta: { id: label, label, group, href } },
    () => null,
  );

beforeEach(() => {
  localStorage.clear();
  // The Logo's grain canvas observes its size; jsdom has no ResizeObserver.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('shell layout', () => {
  it('nav_groups_and_orders_sections', () => {
    const { slots } = setup();
    act(() => {
      section(slots, 'Connectors', 'Integrate', 10);
      section(slots, 'Settings', 'Organisation', 20);
      section(slots, 'Live', 'Data', 40);
      section(slots, 'Postgres', 'Data', 20);
      section(slots, 'Durable', 'Compute', 10);
      section(slots, 'Cloud', 'Organisation', 10);
    });
    const nav = screen.getByRole('navigation', { name: 'Console' });
    const headings = [...nav.querySelectorAll('.lc-nav-heading')].map((h) => h.textContent);
    expect(headings).toEqual(['Data', 'Compute', 'Integrate', 'Organisation']);
    const labels = within(nav)
      .getAllByRole('link')
      .map((a) => a.textContent);
    expect(labels).toEqual(['Postgres', 'Live', 'Durable', 'Connectors', 'Cloud', 'Settings']);
  });

  it('existing_routes_still_render', () => {
    const { slots, router } = setup('/');
    act(() => {
      router.page(
        { id: 'status', path: '/', title: 'Status', nav: { group: 'Instance', order: 0 } },
        () => <p>status page</p>,
      );
      router.page(
        {
          id: 'ns',
          path: '/namespaces',
          title: 'Namespaces',
          nav: { group: 'Instance', order: 10 },
        },
        () => <p>ns page</p>,
      );
      router.page(
        { id: 'apr', path: '/approvals', title: 'Approvals', nav: { group: 'Operate', order: 20 } },
        () => <p>apr page</p>,
      );
      router.page(
        { id: 'odd', path: '/odd', title: 'Odd', nav: { group: 'Weird', order: 1 } },
        () => <p>odd page</p>,
      );
    });
    expect(screen.getByText('status page')).toBeTruthy();
    const nav = screen.getByRole('navigation', { name: 'Console' });
    const sectionOf = (name: string) =>
      within(nav)
        .getByRole('link', { name })
        .closest('.lc-nav-group')
        ?.querySelector('.lc-nav-heading')?.textContent;
    expect(sectionOf('Status')).toBe('Data');
    expect(sectionOf('Namespaces')).toBe('Data');
    expect(sectionOf('Approvals')).toBe('Organisation');
    expect(sectionOf('Odd')).toBe('Organisation');
    expect(within(nav).getByRole('link', { name: 'Status' }).getAttribute('aria-current')).toBe(
      'page',
    );
    expect(slots.entries('console.nav')).toHaveLength(4);
  });

  it('hides the dock and its toggle while the slot is empty', () => {
    setup();
    expect(screen.queryByRole('button', { name: /agent/i })).toBeNull();
    expect(screen.queryByRole('complementary')).toBeNull();
    fireEvent.keyDown(window, { key: 'j', ctrlKey: true });
    expect(screen.queryByRole('complementary')).toBeNull();
  });

  it('dock_toggle_and_shortcut', () => {
    const { slots } = setup();
    act(() => {
      slots.register({ name: 'shell.dock.right', plugin: 'agent' }, () => <p>chat here</p>);
    });
    const toggle = screen.getByRole('button', { name: /agent/i });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByText('chat here')).toBeNull();
    fireEvent.click(toggle);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByText('chat here')).toBeTruthy();
    expect(screen.getByRole('complementary', { name: /agent/i })).toBeTruthy();
    fireEvent.keyDown(window, { key: 'j', ctrlKey: true });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    fireEvent.keyDown(window, { key: 'J', metaKey: true });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    fireEvent.keyDown(window, { key: 'j' });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
  });

  it('dock_stays_mounted_when_closed_and_manages_focus', () => {
    const { slots } = setup();
    let mounts = 0;
    function Chat() {
      useEffect(() => {
        mounts++;
      }, []);
      return <textarea data-dock-autofocus aria-label="Message" />;
    }
    act(() => {
      slots.register({ name: 'shell.dock.right', plugin: 'agent' }, () => <Chat />);
    });
    const toggle = screen.getByRole('button', { name: /agent/i });
    expect(mounts).toBe(0);
    fireEvent.click(toggle);
    expect(document.activeElement).toBe(screen.getByLabelText('Message'));
    fireEvent.change(screen.getByLabelText('Message'), { target: { value: 'draft' } });
    fireEvent.keyDown(window, { key: 'j', ctrlKey: true });
    expect(screen.queryByRole('complementary')).toBeNull();
    expect(document.getElementById('lc-dock')?.hidden).toBe(true);
    expect(document.activeElement).toBe(toggle);
    fireEvent.keyDown(window, { key: 'j', ctrlKey: true });
    expect((screen.getByLabelText('Message') as HTMLTextAreaElement).value).toBe('draft');
    expect(mounts).toBe(1);
    expect(document.activeElement).toBe(screen.getByLabelText('Message'));
  });

  it('dock_width_clamped_and_persisted', () => {
    const { slots } = setup();
    act(() => {
      slots.register({ name: 'shell.dock.right', plugin: 'agent' }, () => <p>chat</p>);
    });
    fireEvent.click(screen.getByRole('button', { name: /agent/i }));
    const handle = screen.getByRole('separator');
    expect(Number(handle.getAttribute('aria-valuenow'))).toBe(DOCK_DEFAULT);
    expect(handle.getAttribute('aria-valuemin')).toBe(String(DOCK_MIN));
    expect(handle.getAttribute('aria-valuemax')).toBe(String(DOCK_MAX));
    for (let i = 0; i < 100; i++) fireEvent.keyDown(handle, { key: 'ArrowLeft' });
    expect(handle.getAttribute('aria-valuenow')).toBe(String(DOCK_MAX));
    for (let i = 0; i < 100; i++) fireEvent.keyDown(handle, { key: 'ArrowRight' });
    expect(handle.getAttribute('aria-valuenow')).toBe(String(DOCK_MIN));
    fireEvent.keyDown(handle, { key: 'ArrowLeft' });
    const saved = JSON.parse(localStorage.getItem(DOCK_KEY) ?? '{}');
    expect(saved.open).toBe(true);
    expect(saved.width).toBeGreaterThan(DOCK_MIN);
    expect(saved.width).toBeLessThanOrEqual(DOCK_MAX);

    // A fresh mount restores it, and a corrupt value falls back and clamps.
    cleanup();
    setupWithDock();
    expect(screen.getByRole('separator').getAttribute('aria-valuenow')).toBe(String(saved.width));
    cleanup();
    localStorage.setItem(DOCK_KEY, JSON.stringify({ open: true, width: 99999 }));
    setupWithDock();
    expect(screen.getByRole('separator').getAttribute('aria-valuenow')).toBe(String(DOCK_MAX));
    cleanup();
    localStorage.setItem(DOCK_KEY, '{nope');
    setupWithDock();
    expect(screen.queryByRole('separator')).toBeNull();
  });
});

function setupWithDock() {
  const r = setup();
  act(() => {
    r.slots.register({ name: 'shell.dock.right', plugin: 'agent' }, () => <p>chat</p>);
  });
  return r;
}
