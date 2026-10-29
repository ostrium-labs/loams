// The git toolbar's server state (SD1 Task 23; §53 §5.7, §6.5 G1–G5): reads through
// connect-query over the agentd transport, two streams through the transport's client,
// and the writes. Every write carries what the person confirmed: the diff checksum
// (CommitWorktree, DiscardPaths, T9-1's `stale_diff`), the source head (MergeBranch),
// and a push is `--force-with-lease` against the tracking ref or nothing (T9-2).
//
// loams-agentd reports no upstream flag (ListWorktrees' ahead and behind count against
// `@{upstream}` and are 0 without one), so a branch counts as published when it is
// ahead or behind, or has a pull request; otherwise "ahead" is its lead on the default
// branch (ListGitHistory's comparison), and a push sets the upstream (T23-2).

import { createClient, type Transport } from '@connectrpc/connect';
import { createConnectQueryKey, skipToken } from '@connectrpc/connect-query';
import { useAgentdQuery, useAgentdTransport } from '@loams/console-host/query';
import {
  ChangeRequestProvider,
  ChangeRequestState,
  type Chat,
  type CheckoutChangeRequestStatus,
  DiffService,
  GitService,
  MergeStrategy,
  ProjectService,
  ThreadService,
  type WorktreeInfo,
} from '@loams/proto/agentd';
import type {
  ChangeRequestProviderKind,
  GitActionOutcome,
  GitChangedFile,
  GitChangeRequest,
  GitMergeInput,
  GitRunPlan,
  GitStatusView,
  GitWriteStep,
  MergeStrategyName,
} from '@loams/studio-ui';
import { type QueryClient, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { follow } from '../thread/diff-source.js';
import { gitErrorOf } from './errors.js';

/** The latest message of a server stream while `input` is set; restarted when its key changes. */
function useStream<T>(
  open: ((transport: Transport, signal: AbortSignal) => AsyncIterable<T>) | null,
  key: string,
): T | undefined {
  const transport = useAgentdTransport();
  const [latest, setLatest] = useState<T | undefined>(undefined);
  const openRef = useRef(open);
  openRef.current = open;
  const enabled = open !== null;
  // oxlint-disable-next-line react/exhaustive-deps -- `key` names the stream.
  useEffect(() => {
    setLatest(undefined);
    const start = openRef.current;
    if (!transport || !enabled || !start) return;
    const abort = new AbortController();
    void (async () => {
      try {
        for await (const message of start(transport, abort.signal)) {
          if (abort.signal.aborted) return;
          setLatest(message);
        }
      } catch {
        // A dropped stream leaves the last answer; the toolbar still works without it.
      }
    })();
    return () => abort.abort();
  }, [transport, enabled, key]);
  return latest;
}

/** A thread's chat (its checkout, branch and project), from WatchThreads. */
export function useThreadChat(threadId: string): Chat | undefined {
  const response = useStream(
    threadId
      ? (transport, signal) => createClient(ThreadService, transport).watchThreads({}, { signal })
      : null,
    `threads:${threadId}`,
  );
  return response?.chats.find((c) => c.id === threadId);
}

const PROVIDERS: Partial<Record<string, ChangeRequestProviderKind>> = {
  forgejo: 'forgejo',
  github: 'github',
};

function changeRequestOf(status: CheckoutChangeRequestStatus | undefined): GitChangeRequest | null {
  const cr = status?.changeRequest;
  if (!cr || cr.number === 0n) return null;
  const state =
    cr.state === ChangeRequestState.OPEN
      ? 'open'
      : cr.state === ChangeRequestState.MERGED
        ? 'merged'
        : 'closed';
  return {
    number: Number(cr.number),
    title: cr.title,
    url: cr.url,
    state,
    provider: PROVIDERS[cr.provider.toLowerCase()] ?? null,
  };
}

/** The checkout's pull request (WatchChangeRequest). */
export function useChangeRequest(cwd: string, branch: string | null): GitChangeRequest | null {
  const response = useStream(
    cwd && branch
      ? (transport, signal) =>
          createClient(GitService, transport).watchChangeRequest({ cwd, branch }, { signal })
      : null,
    `cr:${cwd}:${branch}`,
  );
  return changeRequestOf(response?.status);
}

export interface CheckoutGit {
  /** The checkout's worktree row (ListWorktrees), when loaded. */
  worktree: WorktreeInfo | undefined;
  worktrees: WorktreeInfo[];
  status: GitStatusView | null;
  files: GitChangedFile[];
  /** The checksum of the diff `files` came from: what a commit or discard confirms. */
  checksum: string | undefined;
  checkoutId: string | undefined;
  defaultBranch: string | null;
  loading: boolean;
}

/** Everything the toolbar shows about the checkout at `cwd` of the repository at `repoPath`. */
export function useCheckoutGit(input: {
  repoPath: string;
  cwd: string;
  defaultBranch?: string | null;
}): CheckoutGit {
  const { repoPath, cwd } = input;
  const worktrees = useAgentdQuery(
    GitService.method.listWorktrees,
    repoPath ? { repoPath } : skipToken,
  );
  const branches = useAgentdQuery(
    GitService.method.listBranches,
    repoPath && !input.defaultBranch ? { repoPath } : skipToken,
  );
  const history = useAgentdQuery(
    GitService.method.listGitHistory,
    cwd ? { cwd, limit: 1n } : skipToken,
  );
  const diff = useAgentdQuery(
    DiffService.method.getCheckoutDiff,
    cwd ? { cwd, mode: '' } : skipToken,
  );
  // The checkout changes under the toolbar (the agent's turns, a restore, the diff
  // panel's discards): follow the daemon's diff sync and re-read what the toolbar
  // shows when this checkout's checksum moves, so a confirm carries the checksum the
  // daemon has (SD1 Task 29 met `stale_diff` and a "clean" worktree otherwise).
  const transport = useAgentdTransport();
  const queryClient = useQueryClient();
  useEffect(() => {
    if (!cwd || !transport) return;
    const diffs = createClient(DiffService, transport);
    let last: string | undefined;
    return follow(
      (signal) => diffs.watchCheckoutDiffs({}, { signal }),
      ({ diffs: frame }) => {
        const mine = frame.find((d) => d.cwd === cwd);
        if (!mine || mine.checksum === last) return;
        last = mine.checksum;
        void invalidateGit(queryClient);
      },
    );
  }, [cwd, transport, queryClient]);
  const rows = worktrees.data?.worktrees ?? [];
  const worktree = rows.find((w) => w.path === cwd);
  const refName = worktree && worktree.branch !== 'HEAD' ? worktree.branch : null;
  const pr = useChangeRequest(cwd, refName);
  const defaultBranch = input.defaultBranch ?? branches.data?.branches[0] ?? null;
  const d = diff.data?.diff;
  const files: GitChangedFile[] = (d?.files ?? []).map((f) => ({
    path: f.path,
    insertions: f.additions,
    deletions: f.deletions,
  }));
  let status: GitStatusView | null = null;
  if (worktree) {
    const ahead = worktree.ahead;
    const behind = worktree.behind;
    const aheadOfDefault = history.data?.page?.comparison
      ? Number(history.data.page.comparison.ahead)
      : undefined;
    const hasUpstream = ahead > 0 || behind > 0 || pr !== null;
    status = {
      refName,
      isDefaultRef: refName !== null && refName === defaultBranch,
      hasWorkingTreeChanges: worktree.dirty || files.length > 0,
      hasUpstream,
      aheadCount: hasUpstream ? ahead : (aheadOfDefault ?? 0),
      behindCount: behind,
      ...(aheadOfDefault !== undefined ? { aheadOfDefaultCount: aheadOfDefault } : {}),
      pr,
      provider: pr?.provider ?? null,
    };
  }
  return {
    worktree,
    worktrees: rows,
    status,
    files,
    checksum: d?.checksum,
    checkoutId: d?.checkoutId || worktree?.checkoutId,
    defaultBranch,
    loading: worktrees.isPending,
  };
}

/** Refetches what a write changes. */
export function invalidateGit(client: QueryClient): Promise<void> {
  const keys = [
    createConnectQueryKey({ schema: GitService.method.listWorktrees, cardinality: 'finite' }),
    createConnectQueryKey({ schema: GitService.method.listRefs, cardinality: 'finite' }),
    createConnectQueryKey({ schema: GitService.method.listBranches, cardinality: 'finite' }),
    createConnectQueryKey({ schema: GitService.method.listGitHistory, cardinality: 'finite' }),
    createConnectQueryKey({ schema: DiffService.method.getCheckoutDiff, cardinality: 'finite' }),
  ].map((queryKey) => client.invalidateQueries({ queryKey }));
  return Promise.all(keys).then(() => undefined);
}

const STRATEGY: Record<MergeStrategyName, MergeStrategy> = {
  merge: MergeStrategy.MERGE,
  squash: MergeStrategy.SQUASH,
  rebase: MergeStrategy.REBASE,
  ff: MergeStrategy.FAST_FORWARD,
};

const PROVIDER: Record<ChangeRequestProviderKind, ChangeRequestProvider> = {
  forgejo: ChangeRequestProvider.FORGEJO,
  github: ChangeRequestProvider.GITHUB,
};

const short = (sha: string) => sha.slice(0, 8);

/**
 * The toolbar's writes for one checkout. Each returns an outcome for the dialog: a
 * failure keeps it open with the reason (and, for a stack, the steps already done).
 */
export function useGitWrites(input: { repoPath: string; cwd: string; git: CheckoutGit }) {
  const transport = useAgentdTransport();
  const client = useQueryClient();
  // The checksum and head the person saw when they confirmed: the latest render's.
  const seen = useRef(input.git);
  seen.current = input.git;
  const { repoPath, cwd } = input;

  const clients = () => {
    if (!transport) throw new Error('No loams-agentd is reachable.');
    return {
      git: createClient(GitService, transport),
      diff: createClient(DiffService, transport),
    };
  };

  const settle = async <T>(
    run: () => Promise<T>,
    ok: (value: T) => Extract<GitActionOutcome, { ok: true }>,
  ): Promise<GitActionOutcome> => {
    try {
      const value = await run();
      await invalidateGit(client);
      return ok(value);
    } catch (e) {
      const error = gitErrorOf(e);
      // A stale confirmation refetches what changed, so the dialog shows it.
      if (error.reason === 'stale_diff') await invalidateGit(client);
      return { ok: false, error };
    }
  };

  async function run(plan: GitRunPlan): Promise<GitActionOutcome> {
    const done: GitWriteStep[] = [];
    const notes: string[] = [];
    let url: string | undefined;
    const branch = seen.current.status?.refName ?? '';
    try {
      const { git } = clients();
      for (const step of plan.steps) {
        if (step === 'commit' && plan.commit) {
          const at = seen.current;
          const reply = await git.commitWorktree({
            checkoutId: at.checkoutId ?? '',
            cwd,
            message: plan.commit.message,
            ...(plan.commit.paths ? { paths: plan.commit.paths } : { all: true }),
            signoff: plan.commit.signoff,
            expectedChecksum: at.checksum ?? '',
          });
          notes.push(`Committed ${short(reply.sha)}`);
        } else if (step === 'push') {
          const reply = await git.pushBranch({
            cwd,
            branch,
            setUpstream: true,
            forceWithLease: plan.push?.forceWithLease ?? false,
          });
          notes.push(`Pushed to ${reply.remote}`);
        } else if (step === 'create_pr' && plan.pr) {
          const reply = await git.createChangeRequest({
            cwd,
            provider: PROVIDER[plan.pr.provider],
            base: plan.pr.base,
            head: branch,
            title: plan.pr.title,
            body: plan.pr.body,
            draft: plan.pr.draft,
            ...(plan.pr.linkIssue ? { linkIssue: plan.pr.linkIssue } : {}),
          });
          notes.push(`Opened PR #${reply.number}`);
          url = reply.url;
        }
        done.push(step);
      }
      await invalidateGit(client);
      return {
        ok: true,
        title: notes.join(', ') || 'Done',
        ...(url ? { description: url, url } : {}),
      };
    } catch (e) {
      const error = gitErrorOf(e);
      await invalidateGit(client);
      return {
        ok: false,
        error: done.length
          ? { ...error, message: `${notes.join(', ')}. Then: ${error.message}` }
          : error,
        ...(done.length ? { completed: done } : {}),
      };
    }
  }

  return {
    run,
    merge: (m: GitMergeInput) =>
      settle(
        () =>
          clients().git.mergeBranch({
            repoPath,
            source: seen.current.status?.refName ?? '',
            target: m.target,
            strategy: STRATEGY[m.strategy],
            ...(m.message ? { message: m.message } : {}),
            deleteSource: m.deleteSource,
            removeWorktree: m.removeWorktree,
            expectedSourceHead: seen.current.worktree?.head ?? '',
          }),
        (r) => ({ ok: true, title: `Merged into ${m.target} (${short(r.sha)})` }),
      ),
    discard: (paths: string[]) =>
      settle(
        () =>
          clients().diff.discardPaths({
            checkoutId: seen.current.checkoutId ?? '',
            paths,
            expectedChecksum: seen.current.checksum ?? '',
          }),
        () => ({
          ok: true,
          title: `Discarded ${paths.length === 1 ? paths[0] : `${paths.length} files`}`,
        }),
      ),
    fetch: () =>
      settle(
        () => clients().git.fetchAll({ repoPath }),
        () => ({ ok: true, title: 'Fetched' }),
      ),
    switchRef: (refName: string) =>
      settle(
        () => clients().git.switchRef({ repoPath: cwd, refName }),
        (r) => ({ ok: true, title: `Switched to ${r.branch}` }),
      ),
  };
}

/** The repository's refs for the picker (ListRefs), with the default branch marked. */
export function useBranchRefs(repoPath: string, defaultBranch: string | null) {
  const refs = useAgentdQuery(GitService.method.listRefs, repoPath ? { repoPath } : skipToken);
  return {
    loading: refs.isPending,
    refs: (refs.data?.refs ?? []).map((r) => ({
      name: r.name,
      current: r.current,
      isDefault: r.name === defaultBranch,
      worktreePath: r.worktreePath ?? null,
    })),
  };
}

/** A space's project actions (ListProjectActions); none without a space. */
export function useProjectActions(spaceId: string | undefined) {
  return useAgentdQuery(
    ProjectService.method.listProjectActions,
    spaceId ? { spaceId } : skipToken,
  );
}
