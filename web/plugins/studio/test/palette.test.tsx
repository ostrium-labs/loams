// Studio's palette rows and keyboard layer (SD1 Task 24; §53 §5.10): the
// `palette.command` entry publishes threads, projects, git actions and Studio's
// commands from a `StudioPaletteSource`; keybindings run the commands the mounted
// view handles; Settings › Studio edits them.

import { create } from '@bufbuild/protobuf';
import { createRouterTransport } from '@connectrpc/connect';
import type { AgentdService, Location } from '@loams/console-host';
import { GitService, ProjectService, RepoSchema } from '@loams/proto/agentd';
import type { PaletteCommand } from '@loams/slots';
import { buildThreadActionItems, KEYBINDINGS_KEY, KeybindingStore } from '@loams/studio-ui';
import { rankCommands } from '@loams/ui';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { type StudioAgentd, studioAgentd } from '../src/agentd.js';
import { BranchesSource } from '../src/palette/BranchesSource.js';
import { createStudioKeys, type StudioKeys } from '../src/palette/commands.js';
import { StudioPalette } from '../src/palette/StudioPalette.js';
import { pendingPaletteSource, type StudioPaletteSource } from '../src/palette/source.js';
import { SETTINGS_KEY } from '../src/settings/Settings.js';
import { desktopFake, wrap as wrapServerState } from './fakes.js';
import { StudioInShell, TestPanel } from './shell.js';

const LINUX = 'Linux x86_64';

