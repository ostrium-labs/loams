// The sidebar's navigation polish (UI upgrade phase 2): the current page's sliding chip,
// groups that open and shut, the arrow keys between rows, the empty Pages state, the top
// bar's shortcut tooltips, and the panel rule (panels snap unless Motion is on).

import { type SessionService, ThemeController } from '@loams/console-host';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vite-plus/test';
import { Layout } from '../src/layout.js';
import { writePinned } from '../src/pages.js';
import { panelSeconds } from '../src/panel-motion.js';
import { HashRouter, type HashSource } from '../src/router.js';

function memory(start = '/'): HashSource {
  let path = start;
  const ls = new Set<() => void>();
  return {
    get: () => path,
    set: (p) => {
      path = p;
      for (const l of ls) l();
    },
    listen: (l) => {
      ls.add(l);
      return () => ls.delete(l);
    },
  };
}
const session = { principal: () => null, subscribe: () => () => {} } as unknown as SessionService;
const theme = new ThemeController({ read: () => 'dark', write: () => {} });

/** The console's real page set, as the plugins register it. */
const PAGES: [id: string, label: string, href: string, group: string][] = [
  ['overview', 'Overview', '/', 'Data'],
  ['data', 'Data explorer', '/data', 'Data'],
  ['sql', 'SQL', '/sql', 'Data'],
  ['deployments', 'Deployments', '/deployments', 'Compute'],
  ['jobs', 'Jobs', '/jobs', 'Compute'],
  ['connectors', 'Connectors', '/connectors', 'Integrate'],
  ['git', 'Git', '/git', 'Integrate'],
  ['agents', 'Agents', '/projects/:project/agents', 'Project'],
  ['access', 'Access', '/projects/:project/access', 'Organisation'],
  ['teams', 'Teams', '/teams', 'Organisation'],
  ['audit', 'Audit log', '/audit', 'Organisation'],
  ['metrics', 'Metrics', '/metrics', 'Operate'],
];

function setup(start = '/') {
  const slots = new SlotRegistry();
  const router = new HashRouter(slots, memory(start));
  PAGES.forEach(([id, label, href, group], order) => {
    slots.register(
      { name: 'shell.nav.section', plugin: id, order, meta: { id, label, href, group } },
      () => null,
    );
    router.page({ id: `page-${id}`, path: href, title: label, plugin: id }, () => (
      <p>{label} page</p>
    ));
  });
  render(
    <SlotProvider registry={slots}>
      <Layout router={router} session={session} theme={theme} />
    </SlotProvider>,
  );
  return { slots, router };
}

const sidebar = () => within(screen.getByRole('navigation', { name: 'Sidebar' }));

beforeEach(() => localStorage.clear());
afterEach(cleanup);

describe('sidebar navigation (UI2)', () => {
  it('marks the current page with the one sliding chip', () => {
    setup('/deployments');
    const current = sidebar().getByRole('link', { name: 'Deployments' });
    expect(current.getAttribute('aria-current')).toBe('page');
    const chips = document.querySelectorAll('.lc-pages [data-slot="selection-indicator"]');
    expect(chips).toHaveLength(1);
    expect(current.contains(chips[0] as Node)).toBe(true);
  });

  it('opens and shuts the Pages group from its heading, and remembers it', async () => {
    setup('/deployments');
    const heading = sidebar().getByRole('button', { name: 'Pages' });
    expect(heading.getAttribute('aria-expanded')).toBe('true');
    expect(document.getElementById(heading.getAttribute('aria-controls') ?? '')).not.toBeNull();
    fireEvent.click(heading);
    expect(heading.getAttribute('aria-expanded')).toBe('false');
    await waitFor(() => expect(sidebar().queryByRole('link', { name: 'Deployments' })).toBeNull());
    expect(JSON.parse(localStorage.getItem('loams.shell.nav.collapsed') ?? '[]')).toEqual([
      'lc-pages',
    ]);
    cleanup();
    setup('/deployments');
    expect(sidebar().getByRole('button', { name: 'Pages' }).getAttribute('aria-expanded')).toBe(
      'false',
    );
  });

  it('moves between rows with the arrow keys, Home and End, and shuts a group with Left', () => {
    setup('/deployments');
    const rows = [...document.querySelectorAll<HTMLElement>('.lc-pages [data-nav-row]')];
    expect(rows[0]?.textContent).toBe('Pages');
    expect(rows.at(-1)?.textContent).toBe('All pages');
    rows[1]?.focus();
    fireEvent.keyDown(rows[1] as HTMLElement, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(rows[2]);
    fireEvent.keyDown(rows[2] as HTMLElement, { key: 'ArrowUp' });
    expect(document.activeElement).toBe(rows[1]);
    fireEvent.keyDown(rows[1] as HTMLElement, { key: 'End' });
    expect(document.activeElement).toBe(rows.at(-1));
    fireEvent.keyDown(rows.at(-1) as HTMLElement, { key: 'Home' });
    expect(document.activeElement).toBe(rows[0]);
    fireEvent.keyDown(rows[0] as HTMLElement, { key: 'ArrowLeft' });
    expect(rows[0]?.getAttribute('aria-expanded')).toBe('false');
    fireEvent.keyDown(rows[0] as HTMLElement, { key: 'ArrowRight' });
    expect(rows[0]?.getAttribute('aria-expanded')).toBe('true');
  });

  it('says where pins come from when nothing is pinned', () => {
    writePinned(undefined, []);
    setup('/deployments');
    expect(sidebar().getByText('No pinned pages')).toBeTruthy();
    expect(sidebar().getByRole('link', { name: 'All pages' })).toBeTruthy();
  });

  it("names the sidebar toggle's shortcut and controls the sidebar column", () => {
    setup('/deployments');
    const toggle = screen.getByRole('button', { name: 'Hide sidebar' });
    expect(toggle.getAttribute('aria-keyshortcuts')).toBe('Control+B Meta+B');
    expect(document.getElementById(toggle.getAttribute('aria-controls') ?? '')?.className).toBe(
      'lc-side-col',
    );
    fireEvent.click(toggle);
    // Motion off (the default): the sidebar is gone at once, nothing waits for an animation.
    expect(screen.queryByRole('navigation', { name: 'Sidebar' })).toBeNull();
  });
});

describe('panelSeconds (UI1-1)', () => {
  it('reads --op-panel-duration: 0 unless Motion is on', () => {
    const el = document.createElement('div');
    document.body.append(el);
    expect(panelSeconds(el)).toBe(0);
    el.style.setProperty('--op-panel-duration', '300ms');
    expect(panelSeconds(el)).toBeCloseTo(0.3);
    el.style.setProperty('--op-panel-duration', '0ms');
    expect(panelSeconds(el)).toBe(0);
    expect(panelSeconds(null)).toBe(0);
    el.remove();
  });
});
