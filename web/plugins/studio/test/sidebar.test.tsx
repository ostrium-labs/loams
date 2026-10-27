// Studio's thread list and sidebar (SD1 Task 17; §53 §5.3): threads grouped under their
// projects, status badges that follow the daemon's sessions, the pinned order, delete
// with the worktree prompt, search, the keyboard, and the palette's threads (T24-6). Since
// design 59 the list is the shell sidebar's (`StudioThreads`), shown beside the view, and
// archived threads are in Studio's settings.

import { SHELL_COMMAND_EVENT } from '@loams/console-host';
import { SessionStatus } from '@loams/proto/agentd';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { createStudioKeys, listenShellCommands, type StudioKeys } from '../src/palette/commands.js';
import type { StudioRoute } from '../src/routes.js';
import { agentdPaletteSource } from '../src/sidebar/paletteSource.js';
import { wrap } from './fakes.js';
import { StudioInShell } from './shell.js';
import { at, sidebarDaemon } from './sidebar-fakes.js';

function viewport(wide: boolean) {
  window.matchMedia = ((query: string) => ({
    matches: query.includes('min-width') ? wide : !wide,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia;
}

/**
 * The view with the shell sidebar's thread list beside it (design 59, Task 9). The list shows
 * once the daemon is reachable (Q934), so this waits for it.
 */
async function show(
  daemon: ReturnType<typeof sidebarDaemon>,
  route: StudioRoute = { kind: 'home' },
  keys?: StudioKeys,
) {
  const navigate = vi.fn();
  const view = render(
    wrap(<StudioInShell agentd={daemon.agentd} route={route} navigate={navigate} keys={keys} />),
  );
  await screen.findByRole('navigation', { name: 'Projects' });
  return { navigate, view };
}

const nav = () => screen.getByRole('navigation', { name: 'Projects' });
const threadsOf = (project: string) => screen.getByRole('list', { name: `${project} threads` });
const rowTitles = (list: HTMLElement) =>
  within(list)
    .queryAllByRole('link')
    .filter((a) => a.getAttribute('href')?.startsWith('#/studio/t/'))
    .map((a) => a.getAttribute('aria-label')?.split(',')[0]);

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

describe('the sidebar', () => {
  it('sidebar_groups_by_project', async () => {
    await show(sidebarDaemon());
    await within(nav()).findByRole('link', { name: /^Fix the login redirect/ });
    // loams: newest first; acme-site's thread under it (cwd inside the checkout); the
    // thread no project holds under "Other threads".
    expect(rowTitles(threadsOf('loams'))).toEqual([
      'Fix the login redirect',
      'Speed up the build',
      'Draft the README',
      'Review the README',
    ]);
    expect(rowTitles(threadsOf('acme-site'))).toEqual(['New landing page']);
    expect(rowTitles(threadsOf('Other threads'))).toEqual(['Scratch notes']);
    // A thread in a linked worktree says so.
    expect(
      within(threadsOf('loams')).getByRole('img', { name: /Worktree: loams-fix/ }),
    ).toBeTruthy();
    // A project collapses.
    fireEvent.click(screen.getByRole('button', { name: "Hide loams's threads" }));
    expect(screen.queryByRole('list', { name: 'loams threads' })).toBeNull();
  });

  it('status_badges_follow_sessions', async () => {
    const daemon = sidebarDaemon({
      groups: [
        {
          id: 'g1',
          prompt: 'Try three fixes',
          createdAtMs: at(50),
          members: [{ chatId: 't-a' }, { chatId: 't-b' }],
        },
      ],
    });
    await show(daemon);
    const login = await within(nav()).findByRole('link', { name: /^Fix the login redirect/ });
    expect(login.getAttribute('aria-label')).toBe('Fix the login redirect');

    act(() =>
      daemon.sessions.set([
        {
          chatId: 't-login',
          status: SessionStatus.WORKING,
          updatedAtMs: at(31),
          startedAtMs: at(31),
        },
        { chatId: 't-site', status: SessionStatus.ERRORED, updatedAtMs: at(41) },
      ]),
    );
    await waitFor(() =>
      expect(login.getAttribute('aria-label')).toBe('Fix the login redirect, Working'),
    );
    expect(
      within(nav())
        .getByRole('link', { name: /^New landing page/ })
        .getAttribute('aria-label'),
    ).toBe('New landing page, Errored');

    // A pending approval outranks running.
    act(() =>
      daemon.approvals.set([{ id: 'a1', chatId: 't-login', tool: 'Bash', createdAtMs: at(32) }]),
    );
    await waitFor(() =>
      expect(login.getAttribute('aria-label')).toBe('Fix the login redirect, Pending Approval'),
    );

    // Unread: a message after the last visit.
    act(() =>
      daemon.chats.set(
        daemon.chats.get().map((c) => (c.id === 't-fix' ? { ...c, lastSeenAtMs: at(15) } : c)),
      ),
    );
    await waitFor(() =>
      expect(
        within(nav())
          .getByRole('link', { name: /^Speed up the build/ })
          .getAttribute('aria-label'),
      ).toBe('Speed up the build, Completed'),
    );

    // The session ends: the badge goes.
    act(() => {
      daemon.approvals.set([]);
      daemon.sessions.set([{ chatId: 't-login', status: SessionStatus.IDLE, updatedAtMs: at(33) }]);
    });
    await waitFor(() => expect(login.getAttribute('aria-label')).toBe('Fix the login redirect'));

    // Run-group members sit under their group's row, with its badge.
    const group = within(nav()).getByRole('link', { name: /Try three fixes/ });
    expect(group.getAttribute('href')).toBe('#/studio/g/g1');
    expect(screen.getAllByRole('img', { name: 'Run group: Try three fixes' })).toHaveLength(2);
  });

  it('shows no run groups or approvals when the daemon has none yet', async () => {
    await show(sidebarDaemon({ groups: 'unimplemented', approvals: 'unimplemented' }));
    expect(await within(nav()).findByRole('link', { name: /^Draft the README/ })).toBeTruthy();
    expect(screen.queryByRole('img', { name: /Run group/ })).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('reorder_persists_pin_order', async () => {
    const daemon = sidebarDaemon({ pins: ['t-b', 't-site', 't-a'] });
    await show(daemon);
    await within(nav()).findByRole('link', { name: /^Draft the README/ });
    await waitFor(() =>
      expect(rowTitles(threadsOf('loams')).slice(0, 2)).toEqual([
        'Review the README',
        'Draft the README',
      ]),
    );
    // Move "Draft the README" up, from the menu (the keyboard's way; a drag sends the same).
    fireEvent.click(screen.getByRole('button', { name: 'More actions for Draft the README' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Move up' }));
    await waitFor(() => expect(daemon.calls).toContain('ChangeSidebarPin:reorder:t-a::t-b'));
    await waitFor(() =>
      expect(rowTitles(threadsOf('loams')).slice(0, 2)).toEqual([
        'Draft the README',
        'Review the README',
      ]),
    );
    expect(daemon.pins.get()).toEqual(['t-a', 't-b', 't-site']);

    // Pin another: it joins the pinned block, after the last pin of its project.
    fireEvent.click(
      screen.getByRole('button', { name: 'More actions for Fix the login redirect' }),
    );
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Pin thread' }));
    await waitFor(() => expect(daemon.calls).toContain('ChangeSidebarPin:pin:t-login::'));
    await waitFor(() =>
      expect(rowTitles(threadsOf('loams')).slice(0, 3)).toEqual([
        'Draft the README',
        'Review the README',
        'Fix the login redirect',
      ]),
    );

    // Unpin: back among the rest, by recency.
    fireEvent.click(screen.getByRole('button', { name: 'More actions for Draft the README' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Unpin thread' }));
    await waitFor(() => expect(daemon.calls).toContain('ChangeSidebarPin:unpin:t-a'));
    await waitFor(() =>
      expect(rowTitles(threadsOf('loams'))).toEqual([
        'Review the README',
        'Fix the login redirect',
        'Speed up the build',
        'Draft the README',
      ]),
    );
  });

  it('delete_offers_worktree_cleanup_only_when_unshared', async () => {
    const daemon = sidebarDaemon();
    const { navigate } = await show(daemon, { kind: 'thread', threadId: 't-fix' });
    await within(nav()).findByRole('link', { name: /^Speed up the build/ });

    // Shares its worktree with "Review the README": no worktree offer.
    fireEvent.click(screen.getByRole('button', { name: 'More actions for Draft the README' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }));
    let dialog = await screen.findByRole('alertdialog');
    expect(within(dialog).queryByRole('checkbox')).toBeNull();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());

    // In the main checkout: no worktree offer either.
    fireEvent.click(
      screen.getByRole('button', { name: 'More actions for Fix the login redirect' }),
    );
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }));
    dialog = await screen.findByRole('alertdialog');
    expect(within(dialog).queryByRole('checkbox')).toBeNull();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('alertdialog')).toBeNull());

    // Its own worktree: offered, and removed when asked.
    fireEvent.click(screen.getByRole('button', { name: 'More actions for Speed up the build' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Delete' }));
    dialog = await screen.findByRole('alertdialog');
    const box = within(dialog).getByRole('checkbox', {
      name: /Also remove its worktree, loams-fix/,
    });
    expect((box as HTMLInputElement).checked).toBe(false);
    fireEvent.click(box);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete thread' }));
    await waitFor(() =>
      expect(daemon.calls).toEqual(
        expect.arrayContaining(['DeleteThread:t-fix', 'DeleteWorktree:~/src/loams:/wt/loams-fix']),
      ),
    );
    // It was open: the page moves to the project's next thread.
    await waitFor(() => expect(navigate).toHaveBeenCalledWith('/studio/t/t-login'));
    await waitFor(() =>
      expect(within(nav()).queryByRole('link', { name: /^Speed up the build/ })).toBeNull(),
    );
  });

  it('cleans up worktrees no thread uses, keeping ones with changes', async () => {
    const daemon = sidebarDaemon();
    await show(daemon);
    await within(nav()).findByRole('link', { name: /^Speed up the build/ });
    fireEvent.click(screen.getByRole('button', { name: 'Clean up worktrees' }));
    const dialog = await screen.findByRole('alertdialog');
    expect(within(dialog).getByText(/loams-old/)).toBeTruthy();
    expect(within(dialog).getByText(/loams-wip/)).toBeTruthy();
    expect(within(dialog).getByText('has changes')).toBeTruthy();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Remove 1 worktree' }));
    await waitFor(() => expect(daemon.calls).toContain('DeleteWorktree:~/src/loams:/wt/loams-old'));
    expect(daemon.calls.filter((c) => c.startsWith('DeleteWorktree'))).toHaveLength(1);
  });

  it('search_filters_threads', async () => {
    await show(sidebarDaemon());
    await within(nav()).findByRole('link', { name: /^Fix the login redirect/ });
    const search = screen.getByRole('searchbox', { name: 'Search threads' });
    fireEvent.change(search, { target: { value: 'readme' } });
    expect(rowTitles(nav())).toEqual(['Draft the README', 'Review the README']);
    // Projects with no match are hidden.
    expect(screen.queryByRole('list', { name: 'acme-site threads' })).toBeNull();
    // A branch matches too.
    fireEvent.change(search, { target: { value: 'landing' } });
    expect(rowTitles(nav())).toEqual(['New landing page']);
    fireEvent.change(search, { target: { value: 'nothing like it' } });
    expect(screen.getByText('No threads match "nothing like it".')).toBeTruthy();
    // Escape clears it.
    fireEvent.keyDown(search, { key: 'Escape' });
    expect(rowTitles(nav())).toHaveLength(6);
  });

  it('keyboard_navigation', async () => {
    const daemon = sidebarDaemon();
    const keys = createStudioKeys({ edition: 'desktop' });
    const off = listenShellCommands(keys.commands);
    const { navigate } = await show(daemon, { kind: 'project', projectId: '~/src/loams' }, keys);
    const loams = await within(nav()).findByRole('link', { name: /^loams/ });
    await within(nav()).findByRole('link', { name: /^Fix the login redirect/ });
    // ↓ moves from the project row to its first thread; Enter opens it.
    loams.focus();
    fireEvent.keyDown(loams, { key: 'ArrowDown' });
    const first = within(nav()).getByRole('link', { name: /^Fix the login redirect/ });
    expect(document.activeElement).toBe(first);
    fireEvent.keyDown(first, { key: 'ArrowDown' });
    expect(document.activeElement).toBe(
      within(nav()).getByRole('link', { name: /^Speed up the build/ }),
    );
    fireEvent.keyDown(document.activeElement as Element, { key: 'ArrowUp' });
    expect(document.activeElement).toBe(first);
    fireEvent.keyDown(first, { key: 'Enter' });
    expect(navigate).toHaveBeenCalledWith('/studio/t/t-login');
    fireEvent.keyDown(first, { key: 'End' });
    expect(document.activeElement?.textContent).toContain('Scratch notes');

    // The shell's Mod+2 (D1081) asks Studio for the second thread; Mod+Shift+] (Studio's
    // own) opens the next one (the first, from a project).
    navigate.mockClear();
    const shell = (command: string) => {
      const event = new CustomEvent(SHELL_COMMAND_EVENT, { detail: command, cancelable: true });
      act(() => void window.dispatchEvent(event));
      return event.defaultPrevented;
    };
    expect(shell('thread.jump.2')).toBe(true);
    expect(navigate).toHaveBeenCalledWith('/studio/t/t-fix');
    fireEvent.keyDown(document.body, {
      key: ']',
      code: 'BracketRight',
      ctrlKey: true,
      shiftKey: true,
    });
    expect(navigate).toHaveBeenLastCalledWith('/studio/t/t-login');

    // The shell's Mod+N starts a thread in the project in view.
    expect(shell('chat.new')).toBe(true);
    await waitFor(() => expect(daemon.calls).toContain('CreateThread:~/src/loams'));
    // Settings › Studio's approval mode, supervised by default (Q803; SD1 Task 13).
    await waitFor(() => expect(daemon.calls).toContain('SetThreadApprovalMode:true:SUPERVISED'));
    await waitFor(() =>
      expect(navigate).toHaveBeenLastCalledWith(
        expect.stringMatching(/^\/studio\/t\/[0-9a-f-]{36}$/),
      ),
    );
    off();
  });

  it('opening a thread marks it seen; rename, fork and archive go to the daemon', async () => {
    const daemon = sidebarDaemon();
    const { navigate } = await show(daemon, { kind: 'thread', threadId: 't-site' });
    await within(nav()).findByRole('link', { name: /^New landing page/ });
    await waitFor(() => expect(daemon.calls).toContain('MarkThreadSeen:t-site'));

    const row = within(nav()).getByRole('link', { name: /^Scratch notes/ });
    fireEvent.keyDown(row, { key: 'F2' });
    const dialog = await screen.findByRole('dialog', { name: 'Rename thread' });
    fireEvent.change(within(dialog).getByLabelText('Title'), { target: { value: 'Notes' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Rename' }));
    await waitFor(() => expect(daemon.calls).toContain('RenameThread:t-lost:Notes'));
    expect(await within(nav()).findByRole('link', { name: /^Notes/ })).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'More actions for Notes' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Fork thread' }));
    await waitFor(() => expect(daemon.calls).toContain('ForkThread:t-lost'));
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith(expect.stringMatching(/^\/studio\/t\/[0-9a-f-]{36}$/)),
    );

    fireEvent.click(screen.getByRole('button', { name: 'More actions for Notes' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'Archive thread' }));
    await waitFor(() => expect(daemon.calls).toContain('SetThreadArchived:t-lost:true'));
    await waitFor(() =>
      expect(within(nav()).queryByRole('link', { name: /^Notes,|^Notes$/ })).toBeNull(),
    );
    expect(screen.queryByRole('button', { name: /Show archived/ })).toBeNull();

    // Archived threads are in Studio's settings (design 59, D1079), where Unarchive puts one back.
    cleanup();
    render(
      wrap(
        <StudioInShell agentd={daemon.agentd} route={{ kind: 'settings' }} navigate={vi.fn()} />,
      ),
    );
    const archived = await screen.findByRole('list', { name: 'Archived threads' });
    expect(within(archived).getByRole('link', { name: 'Notes' }).getAttribute('href')).toBe(
      '#/studio/t/t-lost',
    );
    fireEvent.click(within(archived).getByRole('button', { name: 'Unarchive Notes' }));
    await waitFor(() => expect(daemon.calls).toContain('SetThreadArchived:t-lost:false'));
    expect(await within(nav()).findByRole('link', { name: /^Notes(,|$)/ })).toBeTruthy();
    await waitFor(() =>
      expect(screen.queryByRole('list', { name: 'Archived threads' })).toBeNull(),
    );
  });

  it('a running thread cannot be archived', async () => {
    const daemon = sidebarDaemon({
      sessions: [{ chatId: 't-login', status: SessionStatus.WORKING, updatedAtMs: at(31) }],
    });
    await show(daemon);
    await waitFor(() =>
      expect(
        within(nav()).getByRole('link', { name: 'Fix the login redirect, Working' }),
      ).toBeTruthy(),
    );
    fireEvent.click(
      screen.getByRole('button', { name: 'More actions for Fix the login redirect' }),
    );
    const item = await screen.findByRole('menuitem', { name: 'Archive thread' });
    expect(item.getAttribute('aria-disabled')).toBe('true');
  });
});

describe('the palette source (T24-6)', () => {
  it('lists the threads in their projects, and run groups when the daemon has them', async () => {
    const daemon = sidebarDaemon({
      groups: [
        { id: 'g1', prompt: 'Try three fixes', createdAtMs: at(50), members: [{ chatId: 't-a' }] },
      ],
    });
    const source = agentdPaletteSource(daemon.agentd);
    await waitFor(() => expect(daemon.agentd.reach().state).toBe('ready'));
    const threads = await source.threads();
    expect(threads.find((t) => t.id === 't-fix')).toMatchObject({
      title: 'Speed up the build',
      projectId: '~/src/loams',
      branch: 'fix/build',
    });
    expect(threads.find((t) => t.id === 't-lost')?.projectId).toBe('');
    expect(await source.runGroups()).toEqual([
      {
        id: 'g1',
        title: 'Try three fixes',
        projectId: '~/src/loams',
        members: 1,
        updatedAt: Number(at(50)),
      },
    ]);
    // The other rows stay with Task 24's source.
    expect(await source.searchFiles('t-fix', 'x')).toEqual([]);
  });

  it('answers no run groups before Task 12, and nothing without a daemon', async () => {
    const daemon = sidebarDaemon({ groups: 'unimplemented' });
    await waitFor(() => expect(daemon.agentd.reach().state).toBe('ready'));
    expect(await agentdPaletteSource(daemon.agentd).runGroups()).toEqual([]);
    expect(await agentdPaletteSource({ transport: () => undefined }).threads()).toEqual([]);
  });
});
