// The thread actions (SD1 Task 17; §53 §5.3; inventory (a): T3 Code's
// hooks/useThreadActions.ts and useThreadActionMenu.ts rewritten, using them as the
// reference): create, pin, reorder, rename, fork, archive, delete with the worktree
// prompt, mark seen, and clean up worktrees, as `loams.agentd.v1` calls over the agentd
// transport. The daemon's watches bring the result back to the sidebar; a pin change
// answers with the new order, which shows at once (`applyPins`).

import { create, type MessageInitShape } from '@bufbuild/protobuf';
import { type Client, Code, ConnectError, createClient } from '@connectrpc/connect';
import { useAgentdTransport } from '@loams/console-host/query';
import {
  ApprovalMode,
  ApprovalService,
  ChatConfigSchema,
  GitService,
  HarnessId,
  SandboxLevel,
  type SidebarPinChangeSchema,
  type SidebarPinPositionSchema,
  type SidebarPreferencesState,
  ThreadService,
} from '@loams/proto/agentd';
import {
  getFallbackThreadIdAfterDelete,
  type PinPosition,
  type SidebarThread,
} from '@loams/studio-ui';
import { toastManager } from '@loams/ui';
import { useQueryClient } from '@tanstack/react-query';
import { useCallback, useMemo, useState } from 'react';
import type { StudioAgentd } from '../agentd.js';
import { useProjects } from '../projects/queries.js';
import { type StudioRoute, studioHref } from '../routes.js';
import { readSettings, type StudioSettings } from '../settings/Settings.js';
import { usePublishedSidebar, worktreesKey } from './data.js';

const HARNESS: Record<StudioSettings['harness'], HarnessId> = {
  'claude-code': HarnessId.CLAUDE_CODE,
  codex: HarnessId.CODEX,
  opencode: HarnessId.OPENCODE,
  'loams-bot': HarnessId.LOAMS_BOT,
};

/** A new thread's id: the client picks it (`CreateThread.chat_id`). */
export function newThreadId(): string {
  return globalThis.crypto.randomUUID();
}

/** A new thread's approval mode: Settings › Studio's choice, `supervised` by default (Q803). */
export function newThreadApprovalMode(): ApprovalMode {
  return readSettings().approvalMode === 'full-access'
    ? ApprovalMode.FULL_ACCESS
    : ApprovalMode.SUPERVISED;
}

/**
 * Records Settings › Studio's approval mode for a new thread `chatId`, before its first
 * run (G22, SD1 Task 13). Every way a thread starts calls it: New thread and Mod+N, the
 * project page's composer, the judge (SD1 Task 30: the composer never did, so on a
 * desktop daemon its threads ran at the daemon's then `full-access` default; since T30-19
 * the default is `supervised`, so `full-access` is only ever this explicit choice). The
 * daemon keys the mode by id, so it can be set before `Send` creates the thread. A daemon without
 * approvals answers `unimplemented`: its threads run as before.
 */
export async function applyNewThreadApprovalMode(
  approvals: Client<typeof ApprovalService>,
  chatId: string,
): Promise<void> {
  try {
    await approvals.setThreadApprovalMode({ chatId, mode: newThreadApprovalMode() });
  } catch (e) {
    if (!(e instanceof ConnectError && e.code === Code.Unimplemented)) throw e;
  }
}

/** This device's defaults for a new thread (Settings › Studio, T15-6). */
export function newThreadConfig() {
  const settings = readSettings();
  return create(ChatConfigSchema, {
    harness: HARNESS[settings.harness],
    ...(settings.model ? { model: settings.model } : {}),
    sandbox: newThreadSandbox(),
  });
}

/** A new thread's sandbox: full access with Settings › Studio's full-access mode, else workspace writes. */
export function newThreadSandbox(): SandboxLevel {
  return readSettings().approvalMode === 'full-access'
    ? SandboxLevel.DANGER_FULL_ACCESS
    : SandboxLevel.WORKSPACE_WRITE;
}

type PinChange = MessageInitShape<typeof SidebarPinChangeSchema>['change'];

/** A worktree to remove with its thread: the repository and the linked worktree. */
export interface WorktreeTarget {
  repoPath: string;
  worktreePath: string;
}

export interface ThreadActions {
  /** The last action's failure, for the sidebar's alert; null after the next success. */
  error: string | null;
  clearError(): void;
  pending: boolean;
  create(project: { id: string; path: string }): Promise<void>;
  pin(id: string, position?: PinPosition): Promise<void>;
  /** Moves a pinned thread (a drag, or Move up and Move down). */
  reorder(id: string, position: PinPosition): Promise<void>;
  unpin(id: string): Promise<void>;
  rename(id: string, title: string): Promise<void>;
  fork(id: string): Promise<void>;
  setArchived(id: string, archived: boolean): Promise<void>;
  /** Deletes the thread, then its worktree when asked; leaves its page if it was open. */
  remove(thread: SidebarThread, worktree?: WorktreeTarget): Promise<void>;
  markSeen(id: string): Promise<void>;
  removeWorktrees(targets: readonly WorktreeTarget[]): Promise<void>;
}

