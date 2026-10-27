// The sidebar's data (SD1 Task 17; §53 §5.3): agentd's watches and worktrees, folded into
// @loams/studio-ui's thread rows. Every watch answers the whole list again on each change,
// so the latest frame is the state:
//
//   ThreadService.WatchThreads           the threads (ndjson WatchChats)
//   ThreadService.WatchSessions          each thread's run status
//   ThreadService.WatchSidebarPreferences  the pinned order
//   ApprovalService.WatchApprovals       pending tool calls (supervised mode, Task 13)
//   RunGroupService.WatchRunGroups       run groups (Task 12)
//   GitService.ListWorktrees             each project's worktrees, with their threads
//
// A watch the daemon does not serve yet (`unimplemented`: run groups and approvals until
// Tasks 12 and 13) is simply absent: its rows and badges appear when the feature does. The
// streams go over the transport of the nearest `AgentdScope`, as connect-query does.
//
// The folded rows are also kept in the query cache (`sidebarKey`), so the palette and the
// "new thread" command read the sidebar's placement without opening their own streams.

import { Code, ConnectError, createClient, type Transport } from '@connectrpc/connect';
import { NO_AGENTD, useAgentdTransport } from '@loams/console-host/query';
import {
  type Approval,
  ApprovalService,
  type Chat,
  GitService,
  type RunGroup,
  RunGroupService,
  RunGroupState,
  type Session,
  SessionStatus,
  type SidebarPreferencesState,
  ThreadService,
  type WorktreeInfo,
} from '@loams/proto/agentd';
import {
  type ProjectWorktree,
  placeThread,
  type SidebarRunGroup,
  type SidebarThread,
  type ThreadRunStatus,
} from '@loams/studio-ui';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useState } from 'react';
import type { Project, StudioAgentd } from '../agentd.js';

/** One watch's latest frame, its failure, or that the daemon does not serve it. */
export interface WatchState<T> {
  data: T | undefined;
  error: Error | undefined;
  unsupported: boolean;
}

const RETRY_MS = [1000, 2000, 5000, 15000, 30000];

/**
 * Follows a server stream while `open` is given: each frame replaces the state. A
 * stream that ends or fails is reopened after a backoff; `unimplemented` stops it.
 */
export function useWatch<T>(
  open: ((transport: Transport, signal: AbortSignal) => AsyncIterable<T>) | null,
  key: string,
): WatchState<T> {
  const transport = useAgentdTransport();
  const [state, setState] = useState<WatchState<T>>({
    data: undefined,
    error: undefined,
    unsupported: false,
  });
  // `open` is a new closure each render; `key` names what it opens.
  // oxlint-disable-next-line react/exhaustive-deps -- keyed by `key` and the transport.
  useEffect(() => {
    if (!open || !transport) return;
    const abort = new AbortController();
    let attempt = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const run = async () => {
      try {
        for await (const frame of open(transport, abort.signal)) {
          attempt = 0;
          setState({ data: frame, error: undefined, unsupported: false });
        }
      } catch (e) {
        if (abort.signal.aborted) return;
        const err = ConnectError.from(e);
        if (err.code === Code.Canceled) return;
        if (err.code === Code.Unimplemented) {
          setState({ data: undefined, error: undefined, unsupported: true });
          return;
        }
        setState((s) => ({ ...s, error: err }));
      }
      if (abort.signal.aborted) return;
      const wait = RETRY_MS[Math.min(attempt, RETRY_MS.length - 1)] ?? 30000;
      attempt += 1;
      timer = setTimeout(run, wait);
    };
    void run();
    return () => {
      abort.abort();
      if (timer) clearTimeout(timer);
    };
  }, [key, transport, open === null]);
  return state;
}

const ms = (v: bigint | undefined | null): number | null =>
  v === undefined || v === null ? null : Number(v);

export function runStatusOf(status: SessionStatus | undefined): ThreadRunStatus {
  switch (status) {
    case SessionStatus.WORKING:
      return 'working';
    case SessionStatus.AWAITING_INPUT:
      return 'awaiting-input';
    case SessionStatus.ERRORED:
      return 'errored';
    default:
      return 'idle';
  }
}

/** A thread's title: its own, else its last message's first line, else "New thread". */
export function threadTitle(chat: Pick<Chat, 'title' | 'lastMessagePreview'>): string {
  const own = chat.title?.trim();
  if (own) return own;
  const preview = chat.lastMessagePreview?.split('\n')[0]?.trim();
  return preview ? preview.slice(0, 120) : 'New thread';
}

