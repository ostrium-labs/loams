// Studio's side of the factory (SD1 Task 27; §53 §5.9, D916, D922): the sidebar's
// Factory group, the thread draft a factory app hands over, the thread toolbar's
// Forgejo PR, and links only on the web until SF1.

import type { ModeService } from '@loams/console-host';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { desktopFactory, type StudioFactory } from '../src/factory.js';
import { createStudioKeys } from '../src/palette/commands.js';
import { StudioThreads } from '../src/regions.js';
import { routeOf, studioHref } from '../src/routes.js';
import { FactoryGroup } from '../src/sidebar/Factory.js';
import { BranchPr } from '../src/thread/FactoryLinks.js';
import { desktopFake as desktopAgentd, webFake } from './fakes.js';
import { StudioInShell } from './shell.js';

function wrap(children: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

const APPS = [
  ['forgejo', 'Forgejo', 'ok'],
  ['zulip', 'Zulip', 'auth_failed'],
  ['plane', 'Plane (ItsAPlan)', 'ok'],
  ['glitchtip', 'GlitchTip', 'unconfigured'],
  ['openpanel', 'OpenPanel', 'unconfigured'],
  ['matomo', 'Matomo', 'unconfigured'],
  ['langfuse', 'Langfuse', 'locked'],
  ['signoz', 'SigNoz', 'ok'],
] as const;

/** The slice of `desktop.factory` Studio reads. */
function desktopFake(pulls: unknown[] = []) {
  const queries: unknown[] = [];
  const factory = {
    list: async () =>
      APPS.map(([id, label, health]) => ({
        id,
        label,
        health,
        url: health === 'ok' ? `https://${id}.example.test` : undefined,
        hasPanels: id !== 'signoz',
        ...(id === 'signoz' ? { preview: 'Preview: SigNoz on Loams House' } : {}),
        credentialFields: [],
        persistent: true,
      })),
    query: async (q: unknown) => {
      queries.push(q);
      return { ok: true, value: pulls };
    },
  };
  return { factory: desktopFactory({ factory } as never), queries };
}

beforeEach(() => {
  localStorage.clear();
  window.matchMedia = ((query: string) => ({
    matches: query.includes('min-width'),
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('the Factory group', () => {
  it('re-reads the apps after a navigation, so a configure elsewhere shows', async () => {
    // The group stays mounted while the factory's own pages configure an app.
    let health = 'unconfigured';
    const factory = desktopFactory({
      factory: {
        list: async () => [
          { id: 'forgejo', label: 'Forgejo', health, hasPanels: true, credentialFields: [] },
        ],
      },
    } as never);
    render(wrap(<FactoryGroup factory={factory} />));
    const group = await screen.findByRole('navigation', { name: 'Factory' });
    expect(await within(group).findByText('0 of 1 connected')).toBeTruthy();
    health = 'ok';
    await act(async () => {
      window.dispatchEvent(new HashChangeEvent('hashchange'));
    });
    expect(await within(group).findByText('1 of 1 connected')).toBeTruthy();
  });

  it('lists the eight apps with their configured state', async () => {
    render(wrap(<FactoryGroup factory={desktopFake().factory} />));
    const group = await screen.findByRole('navigation', { name: 'Factory' });
    // Collapsed by default (owner feedback 2026-10-17): one line with the count.
    const toggle = within(group).getByRole('button', { name: /^Factory/ });
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    expect(await within(group).findByText('3 of 8 connected')).toBeTruthy();
    // A failing app is announced, not only drawn (the dot is decorative).
    expect(within(group).getByText('Some apps need attention.')).toBeTruthy();
    expect(within(group).queryByRole('list')).toBeNull();
    fireEvent.click(toggle);
    expect(localStorage.getItem('loams.studio.factory.open')).toBe('1');
    const links = within(await within(group).findByRole('list')).getAllByRole('link');
    // Matomo stays (Q813).
    const text = (a: HTMLElement) =>
      [...a.querySelectorAll(':scope > span')].map((s) => s.textContent).join(' ');
    expect(links.map(text)).toEqual([
      'Forgejo Connected',
      'Zulip Auth failed',
      'Plane (ItsAPlan) Connected',
      'GlitchTip Not configured',
      'OpenPanel Not configured',
      'Matomo Not configured',
      'Langfuse Locked',
      'SigNoz Preview Connected',
    ]);
    const href = (name: RegExp) => within(group).getByRole('link', { name }).getAttribute('href');
    // Configured: the panels, or the full UI when there are none; otherwise configure.
    expect(href(/^Forgejo/)).toBe('#/studio/factory/forgejo');
    expect(href(/^SigNoz/)).toBe('#/studio/factory/signoz/app');
    expect(within(group).getByTitle('Preview: SigNoz on Loams House')).toBeTruthy();
    expect(href(/^GlitchTip/)).toBe('#/studio/factory/glitchtip/configure');
    expect(href(/^Langfuse/)).toBe('#/studio/factory/langfuse/configure');
    expect(within(group).getByRole('link', { name: 'All factory apps' }).getAttribute('href')).toBe(
      '#/studio/factory',
    );
  });

  it('shows a failure to list the apps', async () => {
    const factory: StudioFactory = {
      kind: 'pages',
      list: async () => {
        throw new Error('ipc closed');
      },
    };
    render(wrap(<FactoryGroup factory={factory} />));
    expect(await screen.findByText(/Could not list the factory apps: ipc closed/)).toBeTruthy();
  });

  it('is in the desktop sidebar', async () => {
    const { agentd } = desktopAgentd({ folder: null });
    render(
      wrap(
        <StudioInShell
          agentd={agentd}
          factory={desktopFake().factory}
          route={{ kind: 'home' }}
          navigate={vi.fn()}
        />,
      ),
    );
    expect(await screen.findByRole('navigation', { name: 'Factory' })).toBeTruthy();
  });

  it('shows its header while the daemon is down (owner ruling, 2026-10-18)', async () => {
    const { agentd } = desktopAgentd({ running: false });
    const router = {
      current: () => ({ path: '/studio', params: {} }),
      subscribe: () => () => {},
      navigate: vi.fn(),
      page: () => () => {},
    };
    render(
      wrap(
        <StudioThreads
          agentd={agentd}
          factory={desktopFake().factory}
          router={router}
          keys={createStudioKeys()}
        />,
      ),
    );
    const group = await screen.findByRole('navigation', { name: 'Factory' });
    expect(await within(group).findByText('3 of 8 connected')).toBeTruthy();
    // The threads still need the daemon.
    expect(screen.queryByRole('navigation', { name: 'Projects' })).toBeNull();
  });

  it('shows in Studio mode only; the threads show in both (design 59, W10)', async () => {
    const { agentd } = desktopAgentd({ folder: null });
    const listeners = new Set<() => void>();
    let current: 'console' | 'studio' = 'console';
    const mode: ModeService = {
      get current() {
        return current;
      },
      available: ['console', 'studio'],
      set: () => {},
      onChange: (cb) => {
        const listener = () => cb(current);
        listeners.add(listener);
        return () => {
          listeners.delete(listener);
        };
      },
      modesOf: () => ['studio'],
    };
    const router = {
      current: () => ({ path: '/', params: {} }),
      subscribe: () => () => {},
      navigate: vi.fn(),
      page: () => () => {},
    };
    render(
      wrap(
        <StudioThreads
          agentd={agentd}
          factory={desktopFake().factory}
          router={router}
          keys={createStudioKeys()}
          mode={mode}
        />,
      ),
    );
    expect(await screen.findByRole('navigation', { name: 'Projects' })).toBeTruthy();
    expect(screen.queryByRole('navigation', { name: 'Factory' })).toBeNull();
    current = 'studio';
    act(() => {
      for (const l of listeners) l();
    });
    expect(await screen.findByRole('navigation', { name: 'Factory' })).toBeTruthy();
  });

  it('web_factory_links_only_without_sf1', async () => {
    // Without SF1 the browser has no factory source: no Factory group at all.
    render(
      wrap(<StudioInShell agentd={webFake().agentd} route={{ kind: 'home' }} navigate={vi.fn()} />),
    );
    await screen.findByText('Studio needs a Loams server with agentd');
    expect(screen.queryByRole('navigation', { name: 'Factory' })).toBeNull();
    cleanup();
    // A links-only source (D922): each app opens in a new tab; no panels or embeds.
    const links: StudioFactory = {
      kind: 'links',
      list: async () => [
        { id: 'forgejo', label: 'Forgejo', state: 'ok', url: 'https://git.example.test' },
        { id: 'plane', label: 'Plane', state: 'ok', url: 'javascript:alert(1)' },
        { id: 'zulip', label: 'Zulip', state: 'unconfigured' },
      ],
    };
    localStorage.setItem('loams.studio.factory.open', '1');
    render(wrap(<FactoryGroup factory={links} />));
    const group = await screen.findByRole('navigation', { name: 'Factory' });
    const anchors = await within(group).findAllByRole('link');
    expect(anchors).toHaveLength(1);
    expect(anchors[0]?.getAttribute('href')).toBe('https://git.example.test');
    expect(anchors[0]?.getAttribute('target')).toBe('_blank');
    expect(anchors[0]?.getAttribute('rel')).toContain('noopener');
    expect(group.innerHTML).not.toContain('#/studio/factory');
    // Unreachable or unsafe apps are listed, not linked.
    expect(within(group).getByText('Plane')).toBeTruthy();
    expect(within(group).getByText('Zulip')).toBeTruthy();
  });
});

describe('the thread draft', () => {
  it('a factory app hands a draft to Studio`s home', async () => {
    const route = routeOf('home', {
      from: 'plane',
      title: 'MKT-42: Fix the login redirect',
      prompt: 'Work on MKT-42.',
      branch: 'mkt-42-fix-the-login-redirect',
      link: 'https://plane.example.test/i/MKT-42',
    });
    expect(route).toEqual({
      kind: 'home',
      draft: {
        from: 'plane',
        title: 'MKT-42: Fix the login redirect',
        prompt: 'Work on MKT-42.',
        branch: 'mkt-42-fix-the-login-redirect',
        link: 'https://plane.example.test/i/MKT-42',
      },
    });
    expect(routeOf('home', studioQuery(studioHref(route)))).toEqual(route);
    const { agentd } = desktopAgentd({ folder: null });
    render(wrap(<StudioInShell agentd={agentd} route={route} navigate={vi.fn()} />));
    const card = await screen.findByRole('region', { name: 'New thread from Plane' });
    expect(within(card).getByText('MKT-42: Fix the login redirect')).toBeTruthy();
    expect(within(card).getByText('mkt-42-fix-the-login-redirect')).toBeTruthy();
    expect(within(card).getByText('Work on MKT-42.')).toBeTruthy();
    expect(within(card).getByRole('link', { name: /plane\.example\.test/ })).toBeTruthy();
  });

  it('a draft with an unsafe link or branch drops them', () => {
    const route = routeOf('home', {
      from: 'glitchtip',
      title: 't',
      prompt: 'p',
      link: 'javascript:alert(1)',
      branch: '--upload-pack=x',
    });
    expect(route).toEqual({ kind: 'home', draft: { from: 'glitchtip', title: 't', prompt: 'p' } });
    expect(routeOf('home', { from: 'evil', prompt: 'p' })).toEqual({ kind: 'home' });
  });
});

describe('the thread toolbar', () => {
  it('thread_toolbar_shows_forgejo_pr', async () => {
    const { factory, queries } = desktopFake([
      {
        id: '11',
        title: 'Other work',
        head: 'feat/other',
        url: 'https://git.example.test/a/b/pulls/11',
      },
      {
        id: '12',
        title: 'Fix login',
        head: 'mkt-42-fix',
        url: 'https://git.example.test/a/b/pulls/12',
      },
    ]);
    render(wrap(<BranchPr factory={factory} repo="acme/site" branch="mkt-42-fix" />));
    const link = await screen.findByRole('link', { name: /PR #12/ });
    expect(link.getAttribute('href')).toBe('https://git.example.test/a/b/pulls/12');
    expect(link.getAttribute('target')).toBe('_blank');
    expect(queries[0]).toEqual({
      app: 'forgejo',
      op: 'issues',
      params: { type: 'pulls', owner: 'acme', repo: 'site', limit: 50 },
    });
    cleanup();
    // No PR for the branch: nothing in the toolbar.
    const none = desktopFake([]);
    const { container } = render(
      wrap(<BranchPr factory={none.factory} repo="acme/site" branch="mkt-42-fix" />),
    );
    await vi.waitFor(() => expect(none.queries).toHaveLength(1));
    expect(container.textContent).toBe('');
  });
});

/** The query of a hash route, as the router hands it to `routeOf`. */
function studioQuery(href: string): Record<string, string> {
  return Object.fromEntries(new URLSearchParams(href.split('?')[1] ?? ''));
}
