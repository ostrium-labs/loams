// Studio in the shell's regions (design 59 D1078, D1079) and Review Focus 2: many threads and
// very long names in the sidebar.

import type { PanelService, RouterService } from '@loams/console-host';
import { SlotRegistry } from '@loams/slots';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { GitActionBus } from '../src/git/bus.js';
import { createStudioKeys } from '../src/palette/commands.js';
import { registerStudioRegions, StudioThreads, THREAD_ROUTES } from '../src/regions.js';
import { THREADS_SHOWN } from '../src/sidebar/Sidebar.js';
import { StudioView } from '../src/view.js';
import { webFake, wrap } from './fakes.js';
import { at, sidebarDaemon } from './sidebar-fakes.js';

function router(path = '/data'): RouterService {
  return {
    current: () => ({ path, params: {} }),
    subscribe: () => () => {},
    navigate: vi.fn(),
    page: () => () => {},
  };
}
function panel(): PanelService & { calls: unknown[] } {
  const calls: unknown[] = [];
  const state = { open: false, tab: undefined, props: {}, size: 280 };
  return {
    calls,
    state: () => state,
    open: (o) => void calls.push(['open', o]),
    close: () => void calls.push(['close']),
    toggle: (t) => void calls.push(['toggle', t]),
    resize: () => {},
    subscribe: () => () => {},
  };
}
// A selector, not getAllByRole: role queries over hundreds of links take seconds on a busy CI
// runner (T9-8).
const threadLinks = (list: HTMLElement) => [
  ...list.querySelectorAll<HTMLAnchorElement>('a[href^="#/studio/t/"]'),
];