export function worktreesOf(list: readonly WorktreeInfo[]): ProjectWorktree[] {
  // agentd lists the main worktree first (Task 9, T9-9).
  return list.map((w, index) => ({
    path: w.path,
    branch: w.branch,
    chatIds: w.chatIds,
    dirty: w.dirty,
    main: index === 0,
  }));
}

/** Folds the watches into rows. Pure, for the tests and the palette. */
export function foldThreads(input: {
  chats: readonly Chat[];
  sessions: readonly Session[];
  approvals: readonly Approval[];
  groups: readonly RunGroup[];
  projects: readonly Project[];
  worktrees: ReadonlyMap<string, readonly ProjectWorktree[]>;
}): { threads: SidebarThread[]; groups: SidebarRunGroup[] } {
  const sessions = new Map(input.sessions.map((s) => [s.chatId, s]));
  const approvals = new Map<string, number>();
  for (const a of input.approvals) approvals.set(a.chatId, (approvals.get(a.chatId) ?? 0) + 1);
  const groupOf = new Map<string, string>();
  // A member resolved with `keep` is an ordinary thread again (SD1 T12-2).
  for (const g of input.groups)
    for (const m of g.members) if (!m.detached) groupOf.set(m.chatId, g.id);
  const projects = input.projects.map((p) => ({
    id: p.id,
    name: p.name,
    path: p.path,
    branch: p.branch,
  }));

  const threads = input.chats.map((chat): SidebarThread => {
    const session = sessions.get(chat.id);
    const place = placeThread(
      {
        id: chat.id,
        cwd: chat.cwd ?? chat.sourceContext?.cwd ?? null,
        repoRoot: chat.sourceContext?.repoRoot ?? null,
      },
      projects,
      input.worktrees,
    );
    const created = ms(chat.createdAtMs) ?? 0;
    const lastMessageAt = ms(chat.lastMessageAtMs);
    return {
      id: chat.id,
      title: threadTitle(chat),
      projectId: place.projectId,
      worktreePath: place.worktreePath,
      branch: chat.branch ?? chat.sourceContext?.branch ?? null,
      cwd: chat.cwd ?? null,
      archived: chat.archived,
      createdAt: created,
      updatedAt: lastMessageAt ?? created,
      lastMessageAt,
      lastSeenAt: ms(chat.lastSeenAtMs),
      status: runStatusOf(session?.status),
      statusSince: session ? (ms(session.startedAtMs) ?? ms(session.updatedAtMs)) : null,
      pendingApprovals: approvals.get(chat.id) ?? 0,
      groupId: groupOf.get(chat.id) ?? null,
      parentId: chat.parentChatId ?? null,
    };
  });

  const placed = new Map(threads.map((t) => [t.id, t.projectId]));
  const groups = input.groups.map((g): SidebarRunGroup => ({
    id: g.id,
    title: g.prompt.split('\n')[0]?.trim().slice(0, 120) || 'Run group',
    projectId: g.members.map((m) => placed.get(m.chatId)).find((p) => p != null) ?? null,
    memberIds: g.members.filter((m) => !m.detached).map((m) => m.chatId),
    createdAt: ms(g.createdAtMs) ?? 0,
    state:
      g.state === RunGroupState.RUNNING
        ? 'running'
        : g.state === RunGroupState.READY
          ? 'ready'
          : g.state === RunGroupState.RESOLVED
            ? 'resolved'
            : 'unknown',
  }));
  return { threads, groups };
}

/** What the sidebar shows, and what the palette and "new thread" read back. */
export interface SidebarData {
  /** `loading` until the threads arrive; `unavailable` when the daemon has no thread watch. */
  state: 'loading' | 'ready' | 'error' | 'unavailable';
  error?: string;
  threads: SidebarThread[];
  groups: SidebarRunGroup[];
  /** agentd's pinned order, with any change this page just made. */
  pinnedIds: string[];
  worktrees: ReadonlyMap<string, readonly ProjectWorktree[]>;
}

const EMPTY: SidebarData = {
  state: 'loading',
  threads: [],
  groups: [],
  pinnedIds: [],
  worktrees: new Map(),
};

/** The query-cache key the sidebar publishes its rows under, per host. */
export const sidebarKey = (agentd: StudioAgentd) => ['studio', 'sidebar', agentd.cacheKey()];

