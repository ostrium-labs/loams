// `/studio/g/:groupId` (SD1 Task 25; design §53 §5.5, §8.1, D904): a run group's members
// side by side, compared, and resolved. Orca's idea; T3 Code has no equivalent (inventory
// (l)), so the container and @loams/studio-ui's group pieces are new code.
//
//   RunGroupService.WatchRunGroups          the grid (status, +/−, time, tokens, check)
//   RunGroupService.GetRunGroupComparison   the diff matrix and the pair diff (read only)
//   RunGroupService.RunGroupCheck           "Run check on all"
//   RunGroupService.ResolveRunGroup         "Resolve…": a git write (`agentd:git-write`),
//                                           always confirmed, carrying the winner's diff
//                                           checksum and head as the dialog showed them
//   ApprovalService.WatchApprovals/ResolveApproval   "Approve for all variants"
//   ThreadService.Send                      "Start judge thread": an ordinary read-only thread
//
// Every call goes over the transport of the nearest `AgentdScope`.

import { createClient, type Transport } from '@connectrpc/connect';
import { useAgentdTransport } from '@loams/console-host/query';
import {
  ApprovalDecision,
  ApprovalService,
  type Chat,
  DiffService,
  GitService,
  RunGroupService,
  SandboxLevel,
  ThreadService,
} from '@loams/proto/agentd';
import {
  CheckResults,
  type ComparisonRowView,
  DiffMatrix,
  GroupGrid,
  type GroupView,
  isLive,
  judgePrompt,
  PairDiff,
  type ResolveChoice,
  ResolveDialog,
  type ResolveOutcome,
  SharedApprovals,
  sharedApprovals,
  type WinnerState,
} from '@loams/studio-ui/groups';
import { Button, Empty, Notice, Select, Spinner, StatusTag } from '@loams/ui';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { FlaskConical, Gavel, GitMerge, Layers } from 'lucide-react';
import { type ReactNode, useCallback, useMemo, useState } from 'react';
import { httpUrl } from '../factory.js';
import { gitErrorOf } from '../git/errors.js';
import { studioHref } from '../routes.js';
import { useWatch } from '../sidebar/data.js';
import {
  applyNewThreadApprovalMode,
  newThreadConfig,
  newThreadId,
} from '../sidebar/useThreadActions.js';
import { groupView, pendingView, resolveErrorOf, resolveRequest } from './data.js';

function Padded({ children }: { children: ReactNode }) {
  return <div className="p-6">{children}</div>;
}

