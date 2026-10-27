// Parallel runs in Studio (SD1 Task 25; design §53 §5.5, §8.1, D904): `/studio/g/:id` over a
// daemon's RunGroupService (Task 12). The grid follows `WatchRunGroups`; resolving is a git
// write, confirmed in a dialog that names every write and carries what the person saw (the
// winner's diff checksum and head); "Approve for all variants" answers each member once; the
// judge is an ordinary read-only thread.

import { create, fromJson, type MessageInitShape } from '@bufbuild/protobuf';
import { ValueSchema } from '@bufbuild/protobuf/wkt';
import { Code, ConnectError, createRouterTransport, type Transport } from '@connectrpc/connect';
import { gateTransport } from '@loams/console-host';
import { AgentdTransportProvider } from '@loams/console-host/query';
import {
  ApprovalDecision,
  type ApprovalSchema,
  ApprovalService,
  ChangeRequestProvider,
  CheckState,
  DiffService,
  type GetRunGroupComparisonRequest,
  GitService,
  HarnessId,
  MergeStrategy,
  ResolveAction,
  type ResolveApprovalRequest,
  type ResolveRunGroupRequest,
  type RunGroupMemberSchema,
  type RunGroupSchema,
  RunGroupService,
  RunGroupState,
  RunMemberStatus,
  SandboxLevel,
  type SendRequest,
  ThreadService,
} from '@loams/proto/agentd';
import { ErrorInfoSchema } from '@loams/proto/errors';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test';
import { GroupPage } from '../src/groups/GroupPage.js';
import { wrap } from './fakes.js';
import { live } from './sidebar-fakes.js';

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(cleanup);

type GroupInit = MessageInitShape<typeof RunGroupSchema>;
type ApprovalInit = MessageInitShape<typeof ApprovalSchema>;

const REPO = '/srv/site';

type MemberInit = MessageInitShape<typeof RunGroupMemberSchema>;

function memberInit(
  chatId: string,
  harness: HarnessId,
  over: Partial<MemberInit> = {},
): MemberInit {
  return {
    chatId,
    harness,
    status: RunMemberStatus.DONE,
    filesChanged: 1,
    additions: 3,
    deletions: 1,
    elapsedMs: 61_000n,
    cwd: `/wt/site-${chatId}`,
    branch: `loams-desktop/${chatId}`,
    check: {},
    ...over,
  } as MemberInit;
}

function groupInit(over: Partial<GroupInit> = {}): GroupInit {
  return {
    id: 'g1',
    prompt: 'Fix the login redirect loop',
    base: 'main',
    baseSha: 'b45e000',
    repoPath: REPO,
    spaceId: 'sp_1',
    createdAtMs: 1_700_000_000_000n,
    state: RunGroupState.READY,
    checkActionId: 'act_test',
    members: [
      memberInit('m1', HarnessId.CLAUDE_CODE, { model: 'sonnet' }),
      memberInit('m2', HarnessId.CODEX, { additions: 10, deletions: 4, filesChanged: 2 }),
    ],
    ...over,
  } as GroupInit;
}

const PATCH = `diff --git a/src/login.ts b/src/login.ts
index 1111111..2222222 100644
--- a/src/login.ts
+++ b/src/login.ts
@@ -1,2 +1,2 @@
-export const loop = true;
+export const loop = false;
 export const keep = 1;
`;

function refuse(reason: string, message: string, code: Code) {
  return new ConnectError(message, code, undefined, [
    { desc: ErrorInfoSchema, value: create(ErrorInfoSchema, { reason }) },
  ]);
}

