// Navigation in three places (design 59 D1080, CF1 Task 10): the project's pinned Pages in
// the sidebar, every page in the palette, and the organisation admin in Settings.

import { type SessionService, ThemeController } from '@loams/console-host';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { Layout } from '../src/layout.js';
import {
  DEFAULT_PINNED,
  isSettingsPage,
  pageEntries,
  pinnedPages,
  readPinned,
  settingsHome,
  writePinned,
} from '../src/pages.js';
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

const sections = () =>
  PAGES.map(([id, label, href, group], order) => ({
    id: order,
    order,
    plugin: id,
    name: 'shell.nav.section' as const,
    meta: { id, label, href, group },
    component: () => null,
  }));

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

const pagesList = () =>
  within(screen.getByRole('navigation', { name: 'Sidebar' })).getByRole('list', { name: 'Pages' });

beforeEach(() => {
  localStorage.clear();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('pages (D1080)', () => {
  it('fills :project, hides project pages without a project, and classifies Settings', () => {
    const none = pageEntries(sections(), [], undefined);
    expect(none.find((p) => p.id === 'agents')).toBeUndefined();
    const acme = pageEntries(sections(), [], 'acme');
    expect(acme.find((p) => p.id === 'agents')?.href).toBe('/projects/acme/agents');
    expect(
      acme
        .filter(isSettingsPage)
        .map((p) => p.id)
        .sort(),
    ).toEqual(['access', 'audit', 'metrics', 'teams']);
    expect(pinnedPages(acme, DEFAULT_PINNED).map((p) => p.label)).toEqual([
      'Data explorer',
      'Deployments',
      'Git',
      'Agents',
      'Connectors',
    ]);
    expect(settingsHome(acme)?.id).toBe('teams');
  });

  it('leaves the shell`s Preferences out where the desktop`s Settings page lists the sections', () => {
    const entry = (id: string, label: string, href: string, group: string, order: number) => ({
      id: order,
      order,
      plugin: id,
      name: 'shell.nav.section' as const,
      meta: { id, label, href, group },
      component: () => null,
    });
    const web = [entry('preferences', 'Preferences', '/preferences', 'Settings', 1)];
    expect(settingsHome(pageEntries(web, [], undefined))?.id).toBe('preferences');
    const desktop = [...web, entry('settings', 'Settings', '/settings', 'Organisation', 2)];
    const all = pageEntries(desktop, [], undefined);
    expect(all.map((p) => p.id)).toEqual(['settings']);
    expect(settingsHome(all)?.id).toBe('settings');
  });

  it('keeps pins per project, and falls back to the defaults (Q932)', () => {
    expect(readPinned('acme')).toEqual([...DEFAULT_PINNED]);
    writePinned('acme', ['sql']);
    expect(readPinned('acme')).toEqual(['sql']);
    expect(readPinned('other')).toEqual([...DEFAULT_PINNED]);
    localStorage.setItem('loams.shell.pages.broken', '{');
    expect(readPinned('broken')).toEqual([...DEFAULT_PINNED]);
  });

  it('the sidebar lists the pinned Pages and no section headings', () => {
    setup('/projects/acme');
    const sidebar = screen.getByRole('navigation', { name: 'Sidebar' });
    expect(
      within(pagesList())
        .getAllByRole('link')
        .map((a) => a.textContent),
    ).toEqual(['Data explorer', 'Deployments', 'Git', 'Agents', 'Connectors']);
    for (const heading of ['Data', 'Compute', 'Organisation', 'Operate', 'Integrate']) {
      expect(within(sidebar).queryByRole('heading', { name: heading })).toBeNull();
    }
  });

  it('marks the current page, the most specific one', () => {
    setup('/projects/acme/agents');
    const current = within(pagesList())
      .getAllByRole('link')
      .filter((a) => a.getAttribute('aria-current') === 'page');
    expect(current.map((a) => a.textContent)).toEqual(['Agents']);
  });

  it('pins and unpins a page from the sidebar', async () => {
    setup('/projects/acme');
    fireEvent.click(screen.getByRole('button', { name: 'Edit pages' }));
    const sql = await screen.findByRole('menuitemcheckbox', { name: /SQL/ });
    expect(sql.getAttribute('aria-checked')).toBe('false');
    fireEvent.click(sql);
    expect(readPinned('acme')).toContain('sql');
    expect(within(pagesList()).getByRole('link', { name: 'SQL' })).toBeTruthy();
    // Settings pages are not pinnable: Settings lists them.
    expect(screen.queryByRole('menuitemcheckbox', { name: /Teams/ })).toBeNull();
    fireEvent.click(await screen.findByRole('menuitemcheckbox', { name: /Git/ }));
    expect(within(pagesList()).queryByRole('link', { name: 'Git' })).toBeNull();
  });

  it('Settings holds organisation admin, with a way back', () => {
    const { router } = setup('/data');
    // Links are plain hash links; the test's router reads a memory source, so it navigates itself.
    expect(screen.getByRole('link', { name: 'Settings' }).getAttribute('href')).toBe('#/teams');
    act(() => router.navigate('/teams'));
    const settings = screen.getByRole('navigation', { name: 'Settings' });
    expect(
      within(settings)
        .getAllByRole('link')
        .map((a) => a.textContent),
    ).toEqual(expect.arrayContaining(['Teams', 'Audit log', 'Metrics']));
    expect(screen.queryByRole('list', { name: 'Pages' })).toBeNull();
    expect(within(settings).getByRole('link', { name: /Back/ }).getAttribute('href')).toBe(
      '#/data',
    );
    // Moving within Settings keeps the way back.
    act(() => router.navigate('/audit'));
    expect(
      within(screen.getByRole('navigation', { name: 'Settings' }))
        .getByRole('link', { name: /Back/ })
        .getAttribute('href'),
    ).toBe('#/data');
  });

  it('every page keeps its route and is in the palette', async () => {
    const { router } = setup('/');
    for (const [, label, href] of PAGES) {
      if (href.includes(':project')) continue;
      act(() => router.navigate(href));
      expect(router.current().pageId).toBe(`page-${PAGES.find((p) => p[2] === href)?.[0]}`);
      expect(screen.getByText(`${label} page`)).toBeTruthy();
    }
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    fireEvent.change(await screen.findByRole('combobox'), { target: { value: 'metrics' } });
    expect(await screen.findByRole('option', { name: /Go to Metrics/ })).toBeTruthy();
  });

  it('pins a page from the palette, and the sidebar follows', async () => {
    setup('/projects/acme');
    fireEvent.keyDown(window, { key: 'k', ctrlKey: true });
    fireEvent.change(await screen.findByRole('combobox'), { target: { value: 'pin jobs' } });
    fireEvent.click(await screen.findByRole('option', { name: /Pin Jobs to the sidebar/ }));
    expect(readPinned('acme')).toContain('jobs');
    expect(within(pagesList()).getByRole('link', { name: 'Jobs' })).toBeTruthy();
  });
});