/** The sidebar's rows as the sidebar last published them (no stream of its own). */
export function usePublishedSidebar(agentd: StudioAgentd): SidebarData | undefined {
  return useQuery<SidebarData>({
    queryKey: sidebarKey(agentd),
    queryFn: () => EMPTY,
    enabled: false,
    staleTime: Number.POSITIVE_INFINITY,
  }).data;
}

/** The worktrees query's key prefix, for invalidating after a worktree is removed. */
export const worktreesKey = () => ['studio', 'worktrees'];

/**
 * The sidebar's rows, and `applyPins` for a `ChangeSidebarPin` answer: the new pinned
 * order shows at once, until the watch catches up with that revision.
 */
export function useSidebarData(
  agentd: StudioAgentd,
  projects: readonly Project[] | undefined,
): [SidebarData, (prefs: SidebarPreferencesState | undefined) => void] {
  const transport = useAgentdTransport();
  const ready = transport !== undefined;
  const chats = useWatch(
    ready ? (t, signal) => createClient(ThreadService, t).watchThreads({}, { signal }) : null,
    'threads',
  );
  const sessions = useWatch(
    ready ? (t, signal) => createClient(ThreadService, t).watchSessions({}, { signal }) : null,
    'sessions',
  );
  const prefs = useWatch(
    ready
      ? (t, signal) => createClient(ThreadService, t).watchSidebarPreferences({}, { signal })
      : null,
    'sidebar-preferences',
  );
  const approvals = useWatch(
    ready ? (t, signal) => createClient(ApprovalService, t).watchApprovals({}, { signal }) : null,
    'approvals',
  );
  const groups = useWatch(
    ready ? (t, signal) => createClient(RunGroupService, t).watchRunGroups({}, { signal }) : null,
    'run-groups',
  );
  const [override, setOverride] = useState<SidebarPreferencesState | null>(null);
  const watched = prefs.data?.preferences;
  const pinOverride =
    override && !(watched && watched.revision >= override.revision)
      ? override.pinnedSessionIds
      : null;
  // Each project's worktrees, in one query: a repository that is not a git checkout (or
  // a daemon without ListWorktrees) has none.
  const paths = (projects ?? []).map((p) => p.path);
  const worktreeQuery = useQuery({
    queryKey: [...worktreesKey(), agentd.cacheKey(), paths],
    enabled: ready && paths.length > 0,
    retry: false,
    queryFn: async () => {
      const git = createClient(GitService, transport ?? NO_AGENTD);
      return Promise.all(
        paths.map((repoPath) =>
          git.listWorktrees({ repoPath }).then(
            (r) => worktreesOf(r.worktrees),
            () => [] as ProjectWorktree[],
          ),
        ),
      );
    },
  });
  const worktrees = useMemo(() => {
    const map = new Map<string, ProjectWorktree[]>();
    (projects ?? []).forEach((p, i) => {
      const list = worktreeQuery.data?.[i];
      if (list) map.set(p.id, list);
    });
    return map;
  }, [projects, worktreeQuery.data]);

  const data = useMemo((): SidebarData => {
    const pinnedIds = [...(pinOverride ?? prefs.data?.preferences?.pinnedSessionIds ?? [])];
    if (chats.unsupported) return { ...EMPTY, state: 'unavailable', pinnedIds };
    // Threads in linked worktrees are placed by the worktree lists: wait for them, so no
    // thread shows under "Other threads" first and then moves.
    const placing = worktreeQuery.isPending && worktreeQuery.fetchStatus !== 'idle';
    if (!chats.data || placing) {
      return chats.error
        ? { ...EMPTY, state: 'error', error: chats.error.message, pinnedIds }
        : { ...EMPTY, pinnedIds };
    }
    const folded = foldThreads({
      chats: chats.data.chats,
      sessions: sessions.data?.sessions ?? [],
      approvals: approvals.data?.pending ?? [],
      groups: groups.data?.groups ?? [],
      projects: projects ?? [],
      worktrees,
    });
    return { state: 'ready', ...folded, pinnedIds, worktrees };
  }, [
    chats.data,
    chats.error,
    chats.unsupported,
    sessions.data,
    approvals.data,
    groups.data,
    prefs.data,
    pinOverride,
    projects,
    worktrees,
    worktreeQuery.isPending,
    worktreeQuery.fetchStatus,
  ]);

  const client = useQueryClient();
  const key = agentd.cacheKey();
  useEffect(() => {
    client.setQueryData(['studio', 'sidebar', key], data);
  }, [client, data, key]);
  return [data, (next) => setOverride(next ?? null)];
}