function groupDaemon(
  o: {
    group?: GroupInit;
    approvals?: ApprovalInit[];
    resolve?: (req: ResolveRunGroupRequest) => void;
  } = {},
) {
  const groups = live<GroupInit[]>([o.group ?? groupInit()]);
  const approvals = live<ApprovalInit[]>(o.approvals ?? []);
  const resolves: ResolveRunGroupRequest[] = [];
  const answers: ResolveApprovalRequest[] = [];
  const comparisons: GetRunGroupComparisonRequest[] = [];
  const sends: SendRequest[] = [];
  const calls: string[] = [];
  const transport: Transport = createRouterTransport(({ service }) => {
    service(RunGroupService, {
      async *watchRunGroups(_req, ctx) {
        for await (const v of groups.stream(ctx.signal)) yield { groups: v };
      },
      getRunGroupComparison: (req) => {
        comparisons.push(req);
        return {
          matrix: [
            {
              path: 'src/login.ts',
              perMember: [
                { chatId: 'm1', additions: 3, deletions: 1 },
                { chatId: 'm2', additions: 8, deletions: 4 },
              ],
            },
            { path: 'src/util.ts', perMember: [{ chatId: 'm2', additions: 2, deletions: 0 }] },
          ],
          pairDiff: { cwd: '', patch: PATCH, checksum: 'pair' },
        };
      },
      runGroupCheck: (req) => {
        calls.push(`RunGroupCheck:${req.groupId}:${req.chatId ?? ''}`);
        return { runs: [] };
      },
      resolveRunGroup: (req) => {
        resolves.push(req);
        o.resolve?.(req);
        if (req.action === ResolveAction.CHANGE_REQUEST) {
          return { result: 'opened forgejo change request #7', url: 'https://git.example/pr/7' };
        }
        return { result: `merged ${req.merge?.source} into ${req.merge?.target}`, sha: 'f00d1234' };
      },
    });
    service(ThreadService, {
      async *watchThreads(_req, ctx) {
        yield {
          chats: [
            {
              id: 'm1',
              title: 'Fix the login · claude-code sonnet',
              cwd: '/wt/site-m1',
              lastMessagePreview: 'I removed the second redirect.',
            },
            {
              id: 'm2',
              title: 'Fix the login · codex',
              cwd: '/wt/site-m2',
              lastMessagePreview: 'The loop came from the guard.',
            },
          ],
        };
        await new Promise<void>((resolve) => ctx.signal.addEventListener('abort', () => resolve()));
      },
      send: (req) => {
        sends.push(req);
        return { commandId: 'cmd-1' };
      },
    });
    service(ApprovalService, {
      async *watchApprovals(_req, ctx) {
        for await (const v of approvals.stream(ctx.signal)) yield { pending: v };
      },
      resolveApproval: (req) => {
        answers.push(req);
        approvals.set(approvals.get().filter((a) => a.id !== req.callId));
        return {};
      },
    });
    service(DiffService, {
      getCheckoutDiff: (req) => {
        calls.push(`GetCheckoutDiff:${req.cwd}`);
        return {
          diff: {
            cwd: req.cwd,
            checkoutId: `co:${req.cwd}`,
            checksum: `sum:${req.cwd}`,
            files: [{ path: 'src/login.ts', status: 'modified', additions: 1, deletions: 1 }],
            additions: 1,
            deletions: 1,
          },
        };
      },
    });
    service(GitService, {
      listWorktrees: () => ({
        worktrees: [
          { path: REPO, branch: 'main', head: 'b45e000' },
          { path: '/wt/site-m1', branch: 'loams-desktop/m1', head: 'head-m1', chatIds: ['m1'] },
          { path: '/wt/site-m2', branch: 'loams-desktop/m2', head: 'head-m2', chatIds: ['m2'] },
        ],
      }),
    });
  });
  return { transport, groups, approvals, resolves, answers, comparisons, sends, calls };
}

function mount(transport: Transport, navigate = vi.fn()) {
  render(
    wrap(
      <AgentdTransportProvider transport={transport}>
        <GroupPage groupId="g1" navigate={navigate} />
      </AgentdTransportProvider>,
    ),
  );
  return navigate;
}

function column(chatId: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`[data-member="${chatId}"]`);
  if (!el) throw new Error(`no column for ${chatId}`);
  return el;
}

async function openResolve() {
  fireEvent.click(await screen.findByRole('button', { name: 'Resolve…' }));
  return screen.findByRole('dialog', { name: 'Resolve the run group' });
}

