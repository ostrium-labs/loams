// The factory under Studio (SD1 Task 27; §53 §5.9, D916, D922): its manifest and
// nav entries, the redirects from the old `/factory` paths, and the in-context
// hand-offs from a Plane issue and a GlitchTip error to a Studio thread draft.

import { validateManifest } from '@loams/console-host';
import type { SlotSpec } from '@loams/slots';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ComponentType } from 'react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import pkg from '../package.json';
import { branchFor, threadDraftHref } from '../src/drafts.js';
import plugin from '../src/index.js';
import { PanelsPage } from '../src/panels-page.js';
import { legacyTarget } from '../src/paths.js';
import { fakeDesktop, IDS, info } from './fake.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

type Page = { spec: { id: string; path: string }; component: ComponentType<never> };

/** Applies the plugin against plain-object services; returns what it registered. */
function apply() {
  const pages: Page[] = [];
  const slots: SlotSpec<never>[] = [];
  const navigated: { to: string; replace?: boolean }[] = [];
  let query: Record<string, string> | undefined;
  let path = '/';
  const router = {
    page: (spec: Page['spec'], component: Page['component']) => {
      pages.push({ spec, component });
      return () => {};
    },
    navigate: (to: string, opts?: { replace?: boolean }) =>
      void navigated.push({ to, ...(opts?.replace ? { replace: true } : {}) }),
    current: () => ({ path, params: {}, ...(query ? { query } : {}) }),
    subscribe: () => () => {},
  };
  const ctx = {
    effect: (fn: () => unknown) => void fn(),
    desktop: fakeDesktop().api,
    platform: { kind: 'desktop', openExternal: () => {} },
    router,
    slots: {
      register: (spec: SlotSpec<never>) => {
        slots.push(spec);
        return () => {};
      },
    },
  };
  plugin.apply(ctx as never, {});
  const at = (p: string, q?: Record<string, string>) => {
    path = p;
    query = q;
  };
  return { pages, slots, navigated, at };
}

const nav = () => {
  const to: string[] = [];
  return { to, navigate: (p: string) => void to.push(p) };
};

/** The draft a Studio href carries, as Studio's home route reads it. */
function draftOf(href: string) {
  const [path, search] = href.split('?');
  return { path, q: Object.fromEntries(new URLSearchParams(search ?? '')) };
}

describe('the factory in Studio', () => {
  it('factory_in_studio_mode_only', () => {
    const m = validateManifest(pkg);
    // Studio is its home; Console keeps only the overview card (meta.mode below).
    expect(m.modes).toEqual(['console', 'studio']);
    // Every edition since SF1 (amendment F1-F6): the desktop embeds the apps, a
    // browser reads through Studio's bridge to an agentd host.
    expect(m.editions).toEqual(['oss', 'desktop', 'cloud']);
    const { pages, slots } = apply();
    const nav = slots.filter((s) => s.name === 'shell.nav.section');
    expect(nav).toHaveLength(2);
    expect(nav[0]?.meta).toMatchObject({
      href: '/studio/factory',
      group: 'Factory',
      mode: 'studio',
    });
    // The Runs page (FT0 Task 9).
    expect(nav[1]?.meta).toMatchObject({
      href: '/studio/factory/runs',
      group: 'Factory',
      mode: 'studio',
    });
    // Every page but the redirects lives under /studio/factory.
    const live = pages.filter((p) => !p.spec.id.startsWith('factory-legacy'));
    expect(live.map((p) => p.spec.path).sort()).toEqual([
      '/studio/factory',
      '/studio/factory/:app',
      '/studio/factory/:app/app',
      '/studio/factory/:app/configure',
      '/studio/factory/runs',
    ]);
  });

  it('factory_overview_card_in_console', () => {
    const { slots } = apply();
    const cards = slots.filter((s) => s.name === 'environment.overview.card');
    expect(cards).toHaveLength(1);
    expect(cards[0]?.meta?.mode).toBe('console');
    expect(validateManifest(pkg).modes).toContain('console');
  });

  it('factory_redirects_old_paths', () => {
    expect(legacyTarget('/factory')).toBe('/studio/factory');
    expect(legacyTarget('/factory/forgejo')).toBe('/studio/factory/forgejo');
    expect(legacyTarget('/factory/forgejo/app')).toBe('/studio/factory/forgejo/app');
    expect(legacyTarget('/factory/plane/configure')).toBe('/studio/factory/plane/configure');
    expect(legacyTarget('/factory', { tab: 'x y' })).toBe('/studio/factory?tab=x+y');

    const { pages, navigated, at } = apply();
    const legacy = pages.filter((p) => p.spec.id.startsWith('factory-legacy'));
    expect(legacy.map((p) => p.spec.path).sort()).toEqual([
      '/factory',
      '/factory/:app',
      '/factory/:app/app',
      '/factory/:app/configure',
      '/factory/runs',
    ]);
    for (const [path, want] of [
      ['/factory', '/studio/factory'],
      ['/factory/zulip', '/studio/factory/zulip'],
      ['/factory/zulip/app', '/studio/factory/zulip/app'],
      ['/factory/zulip/configure', '/studio/factory/zulip/configure'],
    ] as const) {
      at(path);
      const page = legacy.find((p) =>
        new RegExp(`^${p.spec.path.replace(/:app/, '[^/]+')}$`).test(path),
      );
      const Component = page?.component as ComponentType<{ params: Record<string, string> }>;
      render(<Component params={{}} />);
      // A replace, so Back does not bounce through the redirect again.
      expect(navigated.at(-1)).toEqual({ to: want, replace: true });
      cleanup();
    }
    // A redirect still mounted after the route moved on does nothing.
    expect(legacyTarget('/studio/factory/forgejo')).toBeUndefined();
    expect(legacyTarget('/factoryx')).toBeUndefined();
    const before = navigated.length;
    at('/studio/factory/forgejo');
    const Stale = legacy[0]?.component as ComponentType<{ params: Record<string, string> }>;
    render(<Stale params={{}} />);
    expect(navigated).toHaveLength(before);
  });
});