function wrap(children: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

function viewport(wide: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: query.includes('min-width') ? wide : !wide,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
}

function agentdReady(): StudioAgentd {
  return desktopFake({ folder: null }).agentd;
}

function fakeRouter(location: Location) {
  return {
    current: () => location,
    subscribe: () => () => {},
    navigate: vi.fn(),
  };
}

function keysFor(storage = window.localStorage): StudioKeys {
  return createStudioKeys({
    edition: 'desktop',
    platform: LINUX,
    keybindings: new KeybindingStore(storage),
  });
}

/** Mounts the entry as the shell would, and returns the latest list it published. */
function mountPalette(o: {
  source: StudioPaletteSource;
  location: Location;
  query?: string;
  keys?: StudioKeys;
}) {
  const latest: { commands: readonly PaletteCommand[] } = { commands: [] };
  const publish = vi.fn((c: readonly PaletteCommand[]) => {
    latest.commands = c;
  });
  const router = fakeRouter(o.location);
  const keys = o.keys ?? keysFor();
  const agentd = agentdReady();
  const view = render(
    wrap(
      <StudioPalette
        query={o.query ?? ''}
        publish={publish}
        agentd={agentd}
        source={o.source}
        router={router}
        keys={keys}
      />,
    ),
  );
  return { latest, router, keys, view };
}

const threadAt: Location = {
  path: '/studio/t/c_1',
  pageId: 'studio-thread',
  params: { threadId: 'c_1' },
};

beforeEach(() => {
  localStorage.clear();
  viewport(true);
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe("Studio's palette rows", () => {
  it('palette_runs_git_action', async () => {
    const runGitAction = vi.fn(async () => {});
    const gitActions = vi.fn(async () => [
      { action: 'commit' as const, label: 'Commit changes', disabledReason: 'Nothing to commit' },
      { action: 'push' as const, label: 'Push branch' },
    ]);
    const source: StudioPaletteSource = {
      ...pendingPaletteSource(),
      threads: async () => [
        {
          id: 'c_1',
          title: 'Fix login',
          projectId: '/src/site',
          branch: 'fix-login',
          updatedAt: 2,
        },
      ],
      gitActions,
      runGitAction,
    };
    const { latest } = mountPalette({ source, location: threadAt });
    await waitFor(() => expect(latest.commands.some((c) => c.title === 'Push branch')).toBe(true));

    const commit = latest.commands.find((c) => c.title === 'Commit changes');
    expect(commit).toMatchObject({ group: 'Git', disabled: true, detail: 'Nothing to commit' });
    const push = latest.commands.find((c) => c.title === 'Push branch');
    expect(push?.group).toBe('Git');
    await act(async () => {
      await push?.run();
    });
    expect(runGitAction).toHaveBeenCalledWith('c_1', 'push');
    // The thread's actions are read again after one runs.
    await waitFor(() => expect(gitActions).toHaveBeenCalledTimes(2));
  });

  it('offers no git rows outside a thread, and lists threads, newest first', async () => {
    const gitActions = vi.fn(async () => []);
    const source: StudioPaletteSource = {
      ...pendingPaletteSource(),
      threads: async () => [
        { id: 'c_old', title: 'Old work', projectId: 'p', updatedAt: 1 },
        { id: 'c_new', title: 'New work', projectId: 'p', updatedAt: 5 },
        { id: 'c_gone', title: 'Archived', projectId: 'p', updatedAt: 9, archived: true },
      ],
      gitActions,
    };
    const { latest, router } = mountPalette({
      source,
      location: { path: '/studio', pageId: 'studio', params: {} },
    });
    await waitFor(() =>
      expect(latest.commands.filter((c) => c.group === 'Threads').map((c) => c.title)).toEqual([
        'New work',
        'Old work',
      ]),
    );
    expect(gitActions).not.toHaveBeenCalled();
    latest.commands.find((c) => c.title === 'Old work')?.run();
    expect(router.navigate).toHaveBeenCalledWith('/studio/t/c_old');
  });

  it('quick-opens files of the open thread for a query', async () => {
    const searchFiles = vi.fn(async () => ['src/app/login.ts']);
    const { latest, router } = mountPalette({
      source: { ...pendingPaletteSource(), searchFiles },
      location: threadAt,
      query: 'login',
    });
    await waitFor(() => expect(latest.commands.some((c) => c.group === 'Files')).toBe(true));
    expect(searchFiles).toHaveBeenCalledWith('c_1', 'login');
    const file = latest.commands.find((c) => c.group === 'Files');
    expect(file).toMatchObject({ title: 'login.ts', detail: 'src/app' });
    file?.run();
    expect(router.navigate).toHaveBeenCalledWith(
      '/studio/t/c_1?panel=files&path=src%2Fapp%2Flogin.ts',
    );
  });

  it('switches the default harness for new threads', async () => {
    const { latest } = mountPalette({
      source: pendingPaletteSource(),
      location: { path: '/studio', pageId: 'studio', params: {} },
    });
    await waitFor(() =>
      expect(latest.commands.some((c) => c.title === 'Use Codex for new threads')).toBe(true),
    );
    expect(latest.commands.find((c) => c.title === 'Use Claude Code for new threads')?.detail).toBe(
      'Current',
    );
    latest.commands.find((c) => c.title === 'Use Codex for new threads')?.run();
    expect(JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}').harness).toBe('codex');
  });

  it('lists the commands the mounted view handles, with their shortcuts', async () => {
    const keys = keysFor();
    keys.commands.register('diff.toggle', () => {});
    const { latest } = mountPalette({ source: pendingPaletteSource(), location: threadAt, keys });
    await waitFor(() =>
      expect(latest.commands.some((c) => c.title === 'Show the diff')).toBe(true),
    );
    expect(latest.commands.find((c) => c.title === 'Show the diff')).toMatchObject({
      group: 'Actions',
      shortcut: ['Ctrl', 'D'],
    });
    expect(latest.commands.some((c) => c.title === 'Show the preview')).toBe(false);
  });
});

describe("Studio's keybindings in the view", () => {
  function showThread(keys: StudioKeys) {
    const navigate = vi.fn();
    render(
      wrap(
        <StudioInShell
          agentd={agentdReady()}
          route={{ kind: 'thread', threadId: 'c_1', panel: 'diff' }}
          navigate={navigate}
          keys={keys}
        />,
      ),
    );
    return { navigate };
  }

  it('Mod+Shift+J shows the preview panel of the open thread', async () => {
    const { navigate } = showThread(keysFor());
    await screen.findByRole('tab', { name: 'Diff' });
    fireEvent.keyDown(document.body, { key: 'J', code: 'KeyJ', ctrlKey: true, shiftKey: true });
    expect(navigate).toHaveBeenCalledWith('/studio/t/c_1?panel=preview');
  });

  it('Mod+D on the shown diff hides the panel, and not while typing in a terminal', async () => {
    showThread(keysFor());
    await screen.findByRole('tab', { name: 'Diff' });
    expect(screen.getByRole('complementary', { name: 'Inspector' })).toBeTruthy();
    const terminal = document.createElement('div');
    terminal.setAttribute('data-terminal', '');
    terminal.tabIndex = 0;
    document.body.append(terminal);
    fireEvent.keyDown(terminal, { key: 'd', code: 'KeyD', ctrlKey: true });
    expect(screen.queryByRole('complementary', { name: 'Inspector' })).toBeTruthy();
    fireEvent.keyDown(document.body, { key: 'd', code: 'KeyD', ctrlKey: true });
    await waitFor(() =>
      expect(screen.queryByRole('complementary', { name: 'Inspector' })).toBeNull(),
    );
    terminal.remove();
  });

  it('follows a binding changed in Settings › Studio, and keeps it on this device', async () => {
    const navigate = vi.fn();
    const keys = keysFor();
    const inspector = new TestPanel();
    render(
      wrap(
        <StudioInShell
          agentd={agentdReady()}
          route={{ kind: 'settings' }}
          navigate={navigate}
          keys={keys}
          inspector={inspector}
        />,
      ),
    );
    const change = await screen.findByRole('button', {
      name: 'Shortcut for Panel: Show or Hide: mod+alt+b. Change',
    });
    fireEvent.click(change);
    fireEvent.keyDown(change, { key: 'u', code: 'KeyU', ctrlKey: true, shiftKey: true });
    expect(JSON.parse(localStorage.getItem(KEYBINDINGS_KEY) ?? '[]')).toEqual([
      { key: 'mod+shift+u', command: 'rightPanel.toggle' },
    ]);
    expect(screen.getAllByText('Custom').length).toBeGreaterThan(0);
    fireEvent.keyDown(document.body, { key: 'u', code: 'KeyU', ctrlKey: true, shiftKey: true });
    expect(inspector.state().open).toBe(true);

    // A chord the shell owns (D1081) is flagged.
    const again = screen.getByRole('button', {
      name: 'Shortcut for Panel: Show or Hide: mod+shift+u. Change',
    });
    fireEvent.click(again);
    fireEvent.keyDown(again, { key: 'b', code: 'KeyB', ctrlKey: true });
    expect(await screen.findByText('Conflicts with Shell: Toggle Sidebar')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Reset Panel: Show or Hide' }));
    expect(JSON.parse(localStorage.getItem(KEYBINDINGS_KEY) ?? '[]')).toEqual([]);
  });
});

describe("the palette's branches and thread text (CF1 Task 11)", () => {
  it('matches a thread by its last message (thread text, W13)', () => {
    const items = buildThreadActionItems({
      threads: [
        {
          id: 't1',
          title: 'Untitled',
          projectId: '',
          updatedAt: 1,
          preview: 'migrate the orders table to v2',
        },
        { id: 't2', title: 'Orders page copy', projectId: '', updatedAt: 2 },
      ],
      projectTitleById: new Map(),
      open: () => {},
    });
    const ranked = rankCommands(
      items.map((i) => ({ ...i, text: i.title })),
      'migrate the orders',
    );
    expect(ranked[0]?.id).toBe('thread:t1');
  });

  it("lists each project's branches once typed, and a branch opens its project (never a checkout)", async () => {
    const calls: string[] = [];
    const transport = createRouterTransport(({ service }) => {
      service(ProjectService, {
        listRepos: () => ({
          repos: [
            create(RepoSchema, { path: '~/src/loams', name: 'loams', defaultBranch: 'dev' }),
            create(RepoSchema, { path: '~/src/site', name: 'site', defaultBranch: 'main' }),
          ],
        }),
      });
      service(GitService, {
        listBranches: ({ repoPath }) => {
          calls.push(`ListBranches:${repoPath}`);
          return { branches: repoPath === '~/src/loams' ? ['dev', 'cf1d-t11'] : ['main'] };
        },
        switchRef: () => {
          calls.push('SwitchRef');
          return {};
        },
      });
    });
    const ready = { state: 'ready' } as const;
    const host = { id: 'h', name: 'Host' };
    const service: AgentdService = {
      kind: 'web',
      reach: () => ready,
      subscribe: () => () => {},
      transport: () => transport,
      host: () => host,
    };
    const agentd = studioAgentd(service);
    const published: PaletteCommand[][] = [];
    const navigate = vi.fn();
    const source = (query: string) =>
      wrapServerState(
        <BranchesSource
          query={query}
          publish={(c) => published.push([...c])}
          agentd={agentd}
          navigate={navigate}
        />,
      );
    const { rerender } = render(source(''));
    // An empty query lists no branches and reads none (T11-3).
    await waitFor(() => expect(published.at(-1)).toEqual([]));
    expect(calls).toEqual([]);
    rerender(source('d'));
    await waitFor(() =>
      expect(
        published
          .at(-1)
          ?.map((c) => `${c.title} · ${c.detail}`)
          .sort(),
      ).toEqual(['cf1d-t11 · loams', 'dev · loams', 'main · site']),
    );
    expect(new Set(published.at(-1)?.map((c) => c.group))).toEqual(new Set(['Branches']));
    published
      .at(-1)
      ?.find((c) => c.title === 'cf1d-t11')
      ?.run();
    expect(navigate).toHaveBeenCalledWith(`/studio/p/${encodeURIComponent('~/src/loams')}`);
    expect(calls).not.toContain('SwitchRef');
  });
});