describe('group_grid_follows_watch', () => {
  it('shows one column per member and follows each new frame', async () => {
    const d = groupDaemon({
      group: groupInit({
        state: RunGroupState.RUNNING,
        members: [
          memberInit('m1', HarnessId.CLAUDE_CODE, {
            model: 'sonnet',
            status: RunMemberStatus.RUNNING,
            startedAtMs: BigInt(Date.now() - 5_000),
          }),
          memberInit('m2', HarnessId.CODEX, { status: RunMemberStatus.QUEUED, additions: 0 }),
        ],
      }),
    });
    mount(d.transport);
    await screen.findByRole('heading', { name: 'Fix the login redirect loop' });
    await waitFor(() => expect(within(column('m1')).getByText('Running')).toBeTruthy());
    expect(within(column('m1')).getByText('Claude Code · sonnet')).toBeTruthy();
    expect(within(column('m1')).getByText('I removed the second redirect.')).toBeTruthy();
    expect(within(column('m1')).getByText('+3')).toBeTruthy();
    expect(within(column('m2')).getByText('Queued')).toBeTruthy();
    // Tokens are unknown until the harness reports them (T12-10).
    expect(within(column('m1')).getByText('Tokens —')).toBeTruthy();

    act(() =>
      d.groups.set([
        groupInit({
          members: [
            memberInit('m1', HarnessId.CLAUDE_CODE, {
              model: 'sonnet',
              additions: 12,
              tokens: 4200n,
              check: { state: CheckState.PASSED, exitCode: 0, logTail: ['ok 12 tests'] },
            }),
            memberInit('m2', HarnessId.CODEX, {
              status: RunMemberStatus.ERRORED,
              check: { state: CheckState.FAILED, exitCode: 1, logTail: ['FAIL login.test.ts'] },
            }),
          ],
        }),
      ]),
    );
    await waitFor(() => expect(within(column('m1')).getByText('Done')).toBeTruthy());
    expect(within(column('m1')).getByText('+12')).toBeTruthy();
    expect(within(column('m1')).getByText('Tokens 4,200')).toBeTruthy();
    expect(within(column('m1')).getByText('Check passed')).toBeTruthy();
    expect(within(column('m2')).getByText('Errored')).toBeTruthy();
    expect(within(column('m2')).getByText('Check failed (exit 1)')).toBeTruthy();
    // The check results show each log tail as text.
    expect(screen.getByText('FAIL login.test.ts')).toBeTruthy();
  });

  it('runs the check on all, and continues a member as a thread', async () => {
    const d = groupDaemon();
    const navigate = mount(d.transport);
    fireEvent.click(await screen.findByRole('button', { name: 'Run check on all' }));
    await waitFor(() => expect(d.calls).toContain('RunGroupCheck:g1:'));
    fireEvent.click(within(column('m2')).getByRole('button', { name: 'Continue as thread' }));
    expect(navigate).toHaveBeenCalledWith('/studio/t/m2');
  });

  it('says so when the group does not exist', async () => {
    const d = groupDaemon();
    act(() => d.groups.set([]));
    mount(d.transport);
    await screen.findByText('This run group is not on the daemon');
  });
});

describe('matrix_marks_touched_files', () => {
  it('reads the comparison and marks which members touched which files', async () => {
    const d = groupDaemon();
    mount(d.transport);
    const table = await screen.findByRole('table', { name: 'Files changed by each variant' });
    const util = await within(table).findByRole('row', { name: /src\/util\.ts/ });
    expect(
      within(util)
        .getAllByRole('cell')
        .map((c) => c.getAttribute('data-touched')),
    ).toEqual(['false', 'true']);
    expect(d.comparisons[0]).toMatchObject({ groupId: 'g1', a: 'm1' });
  });
});

describe('pair_diff_side_by_side', () => {
  it('asks for the pair’s diff of one file and shows it split', async () => {
    const d = groupDaemon();
    mount(d.transport);
    const table = await screen.findByRole('table', { name: 'Files changed by each variant' });
    const login = await within(table).findByRole('row', { name: /src\/login\.ts/ });
    fireEvent.click(within(login).getByRole('button', { name: 'Compare src/login.ts from Codex' }));
    await waitFor(() =>
      expect(d.comparisons.at(-1)).toMatchObject({
        groupId: 'g1',
        a: 'm1',
        b: 'm2',
        path: 'src/login.ts',
      }),
    );
    const host = await waitFor(() => {
      const found = document.querySelector<HTMLElement>(
        '[data-pair-file="src/login.ts"] diffs-container',
      );
      if (!found) throw new Error('no viewer yet');
      return found;
    });
    await waitFor(() =>
      expect(
        host.shadowRoot?.querySelector('[data-diff-type]')?.getAttribute('data-diff-type'),
      ).toBe('split'),
    );
  });
});

