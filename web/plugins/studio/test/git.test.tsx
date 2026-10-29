// Studio's branch and git toolbar over a daemon (SD1 Task 23; §53 §5.7, §8.2): the
// containers wire @loams/studio-ui's toolbar to loams-agentd's GitService and
// DiffService (Task 9's writes) through the agentd transport. Every write is confirmed
// first and carries what the person saw: the diff checksum, the source head, a lease.

import { create } from '@bufbuild/protobuf';
import { Code, ConnectError, createRouterTransport, type Transport } from '@connectrpc/connect';
import { gateTransport } from '@loams/console-host';
import { AgentdTransportProvider } from '@loams/console-host/query';
import {
  ChangeRequestProvider,
  ChangeRequestState,
  ChatSchema,
  type CommitWorktreeRequest,
  type CreateChangeRequestRequest,
  DiffService,
  GitService,
  type MergeBranchRequest,
  MergeStrategy,
  type ProjectAction,
  ProjectActionIcon,
  ProjectActionSchema,
  ProjectService,
  type PushBranchRequest,
  ThreadService,
  WorktreeInfoSchema,
} from '@loams/proto/agentd';
import { ErrorInfoSchema } from '@loams/proto/errors';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import type { StudioFactory } from '../src/factory.js';
import { GitActionBus, withGitActions } from '../src/git/bus.js';
import { ProjectGitTools, ThreadGitTools } from '../src/git/GitToolbar.js';
import { pendingPaletteSource } from '../src/palette/source.js';
import { wrap } from './fakes.js';

afterEach(cleanup);

const REPO = '/srv/site';
const WT = '/w/site/brave-otter';

function refuse(
  reason: string,
  message: string,
  code: Code,
  metadata: Record<string, string> = {},
) {
  return new ConnectError(message, code, undefined, [
    { desc: ErrorInfoSchema, value: create(ErrorInfoSchema, { reason, metadata }) },
  ]);
}

interface DaemonState {
  dirty: boolean;
  ahead: number;
  behind: number;
  aheadOfDefault: number;
  checksum: string;
  files: { path: string; additions: number; deletions: number }[];
  pr: { number: number; url: string; provider: string } | null;
  mergeConflict: string[] | null;
  pushRejectedOnce: boolean;
  forgejoOnly: boolean;
}