describe('in-context links', () => {
  const ok = (id: string) => IDS.map((i) => info(i, i === id ? 'ok' : 'unconfigured'));

  it('start_thread_from_plane_issue', async () => {
    const n = nav();
    const { api } = fakeDesktop({
      apps: ok('plane'),
      queries: {
        'plane.stats': {},
        'plane.issues': [{ id: 'MKT-42', title: 'Fix the login redirect', priority: 'high' }],
      },
    });
    render(<PanelsPage desktop={api} app="plane" navigate={n.navigate} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Start thread from MKT-42' }));
    expect(n.to).toHaveLength(1);
    const { path, q } = draftOf(n.to[0] ?? '');
    expect(path).toBe('/studio');
    expect(q.from).toBe('plane');
    expect(q.title).toBe('MKT-42: Fix the login redirect');
    expect(q.prompt).toContain('MKT-42');
    expect(q.prompt).toContain('Fix the login redirect');
    // The branch is named after the issue key.
    expect(q.branch).toBe('mkt-42-fix-the-login-redirect');
    expect(branchFor('ABC-7', '')).toBe('abc-7');
    expect(branchFor('ABC-7', 'Ünïcode & "quotes" — and a very long title that goes on')).toMatch(
      /^abc-7-[a-z0-9-]{1,40}$/,
    );
  });

  it('fix_error_from_glitchtip', async () => {
    const n = nav();
    const d = fakeDesktop({
      apps: ok('glitchtip'),
      queries: {
        'glitchtip.organizations': [{ slug: 'demo', name: 'Demo' }],
        'glitchtip.issues': [
          {
            id: '311',
            title: "TypeError: Cannot read properties of undefined (reading 'id')",
            level: 'error',
            count: '12',
            permalink: 'https://glitchtip.example.test/demo/issues/311',
          },
        ],
        'glitchtip.latestEvent': {
          release: 'web@1.4.2',
          culprit: 'checkout/cart.ts in total',
          stack: 'TypeError: Cannot read properties of undefined\n  at total (checkout/cart.ts:41)',
        },
      },
    });
    render(<PanelsPage desktop={d.api} app="glitchtip" navigate={n.navigate} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Fix this error: 311' }));
    await waitFor(() => expect(n.to).toHaveLength(1));
    expect(d.queries.at(-1)).toMatchObject({
      app: 'glitchtip',
      op: 'latestEvent',
      params: { issueId: '311' },
    });
    const { path, q } = draftOf(n.to[0] ?? '');
    expect(path).toBe('/studio');
    expect(q.from).toBe('glitchtip');
    expect(q.prompt).toContain('at total (checkout/cart.ts:41)');
    expect(q.prompt).toContain('web@1.4.2');
    expect(q.link).toBe('https://glitchtip.example.test/demo/issues/311');
  });

  it('a GlitchTip event that cannot be read still starts the draft', async () => {
    const n = nav();
    const { api } = fakeDesktop({
      apps: ok('glitchtip'),
      queries: {
        'glitchtip.organizations': [{ slug: 'demo' }],
        'glitchtip.issues': [{ id: '9', title: 'Boom' }],
      },
    });
    render(<PanelsPage desktop={api} app="glitchtip" navigate={n.navigate} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Fix this error: 9' }));
    await waitFor(() => expect(n.to).toHaveLength(1));
    const { q } = draftOf(n.to[0] ?? '');
    expect(q.prompt).toContain('Boom');
    expect(q.prompt).toContain('stack trace could not be read');
  });

  it('a draft link is http(s) only', () => {
    const { q } = draftOf(
      threadDraftHref({ from: 'glitchtip', title: 't', prompt: 'p', link: 'javascript:alert(1)' }),
    );
    expect(q.link).toBeUndefined();
  });
});