describe('resolve_merge_flow', () => {
  it('confirms a merge naming every write, then sends the checksum and head it showed', async () => {
    const d = groupDaemon();
    mount(d.transport);
    const dialog = await openResolve();
    // Opening the dialog writes nothing.
    expect(d.resolves).toHaveLength(0);
    fireEvent.change(within(dialog).getByLabelText('Winner'), { target: { value: 'm2' } });
    // What the person confirms: the winner's uncommitted work and head, read now.
    await within(dialog).findByText(/Commit 1 uncommitted file in loams-desktop\/m2 first/);
    expect(within(dialog).getByText(/Merge loams-desktop\/m2 \(head-m2\) into main/)).toBeTruthy();
    expect(within(dialog).getByText(/Archive the other run: Claude Code · sonnet/)).toBeTruthy();
    // Removing the others' worktrees is off until asked for, and then said plainly.
    const remove = within(dialog).getByLabelText('Remove the other worktrees') as HTMLInputElement;
    expect(remove.checked).toBe(false);
    fireEvent.click(remove);
    expect(within(dialog).getByText(/Delete the worktree \/wt\/site-m1/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Merge into main' }));
    // Deleting worktrees is asked about once more, on its own.
    const sure = await screen.findByRole('alertdialog', { name: 'Remove the other worktrees?' });
    expect(d.resolves).toHaveLength(0);
    fireEvent.click(within(sure).getByRole('button', { name: 'Remove and resolve' }));
    await waitFor(() => expect(d.resolves).toHaveLength(1));
    expect(d.resolves[0]).toMatchObject({
      groupId: 'g1',
      winnerChatId: 'm2',
      action: ResolveAction.MERGE,
      archiveOthers: true,
      removeOtherWorktrees: true,
      force: false,
      expectedChecksum: 'sum:/wt/site-m2',
      merge: {
        repoPath: REPO,
        source: 'loams-desktop/m2',
        target: 'main',
        strategy: MergeStrategy.MERGE,
        expectedSourceHead: 'head-m2',
      },
    });
    await screen.findByText(/merged loams-desktop\/m2 into main/);
    expect(screen.queryByRole('dialog', { name: 'Resolve the run group' })).toBeNull();
  });

  it('cancelling writes nothing', async () => {
    const d = groupDaemon();
    mount(d.transport);
    const dialog = await openResolve();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'Resolve the run group' })).toBeNull(),
    );
    expect(d.resolves).toHaveLength(0);
  });

  it('a plugin without agentd:git-write is refused before the daemon', async () => {
    const d = groupDaemon();
    mount(gateTransport(d.transport));
    const dialog = await openResolve();
    await within(dialog).findByText(/Commit 1 uncommitted file/);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Merge into main' }));
    await within(dialog).findByText('Not allowed');
    expect(d.resolves).toHaveLength(0);
  });

  it('a stale confirmation keeps the dialog open and reads the winner again', async () => {
    let first = true;
    const d = groupDaemon({
      resolve: () => {
        if (first) {
          first = false;
          throw refuse('stale_diff', 'stale_diff: the diff changed', Code.FailedPrecondition);
        }
      },
    });
    mount(d.transport);
    const dialog = await openResolve();
    await within(dialog).findByText(/Commit 1 uncommitted file/);
    const reads = d.calls.filter((c) => c === 'GetCheckoutDiff:/wt/site-m1').length;
    fireEvent.click(within(dialog).getByRole('button', { name: 'Merge into main' }));
    await within(dialog).findByText(/changed since you opened this dialog/);
    await waitFor(() =>
      expect(d.calls.filter((c) => c === 'GetCheckoutDiff:/wt/site-m1').length).toBeGreaterThan(
        reads,
      ),
    );
    expect(screen.getByRole('dialog', { name: 'Resolve the run group' })).toBeTruthy();
  });
});

describe('resolve_pr_flow', () => {
  it('pushes the winner and opens a pull request, after confirmation', async () => {
    const d = groupDaemon();
    mount(d.transport);
    const dialog = await openResolve();
    fireEvent.click(within(dialog).getByRole('radio', { name: 'Open a pull request' }));
    const title = within(dialog).getByLabelText('Title') as HTMLInputElement;
    expect(title.value).toBe('Fix the login redirect loop');
    fireEvent.change(within(dialog).getByLabelText('Provider'), {
      target: { value: 'forgejo' },
    });
    fireEvent.click(within(dialog).getByLabelText('Draft'));
    await within(dialog).findByText(/Push loams-desktop\/m1 to the remote/);
    fireEvent.click(within(dialog).getByRole('button', { name: 'Push and open the pull request' }));
    await waitFor(() => expect(d.resolves).toHaveLength(1));
    expect(d.resolves[0]).toMatchObject({
      winnerChatId: 'm1',
      action: ResolveAction.CHANGE_REQUEST,
      expectedChecksum: 'sum:/wt/site-m1',
      archiveOthers: true,
      changeRequest: {
        provider: ChangeRequestProvider.FORGEJO,
        base: 'main',
        head: 'loams-desktop/m1',
        title: 'Fix the login redirect loop',
        draft: true,
      },
    });
    const link = await screen.findByRole('link', { name: 'https://git.example/pr/7' });
    expect(link.getAttribute('href')).toBe('https://git.example/pr/7');
  });
});