/** A daemon with one repository, its main checkout and one worktree with a thread in it. */
function gitDaemon(over: Partial<DaemonState> = {}) {
  const state: DaemonState = {
    dirty: true,
    ahead: 0,
    behind: 0,
    aheadOfDefault: 2,
    checksum: 'sum-1',
    files: [{ path: 'src/login.ts', additions: 3, deletions: 1 }],
    pr: null,
    mergeConflict: null,
    pushRejectedOnce: false,
    forgejoOnly: true,
    ...over,
  };
  const calls: string[] = [];
  const commits: CommitWorktreeRequest[] = [];
  const pushes: PushBranchRequest[] = [];
  const prs: CreateChangeRequestRequest[] = [];
  const merges: MergeBranchRequest[] = [];
  let actions: ProjectAction[] = [];
  let nextAction = 1;
  const transport: Transport = createRouterTransport(({ service }) => {
    service(ProjectService, {
      listRepos: () => ({ repos: [{ path: REPO, name: 'site', defaultBranch: 'main' }] }),
      listProjectActions: () => ({ snapshot: { spaceId: 'sp_1', actions } }),
      upsertProjectAction: (req) => {
        calls.push(`UpsertProjectAction:${req.actionId ?? 'new'}:${req.action?.name}`);
        const id = req.actionId ?? `a_${nextAction++}`;
        const action = create(ProjectActionSchema, {
          id,
          name: req.action?.name ?? '',
          command: req.action?.command ?? '',
          icon: req.action?.icon ?? ProjectActionIcon.PLAY,
          runOnWorktreeCreate: req.action?.runOnWorktreeCreate ?? false,
        });
        actions = req.actionId
          ? actions.map((a) => (a.id === id ? action : a))
          : [...actions, action];
        return { snapshot: { spaceId: req.spaceId, actions } };
      },
      deleteProjectAction: (req) => {
        calls.push(`DeleteProjectAction:${req.actionId}`);
        actions = actions.filter((a) => a.id !== req.actionId);
        return { snapshot: { spaceId: req.spaceId, actions } };
      },
    });
    service(ThreadService, {
      async *watchThreads(_req, ctx) {
        yield {
          chats: [
            create(ChatSchema, {
              id: 'c_1',
              cwd: WT,
              branch: 'feature/login',
              spaceId: 'sp_1',
              sourceContext: { repoRoot: REPO, cwd: WT, branch: 'feature/login' },
            }),
          ],
        };
        await new Promise<void>((resolve) => ctx.signal.addEventListener('abort', () => resolve()));
      },
    });
    service(DiffService, {
      getCheckoutDiff: (req) => ({
        diff: {
          checkoutId: `co:${req.cwd}`,
          cwd: req.cwd,
          checksum: state.checksum,
          files: state.dirty ? state.files.map((f) => ({ ...f, status: 'modified' })) : [],
        },
      }),
      discardPaths: (req) => {
        calls.push(`DiscardPaths:${req.paths.join(',')}:${req.expectedChecksum}`);
        if (req.expectedChecksum !== state.checksum) {
          throw refuse('stale_diff', 'stale', Code.FailedPrecondition);
        }
        return {};
      },
    });
    service(GitService, {
      listWorktrees: () => ({
        worktrees: [
          create(WorktreeInfoSchema, { path: REPO, branch: 'main', head: 'm000' }),
          create(WorktreeInfoSchema, {
            path: WT,
            branch: 'feature/login',
            head: 'abc12345ffff',
            chatIds: ['c_1'],
            dirty: state.dirty,
            ahead: state.ahead,
            behind: state.behind,
            lastActivityMs: 1_700_000_000_000n,
          }),
        ],
      }),
      listBranches: () => ({ branches: ['main', 'dev', 'feature/login'] }),
      listRefs: () => ({
        refs: [
          { name: 'main', current: true, worktreePath: REPO },
          { name: 'dev', current: false },
          { name: 'feature/login', current: false, worktreePath: WT },
        ],
      }),
      listGitHistory: () => ({
        page: {
          comparison: { base: 'origin/main', ahead: BigInt(state.aheadOfDefault), behind: 0n },
        },
      }),
      async *watchChangeRequest(req, ctx) {
        if (state.pr) {
          yield {
            status: {
              cwd: req.cwd,
              branch: req.branch ?? '',
              changeRequest: {
                provider: state.pr.provider,
                number: BigInt(state.pr.number),
                title: 'Fix login',
                url: state.pr.url,
                state: ChangeRequestState.OPEN,
                headRef: 'feature/login',
                baseRef: 'main',
              },
            },
          };
        }
        await new Promise<void>((resolve) => ctx.signal.addEventListener('abort', () => resolve()));
      },
      commitWorktree: (req) => {
        commits.push(req);
        if (req.expectedChecksum !== state.checksum) {
          throw refuse(
            'stale_diff',
            'the working tree changed since the commit was confirmed',
            Code.FailedPrecondition,
          );
        }
        state.dirty = false;
        state.ahead += 1;
        return { sha: 'c0ffee1234567', summary: req.message };
      },
      pushBranch: (req) => {
        pushes.push(req);
        if (state.pushRejectedOnce) {
          state.pushRejectedOnce = false;
          throw refuse('push_rejected', 'non-fast-forward', Code.FailedPrecondition);
        }
        return { remote: req.remote ?? 'origin', ref: `refs/heads/${req.branch}` };
      },
      createChangeRequest: (req) => {
        prs.push(req);
        if (state.forgejoOnly && req.provider !== ChangeRequestProvider.FORGEJO) {
          throw refuse('provider_not_configured', 'no GitHub credential', Code.FailedPrecondition);
        }
        return {
          provider: req.provider,
          number: 7n,
          url: 'https://git.example.test/acme/site/pulls/7',
          state: ChangeRequestState.OPEN,
        };
      },
      mergeBranch: (req) => {
        merges.push(req);
        if (state.mergeConflict) {
          const paths = JSON.stringify(state.mergeConflict);
          throw refuse('merge_conflict', `merge_conflict: ${paths}`, Code.Aborted, { paths });
        }
        return { sha: 'feed5678' };
      },
      fetchAll: () => {
        calls.push('FetchAll');
        return { ok: true };
      },
      switchRef: (req) => {
        calls.push(`SwitchRef:${req.repoPath}:${req.refName}`);
        return { branch: req.refName };
      },
      createWorktree: (req) => {
        calls.push(`CreateWorktree:${req.repoPath}:${req.branch}`);
        return {
          outcome: {
            repoPath: req.repoPath,
            path: '/w/site/calm-heron',
            branch: 'loams-desktop/calm-heron',
            name: 'calm-heron',
          },
        };
      },
    });
  });
  return { transport, state, calls, commits, pushes, prs, merges };
}