export function useThreadActions(o: {
  navigate: (to: string) => void;
  activeThreadId?: string;
  threads: readonly SidebarThread[];
  applyPins?: (prefs: SidebarPreferencesState | undefined) => void;
}): ThreadActions {
  const transport = useAgentdTransport();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(0);
  const { navigate, activeThreadId, threads, applyPins } = o;

  const clients = useMemo(
    () =>
      transport
        ? {
            threads: createClient(ThreadService, transport),
            git: createClient(GitService, transport),
            approvals: createClient(ApprovalService, transport),
          }
        : null,
    [transport],
  );

  const run = useCallback(
    async (what: string, action: (c: NonNullable<typeof clients>) => Promise<void>) => {
      if (!clients) {
        setError(`Could not ${what}: no loams-agentd is reachable`);
        return;
      }
      setPending((n) => n + 1);
      try {
        await action(clients);
        setError(null);
      } catch (e) {
        setError(`Could not ${what}: ${e instanceof Error ? e.message : String(e)}`);
      } finally {
        setPending((n) => n - 1);
      }
    },
    [clients],
  );

  const changePin = useCallback(
    (what: string, change: PinChange) =>
      run(what, async (c) => {
        const res = await c.threads.changeSidebarPin({ change: { change } });
        applyPins?.(res.preferences);
      }),
    [run, applyPins],
  );

  const position = (
    sessionId: string,
    p: PinPosition = {},
  ): MessageInitShape<typeof SidebarPinPositionSchema> => ({
    sessionId,
    ...(p.after !== undefined ? { after: p.after } : {}),
    ...(p.before !== undefined ? { before: p.before } : {}),
  });

  return {
    error,
    clearError: () => setError(null),
    pending: pending > 0,
    create: (project) =>
      run('start a thread', async (c) => {
        const id = newThreadId();
        await c.threads.createThread({ chatId: id, cwd: project.path, config: newThreadConfig() });
        await applyNewThreadApprovalMode(c.approvals, id);
        navigate(studioHref({ kind: 'thread', threadId: id }));
      }),
    pin: (id, p) => changePin('pin the thread', { case: 'pin', value: position(id, p) }),
    reorder: (id, p) => changePin('move the thread', { case: 'reorder', value: position(id, p) }),
    unpin: (id) => changePin('unpin the thread', { case: 'unpin', value: { sessionId: id } }),
    rename: (id, title) =>
      run('rename the thread', async (c) => {
        await c.threads.renameThread({ chatId: id, title });
      }),
    fork: (id) =>
      run('fork the thread', async (c) => {
        const forkId = newThreadId();
        const res = await c.threads.forkThread({ chatId: forkId, sourceChatId: id });
        navigate(studioHref({ kind: 'thread', threadId: res.chat?.id ?? forkId }));
      }),
    setArchived: (id, archived) =>
      run(archived ? 'archive the thread' : 'unarchive the thread', async (c) => {
        await c.threads.setThreadArchived({ chatId: id, archived });
        const title = threads.find((t) => t.id === id)?.title || 'Untitled thread';
        toastManager.add({
          type: 'success',
          title: archived ? 'Thread archived' : 'Thread unarchived',
          description: title,
          ...(archived
            ? {
                actionProps: {
                  children: 'Undo',
                  onClick: () =>
                    void c.threads
                      .setThreadArchived({ chatId: id, archived: false })
                      .catch((e: unknown) =>
                        setError(
                          `Could not unarchive the thread: ${e instanceof Error ? e.message : String(e)}`,
                        ),
                      ),
                },
              }
            : {}),
        });
      }),
    remove: (thread, worktree) =>
      run('delete the thread', async (c) => {
        const fallback =
          thread.id === activeThreadId
            ? getFallbackThreadIdAfterDelete({
                threads: threads.filter((t) => !t.archived),
                deletedThreadId: thread.id,
                sortOrder: 'updated_at',
              })
            : null;
        await c.threads.deleteThread({ chatId: thread.id });
        if (worktree) {
          await c.git.deleteWorktree(worktree);
          await queryClient.invalidateQueries({ queryKey: worktreesKey() });
        }
        toastManager.add({
          type: 'success',
          title: worktree ? 'Thread and worktree deleted' : 'Thread deleted',
          description: thread.title || 'Untitled thread',
        });
        if (thread.id === activeThreadId) {
          navigate(
            fallback
              ? studioHref({ kind: 'thread', threadId: fallback })
              : thread.projectId
                ? studioHref({ kind: 'project', projectId: thread.projectId })
                : '/studio',
          );
        }
      }),
    markSeen: (id) =>
      run('mark the thread seen', async (c) => {
        await c.threads.markThreadSeen({ chatId: id });
      }),
    removeWorktrees: (targets) =>
      run('remove the worktrees', async (c) => {
        try {
          for (const target of targets) await c.git.deleteWorktree(target);
          toastManager.add({
            type: 'success',
            title:
              targets.length === 1 ? 'Removed 1 worktree' : `Removed ${targets.length} worktrees`,
          });
        } finally {
          await queryClient.invalidateQueries({ queryKey: worktreesKey() });
        }
      }),
  };
}

/**
 * `chat.new` (Mod+N): a thread in the project in view (the route's project, or the open
 * thread's), else the home page to pick one. The open thread's project comes from the
 * sidebar's published rows.
 */
export function useNewThread(
  agentd: StudioAgentd,
  route: StudioRoute,
  navigate: (to: string) => void,
): () => void {
  const projects = useProjects(agentd);
  const sidebar = usePublishedSidebar(agentd);
  const actions = useThreadActions({ navigate, threads: sidebar?.threads ?? [] });
  const projectId =
    route.kind === 'project'
      ? route.projectId
      : route.kind === 'thread'
        ? (sidebar?.threads.find((t) => t.id === route.threadId)?.projectId ?? undefined)
        : undefined;
  const project = projects.data?.find((p) => p.id === projectId);
  return () => {
    if (project) void actions.create(project);
    else navigate('/studio');
  };
}