describe('resolve_refused_while_running_shows_reason', () => {
  it('shows the daemon’s group_not_ready reason in the dialog, which stays open', async () => {
    const d = groupDaemon({
      group: groupInit({
        state: RunGroupState.RUNNING,
        members: [
          memberInit('m1', HarnessId.CLAUDE_CODE, { status: RunMemberStatus.RUNNING }),
          memberInit('m2', HarnessId.CODEX),
        ],
      }),
      resolve: (req) => {
        if (!req.force) {
          throw refuse(
            'group_not_ready',
            'group_not_ready: 1 of the group’s members are still running',
            Code.FailedPrecondition,
          );
        }
      },
    });
    mount(d.transport);
    const dialog = await openResolve();
    fireEvent.change(within(dialog).getByLabelText('Winner'), { target: { value: 'm2' } });
    fireEvent.click(within(dialog).getByRole('radio', { name: 'Keep as a thread' }));
    fireEvent.click(within(dialog).getByRole('button', { name: 'Keep this run' }));
    await within(dialog).findByText(/1 of the group’s members are still running/);
    expect(within(dialog).getByText('Not ready')).toBeTruthy();
    expect(d.resolves[0]).toMatchObject({ action: ResolveAction.KEEP, force: false });
    // Stopping the others is a separate, explicit choice.
    fireEvent.click(within(dialog).getByLabelText('Stop the runs still going'));
    fireEvent.click(within(dialog).getByRole('button', { name: 'Keep this run' }));
    await waitFor(() => expect(d.resolves).toHaveLength(2));
    expect(d.resolves[1]).toMatchObject({ force: true });
  });
});

describe('approve_for_all_variants_resolves_each', () => {
  it('answers the same tool and arguments once in each member, and nothing else', async () => {
    const args = { command: 'pnpm test' };
    const d = groupDaemon({
      group: groupInit({ state: RunGroupState.RUNNING }),
      approvals: [
        { id: 'ap1', chatId: 'm1', tool: 'Bash', args: fromJson(ValueSchema, args) },
        { id: 'ap2', chatId: 'm2', tool: 'Bash', args: fromJson(ValueSchema, args) },
        {
          id: 'ap3',
          chatId: 'm2',
          tool: 'Bash',
          args: fromJson(ValueSchema, { command: 'rm -rf dist' }),
        },
        { id: 'ap4', chatId: 'elsewhere', tool: 'Bash', args: fromJson(ValueSchema, args) },
      ],
    });
    mount(d.transport);
    const region = await screen.findByRole('region', { name: 'Pending in several variants' });
    expect(within(region).getByText('Bash')).toBeTruthy();
    expect(within(region).getByText(/pnpm test/)).toBeTruthy();
    fireEvent.click(within(region).getByRole('button', { name: 'Approve for all variants (2)' }));
    await waitFor(() => expect(d.answers).toHaveLength(2));
    expect(d.answers.map((a) => [a.callId, a.chatId, a.decision])).toEqual([
      ['ap1', 'm1', ApprovalDecision.ONCE],
      ['ap2', 'm2', ApprovalDecision.ONCE],
    ]);
    await waitFor(() =>
      expect(screen.queryByRole('region', { name: 'Pending in several variants' })).toBeNull(),
    );
  });
});

describe('judge_thread_is_read_only', () => {
  it('starts an ordinary read-only thread with the comparison in its prompt', async () => {
    const d = groupDaemon();
    const navigate = mount(d.transport);
    fireEvent.click(await screen.findByRole('button', { name: 'Start judge thread' }));
    await waitFor(() => expect(d.sends).toHaveLength(1));
    const sent = d.sends[0] as SendRequest;
    expect(sent.request?.sandbox).toBe(SandboxLevel.READ_ONLY);
    expect(sent.request?.autoApprove).toBe(false);
    expect(sent.request?.cwd).toBe(REPO);
    expect(sent.request?.worktree).toBeUndefined();
    expect(sent.request?.prompt).toContain('Fix the login redirect loop');
    expect(sent.request?.prompt).toContain('src/util.ts');
    expect(sent.request?.prompt).toContain('export const loop = false;');
    // It is not a member, so it can never resolve the group.
    expect(sent.chatId).not.toMatch(/^m[12]$/);
    expect(d.resolves).toHaveLength(0);
    // Each member's diff from the base, read without writing.
    expect(d.comparisons.filter((c) => c.b === undefined).map((c) => c.a)).toEqual(
      expect.arrayContaining(['m1', 'm2']),
    );
    await waitFor(() => expect(navigate).toHaveBeenCalledWith(`/studio/t/${sent.chatId}`));
  });
});