function mount(transport: Transport, children: ReactNode) {
  return render(
    wrap(<AgentdTransportProvider transport={transport}>{children}</AgentdTransportProvider>),
  );
}

/** Picks an option in a @loams/ui SelectMenu (Base UI selects on pointer up). */
async function pick(name: string, option: RegExp) {
  fireEvent.click(screen.getByRole('combobox', { name }));
  const item = await screen.findByRole('option', { name: option });
  fireEvent.pointerDown(item, { pointerType: 'mouse' });
  fireEvent.pointerUp(item, { pointerType: 'mouse' });
  fireEvent.click(item);
}

/** Opens the git actions menu and chooses an entry. */
async function menu(entry: RegExp) {
  const trigger = await screen.findByRole('button', { name: 'Git action options' });
  fireEvent.click(trigger);
  const item = await screen.findByRole('menuitem', { name: entry });
  fireEvent.click(item);
}

const submit = (dialog: HTMLElement) =>
  fireEvent.submit(dialog.querySelector('form') as HTMLFormElement);

describe('commit', () => {
  it('commit_flow_calls_commit_with_checksum', async () => {
    const d = gitDaemon();
    mount(d.transport, <ThreadGitTools threadId="c_1" />);
    await screen.findByRole('button', { name: /Commit, push & PR/ });
    await menu(/^Commit/);
    const dialog = await screen.findByRole('dialog', { name: 'Commit changes' });
    expect(within(dialog).getByText('src/login.ts')).toBeTruthy();
    fireEvent.change(within(dialog).getByLabelText('Commit message'), {
      target: { value: 'Fix the login form' },
    });
    // The tree moves after the person looked: the daemon refuses, nothing is written.
    d.state.checksum = 'sum-2';
    submit(dialog);
    await within(dialog).findByText('The changes moved since you opened this');
    expect(d.commits).toHaveLength(1);
    expect(d.commits[0]).toMatchObject({
      checkoutId: `co:${WT}`,
      cwd: WT,
      message: 'Fix the login form',
      all: true,
      signoff: true,
      expectedChecksum: 'sum-1',
    });
    // The diff is read again; confirming once more carries the new checksum.
    submit(dialog);
    await waitFor(() => expect(d.commits).toHaveLength(2));
    expect(d.commits[1]?.expectedChecksum).toBe('sum-2');
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(await screen.findByText(/Committed c0ffee12/)).toBeTruthy();
  });

  it('is refused before the daemon without agentd:git-write (T6-3)', async () => {
    const d = gitDaemon();
    mount(gateTransport(d.transport), <ThreadGitTools threadId="c_1" />);
    await screen.findByRole('button', { name: /Commit, push & PR/ });
    await menu(/^Commit/);
    const dialog = await screen.findByRole('dialog', { name: 'Commit changes' });
    fireEvent.change(within(dialog).getByLabelText('Commit message'), { target: { value: 'x' } });
    submit(dialog);
    await within(dialog).findByText('Not allowed');
    expect(within(dialog).getByText(/needs the agentd:git-write permission/)).toBeTruthy();
    expect(d.commits).toHaveLength(0);
  });
});