beforeEach(() => {
  localStorage.clear();
  window.matchMedia = ((q: string) => ({
    matches: q.includes('min-width'),
    media: q,
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof window.matchMedia;
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('the sidebar thread list (Review Focus 2)', () => {
  it('long names truncate and big projects collapse', async () => {
    const long = 'x'.repeat(300);
    const chats = Array.from({ length: 400 }, (_, i) => ({
      id: `t-${i}`,
      title: i === 0 ? `${long} first` : `Thread ${i} ${i % 7 === 0 ? long : ''}`,
      cwd: i < 250 ? '~/src/loams' : i < 380 ? '~/src/acme-site' : '/tmp/elsewhere',
      createdAtMs: at(i),
      lastMessageAtMs: at(1000 - i),
    }));
    const daemon = sidebarDaemon({ chats });
    render(
      wrap(<StudioThreads agentd={daemon.agentd} router={router()} keys={createStudioKeys()} />),
    );
    const list = await screen.findByRole('list', { name: 'loams threads' });
    await waitFor(() => expect(threadLinks(list)).toHaveLength(THREADS_SHOWN));
    const first = threadLinks(list)[0] as HTMLElement;
    expect(first.getAttribute('title')).toContain(long);
    expect(first.className).toContain('h-7');
    expect(first.className).toContain('min-w-0');
    expect(first.querySelector('.truncate')).toBeTruthy();
    fireEvent.click(within(list).getByRole('button', { name: `Show ${250 - THREADS_SHOWN} more` }));
    await waitFor(() => expect(threadLinks(list)).toHaveLength(250));
    expect(within(list).queryByRole('button', { name: /^Show \d+ more$/ })).toBeNull();
  }, 20_000); // 400 threads, 250 of them rendered; a busy CI runner took over 5 s (T9-8)

  it('keeps the open thread in view when it is past the cap', async () => {
    const chats = Array.from({ length: 30 }, (_, i) => ({
      id: `t-${i}`,
      title: `Thread ${i}`,
      cwd: '~/src/loams',
      createdAtMs: at(i),
      lastMessageAtMs: at(1000 - i),
    }));
    const daemon = sidebarDaemon({ chats });
    const at25: RouterService = {
      ...router('/studio/t/t-25'),
      current: () => ({
        path: '/studio/t/t-25',
        pageId: 'studio-thread',
        params: { threadId: 't-25' },
      }),
    };
    render(wrap(<StudioThreads agentd={daemon.agentd} router={at25} keys={createStudioKeys()} />));
    const list = await screen.findByRole('list', { name: 'loams threads' });
    await waitFor(() => expect(threadLinks(list)).toHaveLength(THREADS_SHOWN + 1));
    expect(within(list).getByRole('link', { current: 'page' }).getAttribute('href')).toBe(
      '#/studio/t/t-25',
    );
    expect(within(list).getByRole('button', { name: `Show ${30 - THREADS_SHOWN - 1} more` }));
  });

  it('shows nothing without a reachable agentd (Q934)', () => {
    const { agentd } = webFake(); // the web edition with no agentd host
    const { container } = render(
      wrap(<StudioThreads agentd={agentd} router={router()} keys={createStudioKeys()} />),
    );
    expect(container.textContent).toBe('');
  });

  it('archived threads are not in the sidebar; they live in Settings (D1079)', async () => {
    const daemon = sidebarDaemon({
      chats: [
        { id: 'old', title: 'Old idea', cwd: '~/src/loams', archived: true, createdAtMs: at(0) },
      ],
    });
    render(
      wrap(<StudioThreads agentd={daemon.agentd} router={router()} keys={createStudioKeys()} />),
    );
    await screen.findByRole('navigation', { name: 'Projects' });
    expect(screen.queryByRole('button', { name: /Show archived/ })).toBeNull();
    expect(screen.queryByText('Old idea')).toBeNull();
  });
});

describe('Studio registers its regions', () => {
  it('home in Studio mode, threads, four inspector tabs and the terminal on thread routes', () => {
    const slots = new SlotRegistry();
    const daemon = sidebarDaemon();
    registerStudioRegions(slots, {
      agentd: daemon.agentd,
      router: router(),
      keys: createStudioKeys(),
      inspector: panel(),
      drawer: panel(),
      gitBus: new GitActionBus(),
      openExternal: async () => {},
    });
    expect(slots.entries('shell.home')[0]?.meta?.mode).toBe('studio');
    expect(slots.entries('shell.sidebar.threads')).toHaveLength(1);
    expect(slots.entries('shell.inspector.tab').map((e) => [e.meta?.id, e.meta?.routes])).toEqual([
      ['studio.diff', THREAD_ROUTES],
      ['studio.files', THREAD_ROUTES],
      ['studio.preview', THREAD_ROUTES],
      ['studio.plan', THREAD_ROUTES],
    ]);
    expect(slots.entries('shell.drawer.tab').map((e) => e.meta?.id)).toEqual(['studio.terminal']);
    expect(slots.entries('shell.topbar.left')[0]?.meta?.mode).toBe('studio');
    expect(slots.entries('shell.topbar.right')[0]?.meta?.mode).toBe('studio');
  });

  it('a ?panel= link opens that inspector tab, and ?panel=terminal the drawer', async () => {
    const daemon = sidebarDaemon();
    const inspector = panel();
    const drawer = panel();
    const view = render(
      wrap(
        <StudioView
          agentd={daemon.agentd}
          route={{ kind: 'thread', threadId: 't-login', panel: 'files' }}
          navigate={vi.fn()}
          inspector={inspector}
          drawer={drawer}
        />,
      ),
    );
    await waitFor(() =>
      expect(inspector.calls).toContainEqual(['open', { tab: 'studio.files', props: {} }]),
    );
    view.rerender(
      wrap(
        <StudioView
          agentd={daemon.agentd}
          route={{ kind: 'thread', threadId: 't-login', panel: 'terminal' }}
          navigate={vi.fn()}
          inspector={inspector}
          drawer={drawer}
        />,
      ),
    );
    await waitFor(() => expect(drawer.calls).toContainEqual(['open', { tab: 'studio.terminal' }]));
  });
});