function Section({
  id,
  title,
  actions,
  children,
}: {
  id: string;
  title: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section aria-labelledby={id} className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <h2 id={id} className="m-0 text-micro font-medium text-faint">
          {title}
        </h2>
        {actions && <div className="ml-auto flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {children}
    </section>
  );
}

const STATE_TAG = {
  running: { status: 'progress', label: 'Running' },
  ready: { status: 'done', label: 'Ready to resolve' },
  resolved: { status: 'neutral', label: 'Resolved' },
} as const;

/** The comparison's members in a stable order, and a key that changes with their diffs. */
function diffSignature(group: GroupView): string {
  return group.members
    .map((m) => `${m.chatId}:${m.status}:${m.filesChanged}:${m.additions}:${m.deletions}`)
    .join('|');
}

/** The pair being compared: `from` empty is the group's base. */
interface Pair {
  from: string;
  to: string;
  path?: string;
}

function comparisonInput(groupId: string, pair: Pair) {
  return pair.from
    ? { groupId, a: pair.from, b: pair.to, ...(pair.path ? { path: pair.path } : {}) }
    : { groupId, a: pair.to, ...(pair.path ? { path: pair.path } : {}) };
}

/** The winner's checkout as the dialog confirms it: its diff checksum and its branch's head. */
function useWinnerState(
  transport: Transport | undefined,
  group: GroupView | undefined,
  chatId: string | null,
) {
  const member = group?.members.find((m) => m.chatId === chatId);
  const cwd = member?.cwd ?? '';
  const diff = useQuery({
    queryKey: ['studio', 'group-winner', 'diff', cwd],
    enabled: Boolean(transport && cwd),
    retry: false,
    queryFn: async ({ signal }) => {
      const res = await createClient(DiffService, transport as Transport).getCheckoutDiff(
        { cwd, mode: '' },
        { signal },
      );
      return res.diff;
    },
  });
  const worktrees = useQuery({
    queryKey: ['studio', 'group-winner', 'worktrees', group?.repoPath ?? ''],
    enabled: Boolean(transport && group?.repoPath),
    retry: false,
    queryFn: ({ signal }) =>
      createClient(GitService, transport as Transport).listWorktrees(
        { repoPath: group?.repoPath ?? '' },
        { signal },
      ),
  });
  const worktree = worktrees.data?.worktrees.find((w) => w.path === cwd);
  const state: WinnerState = !member
    ? { loading: false, error: 'Pick a winner.' }
    : diff.isPending || worktrees.isPending
      ? { loading: true }
      : diff.error || worktrees.error
        ? { loading: false, error: (diff.error ?? worktrees.error)?.message ?? 'Unreadable' }
        : !worktree
          ? { loading: false, error: `The worktree ${cwd} is no longer on the daemon.` }
          : {
              loading: false,
              uncommittedFiles: diff.data?.files.length ?? 0,
              ...(diff.data?.checksum ? { checksum: diff.data.checksum } : {}),
              ...(worktree.head ? { head: worktree.head } : {}),
            };
  return {
    state,
    branch: worktree?.branch || member?.branch || '',
    cwd,
    refresh: () => Promise.all([diff.refetch(), worktrees.refetch()]).then(() => undefined),
  };
}

export function GroupPage({
  groupId,
  navigate,
}: {
  groupId: string;
  navigate: (to: string) => void;
}) {
  const transport = useAgentdTransport();
  const queryClient = useQueryClient();
  const groups = useWatch(
    (t, signal) => createClient(RunGroupService, t).watchRunGroups({}, { signal }),
    'studio-group-page-groups',
  );
  const threads = useWatch(
    (t, signal) => createClient(ThreadService, t).watchThreads({}, { signal }),
    'studio-group-page-threads',
  );
  const approvals = useWatch(
    (t, signal) => createClient(ApprovalService, t).watchApprovals({}, { signal }),
    'studio-group-page-approvals',
  );

  const record = groups.data?.groups.find((g) => g.id === groupId);
  const chats = useMemo(
    () => new Map<string, Chat>((threads.data?.chats ?? []).map((c) => [c.id, c])),
    [threads.data],
  );
  const memberIds = useMemo(() => new Set(record?.members.map((m) => m.chatId)), [record]);
  const pending = useMemo(
    () => (approvals.data?.pending ?? []).filter((a) => memberIds.has(a.chatId)),
    [approvals.data, memberIds],
  );
  const group = useMemo(
    () => (record ? groupView(record, chats, pending) : undefined),
    [record, chats, pending],
  );
  const signature = group ? diffSignature(group) : '';
  const firstMember = group?.members[0]?.chatId ?? '';

  // The diff matrix: one comparison, refreshed when a member's diff moves.
  const matrix = useQuery<ComparisonRowView[]>({
    queryKey: ['studio', 'group', groupId, 'matrix', signature],
    enabled: Boolean(transport && firstMember),
    retry: false,
    placeholderData: (previous) => previous,
    queryFn: async ({ signal }) => {
      const res = await createClient(RunGroupService, transport as Transport).getRunGroupComparison(
        { groupId, a: firstMember },
        { signal },
      );
      return res.matrix.map((row): ComparisonRowView => ({
        path: row.path,
        perMember: row.perMember.map((d) => ({
          chatId: d.chatId,
          additions: d.additions,
          deletions: d.deletions,
        })),
      }));
    },
  });

  // The pair diff: the first two members until the person picks.
  const [picked, setPicked] = useState<Pair | null>(null);
  const pair: Pair | null =
    picked ??
    (group && group.members.length > 1
      ? { from: group.members[0]?.chatId ?? '', to: group.members[1]?.chatId ?? '' }
      : group?.members[0]
        ? { from: '', to: group.members[0].chatId }
        : null);
  const pairDiff = useQuery({
    queryKey: ['studio', 'group', groupId, 'pair', pair, signature],
    enabled: Boolean(transport && pair?.to),
    retry: false,
    queryFn: async ({ signal }) => {
      const res = await createClient(RunGroupService, transport as Transport).getRunGroupComparison(
        comparisonInput(groupId, pair as Pair),
        { signal },
      );
      return res.pairDiff?.patch ?? '';
    },
  });

  const labelOf = useCallback(
    (chatId: string) => group?.members.find((m) => m.chatId === chatId)?.label ?? chatId,
    [group],
  );

  // Notices from the page's own actions.
  const [notice, setNotice] = useState<{
    tone: 'info' | 'danger';
    text: string;
    url?: string;
  } | null>(null);

  // "Run check on all".
  const [checking, setChecking] = useState(false);
  const runChecks = async () => {
    if (!transport) return;
    setChecking(true);
    setNotice(null);
    try {
      await createClient(RunGroupService, transport).runGroupCheck({ groupId });
      setNotice({ tone: 'info', text: 'The check is running in each finished worktree.' });
    } catch (e) {
      setNotice({ tone: 'danger', text: resolveErrorOf(e).message });
    } finally {
      setChecking(false);
    }
  };

  // "Approve for all variants": one answer per member, in order.
  const approveAll = async (list: readonly { id: string; chatId: string }[]) => {
    if (!transport) throw new Error('No daemon is reachable');
    const client = createClient(ApprovalService, transport);
    for (const a of list) {
      await client.resolveApproval({
        callId: a.id,
        chatId: a.chatId,
        decision: ApprovalDecision.ONCE,
      });
    }
  };

  // "Start judge thread" (Q811, T12-11): an ordinary read-only thread, never a member.
  const [judging, setJudging] = useState(false);
  const startJudge = async () => {
    if (!transport || !group) return;
    setJudging(true);
    setNotice(null);
    try {
      const groupsClient = createClient(RunGroupService, transport);
      const candidates = group.members.filter((m) => m.status !== 'cancelled');
      const diffs: Record<string, string> = {};
      let rows = matrix.data ?? [];
      for (const m of candidates) {
        const res = await groupsClient.getRunGroupComparison({ groupId, a: m.chatId });
        diffs[m.chatId] = res.pairDiff?.patch ?? '';
        if (rows.length === 0 && res.matrix.length > 0) {
          rows = res.matrix.map((r) => ({
            path: r.path,
            perMember: r.perMember.map((d) => ({ ...d })),
          }));
        }
      }
      const prompt = judgePrompt({
        prompt: group.prompt,
        base: group.base,
        members: candidates,
        matrix: rows,
        diffs,
      });
      const config = newThreadConfig();
      const chatId = newThreadId();
      await applyNewThreadApprovalMode(createClient(ApprovalService, transport), chatId);
      await createClient(ThreadService, transport).send({
        chatId,
        messageId: newThreadId(),
        request: {
          prompt,
          harness: config.harness,
          ...(config.model ? { model: config.model } : {}),
          cwd: group.repoPath,
          sandbox: SandboxLevel.READ_ONLY,
          autoApprove: false,
        },
      });
      navigate(studioHref({ kind: 'thread', threadId: chatId }));
    } catch (e) {
      setNotice({ tone: 'danger', text: `The judge could not start: ${gitErrorOf(e).message}` });
    } finally {
      setJudging(false);
    }
  };

  // "Resolve…".
  const [resolving, setResolving] = useState(false);
  const [winnerId, setWinnerId] = useState<string | null>(null);
  const winner = useWinnerState(transport, group, winnerId);
  const resolve = async (choice: ResolveChoice): Promise<ResolveOutcome> => {
    if (!transport || !group) {
      return { ok: false, error: { message: 'No daemon is reachable.' } };
    }
    try {
      const res = await createClient(RunGroupService, transport).resolveRunGroup(
        resolveRequest(group, choice, {
          branch: winner.branch,
          cwd: winner.cwd,
          ...(winner.state.checksum ? { checksum: winner.state.checksum } : {}),
          ...(winner.state.head ? { head: winner.state.head } : {}),
        }),
      );
      const url = httpUrl(res.url);
      setResolving(false);
      setNotice({ tone: 'info', text: `Resolved: ${res.result}`, ...(url ? { url } : {}) });
      await queryClient.invalidateQueries({ queryKey: ['studio', 'group-winner'] });
      return { ok: true };
    } catch (e) {
      const error = resolveErrorOf(e);
      // A stale confirmation reads the winner again, so the dialog shows what changed.
      if (error.reason === 'stale_diff') await winner.refresh();
      return { ok: false, error };
    }
  };

  if (!transport) {
    return (
      <Padded>
        <Empty title="No daemon is reachable">
          This run group lives on a loams-agentd that Studio cannot reach.
        </Empty>
      </Padded>
    );
  }
  if (groups.unsupported) {
    return (
      <Padded>
        <Empty title="Run groups are not available">
          This daemon does not run parallel runs. Update loams-agentd to compare runs.
        </Empty>
      </Padded>
    );
  }
  if (!groups.data) {
    if (groups.error) {
      return (
        <Padded>
          <Empty title="The run groups could not be read" grain="clay">
            {groups.error.message}
          </Empty>
        </Padded>
      );
    }
    return (
      <Padded>
        <Spinner label="Loading the run group" />
      </Padded>
    );
  }
  if (!group) {
    return (
      <Padded>
        <Empty title="This run group is not on the daemon">
          It may have been made on another host. Start one with “Run in parallel…” in the composer.
        </Empty>
      </Padded>
    );
  }

  const tag = STATE_TAG[group.state];
  const running = group.members.some((m) => isLive(m.status));
  const shared = sharedApprovals(
    pendingView(pending),
    group.members.map((m) => m.chatId),
  );
  const resolved = group.state === 'resolved';
  const title =
    group.prompt
      .split('\n')
      .find((l) => l.trim())
      ?.trim() ?? 'Run group';
  // The prompt after its first line, which is the title.
  const prompt = group.prompt.trim();
  const rest = prompt.startsWith(title) ? prompt.slice(title.length).trim() : prompt;
  const memberOptions = group.members.map((m) => (
    <option key={m.chatId} value={m.chatId}>
      {m.label}
    </option>
  ));

  return (
    <div className="mx-auto flex w-full max-w-[1200px] flex-col gap-6 px-6 py-6">
      <header className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2 text-small text-muted">
          <Layers size={14} aria-hidden="true" className="text-accent-fg" />
          <span>Run group</span>
          <span aria-hidden="true">·</span>
          <span>
            {group.members.length} variants from <code className="font-mono">{group.base}</code>
          </span>
          <StatusTag status={tag.status}>{tag.label}</StatusTag>
        </div>
        <h1 className="m-0 text-title font-semibold text-ink">{title}</h1>
        {rest && (
          <p className="m-0 line-clamp-3 whitespace-pre-wrap text-body text-muted">{rest}</p>
        )}
        <div className="flex flex-wrap items-center gap-2 pt-1">
          {group.hasCheck && !resolved && (
            <Button size="sm" variant="secondary" disabled={checking} onClick={runChecks}>
              <FlaskConical size={14} aria-hidden="true" />
              {checking ? 'Starting checks…' : 'Run check on all'}
            </Button>
          )}
          <Button size="sm" variant="secondary" disabled={judging} onClick={startJudge}>
            <Gavel size={14} aria-hidden="true" />
            {judging ? 'Starting the judge…' : 'Start judge thread'}
          </Button>
          {!resolved && (
            <Button size="sm" variant="primary" onClick={() => setResolving(true)}>
              <GitMerge size={14} aria-hidden="true" />
              Resolve…
            </Button>
          )}
        </div>
      </header>

      {notice && (
        <Notice tone={notice.tone} title={notice.text}>
          {notice.url && (
            <a href={notice.url} target="_blank" rel="noreferrer noopener">
              {notice.url}
            </a>
          )}
        </Notice>
      )}
      {group.resolution && (
        <Notice tone="info" title={`Resolved: ${group.resolution.result}`}>
          <a href={`#${studioHref({ kind: 'thread', threadId: group.resolution.winnerChatId })}`}>
            Open {labelOf(group.resolution.winnerChatId)}
          </a>
        </Notice>
      )}

      <SharedApprovals shared={shared} labelOf={labelOf} onApproveAll={approveAll} />

      <Section id="group-variants" title="Variants">
        <GroupGrid
          members={group.members}
          hasCheck={group.hasCheck}
          {...(group.resolution ? { winnerChatId: group.resolution.winnerChatId } : {})}
          onContinue={(chatId) => navigate(studioHref({ kind: 'thread', threadId: chatId }))}
        />
      </Section>

      <Section id="group-compare" title="Compare">
        {matrix.isPending && !matrix.data ? (
          <Spinner tone="muted" label="Comparing the variants" />
        ) : matrix.error ? (
          <p role="alert" className="m-0 text-body text-danger">
            {matrix.error.message}
          </p>
        ) : (
          <DiffMatrix
            rows={matrix.data ?? []}
            members={group.members}
            selected={pair?.path ? { path: pair.path, chatId: pair.to } : null}
            onSelect={({ path, chatId }) =>
              setPicked((current) => {
                const base = current ?? pair ?? { from: '', to: chatId };
                return base.from === chatId
                  ? { from: '', to: chatId, path }
                  : { from: base.from, to: chatId, path };
              })
            }
          />
        )}
        {pair && (
          <div className="flex flex-col gap-3 border-t border-rule-soft pt-3">
            <div className="flex flex-col gap-1.5">
              <span className="text-small text-muted">Side by side</span>
              <div className="grid items-center gap-2 text-body text-muted md:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_minmax(0,1.5fr)]">
                <Select
                  aria-label="Left"
                  value={pair.from}
                  onChange={(e) => setPicked({ ...pair, from: e.target.value })}
                >
                  <option value="">The base ({group.base})</option>
                  {memberOptions}
                </Select>
                <span aria-hidden="true" className="hidden text-center md:block">
                  →
                </span>
                <Select
                  aria-label="Right"
                  value={pair.to}
                  onChange={(e) => setPicked({ ...pair, to: e.target.value })}
                >
                  {memberOptions}
                </Select>
                <Select
                  aria-label="File"
                  value={pair.path ?? ''}
                  onChange={(e) => {
                    const { path: _drop, ...rest } = pair;
                    setPicked(e.target.value ? { ...rest, path: e.target.value } : rest);
                  }}
                >
                  <option value="">All files</option>
                  {(matrix.data ?? []).map((row) => (
                    <option key={row.path} value={row.path}>
                      {row.path}
                    </option>
                  ))}
                </Select>
              </div>
            </div>
            {pair.from === pair.to ? (
              <p className="m-0 text-body text-muted">Pick two different variants.</p>
            ) : pairDiff.isPending ? (
              <Spinner tone="muted" label="Reading the pair’s diff" />
            ) : pairDiff.error ? (
              <p role="alert" className="m-0 text-body text-danger">
                {pairDiff.error.message}
              </p>
            ) : (
              <PairDiff
                patch={pairDiff.data ?? ''}
                from={pair.from ? labelOf(pair.from) : `The base (${group.base})`}
                to={labelOf(pair.to)}
              />
            )}
          </div>
        )}
      </Section>

      {(group.hasCheck || group.members.some((m) => m.check.state !== 'none')) && (
        <Section id="group-checks" title="Check results">
          <CheckResults members={group.members} />
        </Section>
      )}

      {resolving && (
        <ResolveDialog
          open
          onClose={() => setResolving(false)}
          members={group.members}
          base={group.base}
          prompt={group.prompt}
          running={running}
          winner={winner.state}
          onWinnerChange={setWinnerId}
          onConfirm={resolve}
        />
      )}
    </div>
  );
}