describe('push', () => {
  it('push_uses_lease', async () => {
    const d = gitDaemon({ dirty: false, ahead: 1, pushRejectedOnce: true });
    mount(d.transport, <ThreadGitTools threadId="c_1" />);
    await menu(/^Push/);
    let dialog = await screen.findByRole('dialog', { name: 'Push' });
    expect(within(dialog).getByText('origin/feature/login')).toBeTruthy();
    const lease = within(dialog).getByLabelText('Force with lease') as HTMLInputElement;
    expect(lease.checked).toBe(false);
    submit(dialog);
    await within(dialog).findByText('The remote refused the push');
    expect(d.pushes[0]).toMatchObject({
      cwd: WT,
      branch: 'feature/login',
      setUpstream: true,
      forceWithLease: false,
    });
    // Only a lease is offered to overwrite: there is no plain force to choose.
    fireEvent.click(lease);
    submit(dialog);
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(d.pushes[1]).toMatchObject({ forceWithLease: true });
    expect(Object.keys(d.pushes[1] ?? {})).not.toContain('force');
    dialog = document.body;
    expect(within(dialog).getByText(/Pushed to origin/)).toBeTruthy();
  });
});

describe('pull requests', () => {
  it('open_pr_requires_provider', async () => {
    const d = gitDaemon({ dirty: false });
    mount(d.transport, <ThreadGitTools threadId="c_1" />);
    fireEvent.click(await screen.findByRole('button', { name: /Push & create PR/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Push & open PR' });
    fireEvent.change(within(dialog).getByLabelText('Title'), { target: { value: 'Fix login' } });
    const confirm = () =>
      screen
        .getAllByRole('button', { name: 'Push & open PR' })
        .find((b) => b.getAttribute('type') === 'submit');
    // No forge chosen: nothing can be sent.
    expect(confirm()?.hasAttribute('disabled')).toBe(true);
    submit(dialog);
    expect(d.prs).toHaveLength(0);
    await pick('Forge', /GitHub/);
    fireEvent.change(within(dialog).getByLabelText('Closes issue'), { target: { value: '42' } });
    fireEvent.click(within(dialog).getByLabelText('Draft'));
    submit(dialog);
    await within(dialog).findByText('No credential for this forge');
    expect(d.prs[0]).toMatchObject({ provider: ChangeRequestProvider.GITHUB });
    // The push went through before the PR failed: a retry only opens the PR.
    expect(d.pushes).toHaveLength(1);
    await pick('Forge', /Forgejo/);
    submit(dialog);
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(d.pushes).toHaveLength(1);
    expect(d.prs[1]).toMatchObject({
      cwd: WT,
      provider: ChangeRequestProvider.FORGEJO,
      base: 'main',
      head: 'feature/login',
      title: 'Fix login',
      draft: true,
      linkIssue: '42',
    });
    expect(await screen.findByText(/Opened PR #7/)).toBeTruthy();
  });
});

describe('merge', () => {
  it('merge_conflict_shown_with_paths', async () => {
    const d = gitDaemon({ dirty: false, mergeConflict: ['a.txt', 'b c.txt'] });
    mount(d.transport, <ThreadGitTools threadId="c_1" />);
    await menu(/^Merge into base/);
    const dialog = await screen.findByRole('dialog', { name: 'Merge feature/login into main' });
    submit(dialog);
    await within(dialog).findByText('The merge stopped on conflicts');
    const list = within(dialog).getByRole('list', { name: 'Conflicting paths' });
    expect(
      within(list)
        .getAllByRole('listitem')
        .map((li) => li.textContent),
    ).toEqual(['a.txt', 'b c.txt']);
    expect(d.merges[0]).toMatchObject({
      repoPath: REPO,
      source: 'feature/login',
      target: 'main',
      strategy: MergeStrategy.MERGE,
      expectedSourceHead: 'abc12345ffff',
      deleteSource: false,
      removeWorktree: false,
    });
  });
});

describe('the workspace', () => {
  const project = { id: REPO, name: 'site', path: REPO, branch: 'main' };

  it('local_vs_worktree_choice', async () => {
    const d = gitDaemon({ dirty: false });
    mount(d.transport, <ProjectGitTools project={project} />);
    // Local: a pick of branch is confirmed, then switched.
    fireEvent.click(await screen.findByRole('button', { name: 'Branch: main' }));
    const list = await screen.findByRole('list', { name: 'Branches' });
    fireEvent.click(within(list).getByRole('button', { name: /^dev/ }));
    const ask = await screen.findByRole('alertdialog', { name: 'Switch to dev?' });
    expect(d.calls.filter((c) => c.startsWith('SwitchRef'))).toEqual([]);
    fireEvent.click(within(ask).getByRole('button', { name: 'Switch branch' }));
    await waitFor(() => expect(d.calls).toContain(`SwitchRef:${REPO}:dev`));

    // New worktree: the base is chosen, the creation is confirmed, its setup shown.
    await pick('Workspace', /New worktree/);
    expect(await screen.findByRole('button', { name: 'Branch: From main' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: /Create worktree/ }));
    const confirm = await screen.findByRole('alertdialog', { name: 'Create a worktree?' });
    expect(d.calls.filter((c) => c.startsWith('CreateWorktree'))).toEqual([]);
    fireEvent.click(within(confirm).getByRole('button', { name: 'Create worktree' }));
    await waitFor(() => expect(d.calls).toContain(`CreateWorktree:${REPO}:main`));
    const setup = await screen.findByRole('dialog', { name: 'New worktree' });
    expect(await within(setup).findByText('Worktree ready')).toBeTruthy();
    fireEvent.click(within(setup).getByRole('button', { name: /Details/ }));
    expect(setup.textContent).toContain('loams-desktop/calm-heron');
    expect(setup.textContent).toContain('/w/site/calm-heron');
  });

  it('a branch already in a worktree selects that worktree, writing nothing', async () => {
    const d = gitDaemon();
    mount(d.transport, <ProjectGitTools project={project} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Branch: main' }));
    const list = await screen.findByRole('list', { name: 'Branches' });
    fireEvent.click(within(list).getByRole('button', { name: /feature\/login/ }));
    expect(await screen.findByRole('button', { name: 'Branch: feature/login' })).toBeTruthy();
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(d.calls.filter((c) => c.startsWith('SwitchRef'))).toEqual([]);
  });
});

describe('project actions', () => {
  it('project_action_editor_round_trip', async () => {
    const d = gitDaemon();
    mount(d.transport, <ThreadGitTools threadId="c_1" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Project actions' }));
    const dialog = await screen.findByRole('dialog', { name: 'Project actions' });
    await within(dialog).findByText('No actions yet.');
    fireEvent.click(within(dialog).getByRole('button', { name: /Add action/ }));
    const form = within(dialog).getByRole('form', { name: 'New project action' });
    fireEvent.change(within(form).getByLabelText('Name'), { target: { value: 'Test' } });
    fireEvent.change(within(form).getByLabelText(/^Command/), { target: { value: 'pnpm test' } });
    fireEvent.click(within(form).getByLabelText(/Run when a worktree is created/));
    fireEvent.submit(form);
    await waitFor(() => expect(d.calls).toContain('UpsertProjectAction:new:Test'));
    const list = await within(dialog).findByRole('list', { name: 'Actions' });
    expect(within(list).getByText('pnpm test')).toBeTruthy();
    expect(within(list).getByText('setup')).toBeTruthy();

    // Edit it: the same id goes back with the new command.
    fireEvent.click(within(list).getByRole('button', { name: 'Edit Test' }));
    const edit = within(dialog).getByRole('form', { name: 'Edit Test' });
    expect((within(edit).getByLabelText(/^Command/) as HTMLInputElement).value).toBe('pnpm test');
    fireEvent.change(within(edit).getByLabelText(/^Command/), { target: { value: 'pnpm vitest' } });
    fireEvent.submit(edit);
    await waitFor(() => expect(d.calls).toContain('UpsertProjectAction:a_1:Test'));
    expect(await within(dialog).findByText('pnpm vitest')).toBeTruthy();

    // Delete asks first.
    fireEvent.click(within(dialog).getByRole('button', { name: 'Delete Test' }));
    const ask = await screen.findByRole('alertdialog', { name: 'Delete Test?' });
    expect(d.calls.filter((c) => c.startsWith('Delete'))).toEqual([]);
    fireEvent.click(within(ask).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(d.calls).toContain('DeleteProjectAction:a_1'));
    expect(await within(dialog).findByText('No actions yet.')).toBeTruthy();
  });
});

describe('the palette (T24-6)', () => {
  it("runs a git command through the toolbar's own confirmation", async () => {
    const d = gitDaemon({ dirty: false });
    const bus = new GitActionBus();
    const source = withGitActions(pendingPaletteSource(), bus);
    mount(d.transport, <ThreadGitTools threadId="c_1" bus={bus} />);
    await screen.findByRole('button', { name: /Push & create PR/ });
    await waitFor(async () =>
      expect(await source.gitActions('c_1')).toContainEqual({
        action: 'merge',
        label: 'Merge into base',
      }),
    );
    expect(await source.gitActions('c_1')).toContainEqual({
      action: 'commit',
      label: 'Commit',
      disabledReason: 'Worktree is clean. Make changes before committing.',
    });
    await act(() => source.runGitAction('c_1', 'merge'));
    const dialog = await screen.findByRole('dialog', { name: 'Merge feature/login into main' });
    expect(d.merges).toHaveLength(0);
    submit(dialog);
    await waitFor(() => expect(d.merges).toHaveLength(1));
    await expect(source.runGitAction('c_9', 'push')).rejects.toThrow(/Open the thread/);
  });
});

describe('the thread toolbar', () => {
  it("shows the branch's Forgejo PR from the factory (T27-5)", async () => {
    const d = gitDaemon({
      dirty: false,
      pr: { number: 12, url: 'https://git.example.test/acme/site/pulls/12', provider: 'forgejo' },
    });
    const pulls = vi.fn(async () => [
      {
        id: '12',
        title: 'Fix login',
        head: 'feature/login',
        url: 'https://git.example.test/acme/site/pulls/12',
      },
    ]);
    const factory: StudioFactory = { kind: 'pages', list: async () => [], pulls };
    mount(d.transport, <ThreadGitTools threadId="c_1" factory={factory} />);
    const toolbar = await screen.findByRole('toolbar', { name: 'Branch and git' });
    await waitFor(() => expect(pulls).toHaveBeenCalledWith('acme', 'site'));
    const links = await within(toolbar).findAllByRole('link', { name: /PR #12/ });
    expect(links.map((l) => l.getAttribute('href'))).toContain(
      'https://git.example.test/acme/site/pulls/12',
    );
    // The thread's checkout and branch are fixed.
    expect(
      within(toolbar)
        .getByRole('button', { name: 'Branch: feature/login' })
        .hasAttribute('disabled'),
    ).toBe(true);
  });
});
