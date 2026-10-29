// The thread page (SD1 Task 18; §53 §5.4). Written new, with T3 Code's ChatView.tsx
// (apps/web/src/components/ChatView.tsx at 6524a8d4, MIT) as the reference for what the
// page wires together: the header, the timeline, find, inline questions and
// approvals, and per-turn restore. The data is the daemon's:
// - the transcript through the shared store (`ThreadService.WatchTranscript`, folded by
//   `@loams/agentd-client`), so the dock shows the same thread from the same watch;
// - the thread's record from `WatchThreads`; checkpoints (`ListCheckpoints`) and usage
//   (`GetThreadUsage`) through connect-query;
// - answers (`AnswerQuestion`), interrupts (`Interrupt`), approvals (`WatchApprovals`,
//   `ResolveApproval`; G17, G18) and restores (`GetCheckoutDiff` for the checksum the
//   person confirmed against, then `RestoreCheckpoint`).

import { createClient } from '@connectrpc/connect';
import { useAgentdMutation, useAgentdQuery, useAgentdTransport } from '@loams/console-host/query';
import {
  ApprovalService,
  CheckpointScope,
  DiffService,
  HarnessId,
  ApprovalDecision as ProtoDecision,
  SandboxLevel,
  ThreadService,
} from '@loams/proto/agentd';
import type { ApprovalDecision } from '@loams/studio-ui/approvals';
import {
  findTimelineMatches,
  isThreadRunning,
  type PendingApproval,
  ThreadFindBar,
  ThreadHeader,
  Timeline,
  useThreadTranscript,
  useTimelineRows,
} from '@loams/studio-ui/chat';
import { Button, Empty, Notice, Spinner } from '@loams/ui';
import { type KeyboardEvent, useCallback, useEffect, useMemo, useState } from 'react';
import { MARKETPLACE_HARNESSES, studioHref } from '../routes.js';
import { readSettings } from '../settings/Settings.js';
import { ThreadComposer } from './Composer.js';
import { useThreadApprovals, useThreadRecord, useTranscriptStore } from './streams.js';

const HARNESS_LABEL: Partial<Record<HarnessId, string>> = {
  [HarnessId.CLAUDE_CODE]: 'Claude Code',
  [HarnessId.CODEX]: 'Codex',
  [HarnessId.DEVIN]: 'Devin',
  [HarnessId.GROK]: 'Grok',
  [HarnessId.HERMES]: 'Hermes',
  [HarnessId.PI]: 'Pi',
  [HarnessId.OPENCODE]: 'OpenCode',
  [HarnessId.ANTIGRAVITY]: 'Antigravity',
  [HarnessId.LOAMS_BOT]: 'Loams Bot',
  [HarnessId.MOCK]: 'Mock',
};

const SANDBOX_LABEL: Partial<Record<SandboxLevel, string>> = {
  [SandboxLevel.READ_ONLY]: 'read-only',
  [SandboxLevel.WORKSPACE_WRITE]: 'workspace write',
  [SandboxLevel.DANGER_FULL_ACCESS]: 'full access',
};

const DECISION: Record<ApprovalDecision, ProtoDecision> = {
  once: ProtoDecision.ONCE,
  always: ProtoDecision.ALWAYS,
  deny: ProtoDecision.DENY,
};

const threadHref = (threadId: string, query: Record<string, string>) =>
  `/studio/t/${encodeURIComponent(threadId)}?${new URLSearchParams(query)}`;

export interface ThreadPageProps {
  threadId: string;
  navigate: (to: string) => void;
  /** Opens an http(s) link outside the console (the platform's opener). */
  openExternal?: (url: string) => void;
  /**
   * Desktop: opens the thread's checkout in the person's editor (SD1 Task 28, Q810).
   * Electron main builds the editor's URL or argv; absent on the web.
   */
  openInEditor?: OpenInEditor;
}

export type OpenInEditor = (
  path: string,
) => Promise<{ ok: true } | { ok: false; code: string; message: string }>;

export function ThreadPage({ threadId, navigate, openExternal, openInEditor }: ThreadPageProps) {
  const transport = useAgentdTransport();
  const store = useTranscriptStore(transport);
  if (!store) {
    return (
      <div className="p-6">
        <Spinner label="Connecting to the daemon" />
      </div>
    );
  }
  return (
    <ThreadView
      key={threadId}
      threadId={threadId}
      store={store}
      navigate={navigate}
      {...(openExternal ? { openExternal } : {})}
      {...(openInEditor ? { openInEditor } : {})}
    />
  );
}

function ThreadView({
  threadId,
  store,
  navigate,
  openExternal,
  openInEditor,
}: ThreadPageProps & { store: NonNullable<ReturnType<typeof useTranscriptStore>> }) {
  const transport = useAgentdTransport();
  const transcript = useThreadTranscript(store, threadId);
  const record = useThreadRecord(transport, threadId);
  const approvals = useThreadApprovals(transport, threadId);
  const running = isThreadRunning(transcript.entries);

  const checkpointsQuery = useAgentdQuery(
    ThreadService.method.listCheckpoints,
    { chatId: threadId },
    { retry: false },
  );
  const usageQuery = useAgentdQuery(
    ThreadService.method.getThreadUsage,
    { chatId: threadId },
    { retry: false },
  );
  // A finished turn may have a new checkpoint and new usage.
  const { refetch: refetchCheckpoints } = checkpointsQuery;
  const { refetch: refetchUsage } = usageQuery;
  // A turn short enough never to show as running between two renders (a queued run,
  // such as a review sent to the agent) still adds entries, so the entry count is a
  // trigger too; and the daemon records a turn's checkpoint as the turn ends, which can
  // be just after the transcript says it is done, so the list is read again shortly
  // after (SD1 Task 29 saw a turn's restore button missing).
  const entryCount = transcript.entries.length;
  useEffect(() => {
    if (running || entryCount < 0) return;
    void refetchCheckpoints();
    void refetchUsage();
    const again = setTimeout(() => {
      void refetchCheckpoints();
      void refetchUsage();
    }, 1_500);
    return () => clearTimeout(again);
  }, [running, entryCount, refetchCheckpoints, refetchUsage]);

  const checkpoints = useMemo(
    () => new Set((checkpointsQuery.data?.checkpoints ?? []).map((c) => c.turnId)),
    [checkpointsQuery.data],
  );

  const answer = useAgentdMutation(ThreadService.method.answerQuestion);
  const interrupt = useAgentdMutation(ThreadService.method.interrupt);
  const restore = useAgentdMutation(ThreadService.method.restoreCheckpoint);
  const [notice, setNotice] = useState<string | null>(null);

  const onAnswer = useCallback(
    (requestId: string, answers: { questionId: string; labels: string[] }[]) =>
      answer.mutateAsync({ chatId: threadId, requestId, answers }),
    [answer, threadId],
  );

  const onResolveApproval = useCallback(
    async (approval: PendingApproval, decision: ApprovalDecision) => {
      if (!transport) throw new Error('No daemon is reachable');
      // "Always" is scoped to this thread: the daemon remembers it for `chatId` only.
      await createClient(ApprovalService, transport).resolveApproval({
        callId: approval.id,
        chatId: threadId,
        decision: DECISION[decision],
      });
    },
    [transport, threadId],
  );

  const cwd = record.chat?.cwd ?? record.chat?.sourceContext?.cwd;
  const onRestoreTurn = useCallback(
    async (turnId: string) => {
      if (!transport) throw new Error('No daemon is reachable');
      if (!cwd) throw new Error('This thread has no checkout to restore');
      setNotice(null);
      try {
        const { diff } = await createClient(DiffService, transport).getCheckoutDiff({
          cwd,
          mode: '',
          chatId: threadId,
        });
        const result = await restore.mutateAsync({
          chatId: threadId,
          turnId,
          expectedChecksum: diff?.checksum ?? '',
          scope: CheckpointScope.WORKTREE,
        });
        setNotice(`Restored the checkout to ${result.restoredTo.slice(0, 12)}.`);
        void refetchCheckpoints();
      } catch (cause) {
        setNotice(`Restore failed: ${cause instanceof Error ? cause.message : String(cause)}`);
        throw cause;
      }
    },
    [transport, cwd, threadId, restore, refetchCheckpoints],
  );

  const onOpenInEditor = useMemo(
    () =>
      openInEditor && cwd
        ? () => {
            setNotice(null);
            void openInEditor(cwd).then(
              (r) => {
                if (!r.ok) setNotice(`Open in editor failed: ${r.message}`);
              },
              (cause: unknown) =>
                setNotice(
                  `Open in editor failed: ${cause instanceof Error ? cause.message : String(cause)}`,
                ),
            );
          }
        : undefined,
    [openInEditor, cwd],
  );

  // Find in thread.
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState('');
  const [findIndex, setFindIndex] = useState(0);
  const rows = useTimelineRows(transcript.entries, checkpoints);
  const matches = useMemo(
    () => (findOpen ? findTimelineMatches(rows, findQuery) : []),
    [findOpen, rows, findQuery],
  );
  const activeMatchId =
    matches.length > 0
      ? (matches[((findIndex % matches.length) + matches.length) % matches.length] ?? null)
      : null;
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if ((event.metaKey || event.ctrlKey) && !event.altKey && event.key.toLowerCase() === 'f') {
      event.preventDefault();
      setFindOpen(true);
    }
  };

  const chat = record.chat;
  const config = chat?.config;
  const usage = usageQuery.data?.totals;
  const title = chat?.title ?? (record.loaded ? 'Untitled thread' : 'Thread');
  const settings = useMemo(() => readSettings(), []);

  const empty =
    transcript.status === 'connecting' ? (
      <div className="flex justify-center p-8">
        <Spinner label="Loading the thread" />
      </div>
    ) : transcript.status === 'error' ? (
      <div className="p-6">
        <Empty title="This thread could not be loaded">{transcript.error}</Empty>
      </div>
    ) : (
      <div className="p-6">
        <Empty title="No messages yet" seed={3}>
          Messages, tool calls and questions appear here as the agent works.
        </Empty>
      </div>
    );

  return (
    // oxlint-disable-next-line jsx-a11y/no-static-element-interactions -- Mod+F opens find while focus is in the thread
    <div
      className="relative flex h-full min-h-0 flex-col"
      onKeyDown={onKeyDown}
      data-thread-page={threadId}
    >
      <ThreadHeader
        title={title}
        harness={config ? HARNESS_LABEL[config.harness] : undefined}
        model={config?.model}
        approvalMode={approvals.mode ?? settings.approvalMode}
        sandbox={config ? SANDBOX_LABEL[config.sandbox] : undefined}
        branch={chat?.branch ?? chat?.sourceContext?.branch}
        usage={
          usage
            ? {
                inputTokens:
                  usage.inputTokens === undefined ? undefined : Number(usage.inputTokens),
                outputTokens:
                  usage.outputTokens === undefined ? undefined : Number(usage.outputTokens),
                cacheTokens:
                  usage.cacheTokens === undefined ? undefined : Number(usage.cacheTokens),
              }
            : undefined
        }
        running={running}
        onInterrupt={() => interrupt.mutate({ chatId: threadId })}
        onFind={() => setFindOpen(true)}
        onOpenInEditor={onOpenInEditor}
      />
      {findOpen ? (
        <div className="absolute top-14 right-4 z-10">
          <ThreadFindBar
            query={findQuery}
            matchCount={matches.length}
            activeIndex={findIndex}
            onQueryChange={(q) => {
              setFindQuery(q);
              setFindIndex(0);
            }}
            onNext={() => setFindIndex((i) => i + 1)}
            onPrevious={() => setFindIndex((i) => i - 1)}
            onClose={() => {
              setFindOpen(false);
              setFindQuery('');
            }}
          />
        </div>
      ) : null}
      {transcript.status === 'error' && transcript.entries.length > 0 ? (
        <div className="px-4 pt-2">
          <Notice tone="warn" title="Reconnecting to the daemon">
            {transcript.error}
          </Notice>
        </div>
      ) : null}
      {approvals.error ? (
        <div className="px-4 pt-2">
          <Notice tone="warn" title="Approvals are unavailable">
            {approvals.error}
          </Notice>
        </div>
      ) : null}
      {notice ? (
        <div className="flex items-center gap-2 px-4 pt-2">
          <Notice tone={/ failed: /.test(notice) ? 'danger' : 'info'} title={notice} />
          <Button size="sm" variant="quiet" onClick={() => setNotice(null)}>
            Dismiss
          </Button>
        </div>
      ) : null}
      <Timeline
        entries={transcript.entries}
        checkpoints={checkpoints}
        approvals={approvals.pending}
        historyPending={transcript.historyPending}
        threadKey={threadId}
        onAnswer={onAnswer}
        onResolveApproval={onResolveApproval}
        {...(cwd ? { onRestoreTurn } : {})}
        onViewTurnDiff={(turnId) => navigate(threadHref(threadId, { panel: 'diff', turn: turnId }))}
        onOpenFile={(path) => navigate(threadHref(threadId, { panel: 'files', path }))}
        {...(openExternal ? { onOpenLink: openExternal } : {})}
        findMatches={matches}
        activeMatchId={activeMatchId}
        empty={empty}
      />
      {/* The composer (SD1 Task 19). */}
      <div className="shrink-0 border-rule-soft border-t px-4 py-3">
        <ThreadComposer
          threadId={threadId}
          openFile={(path) => navigate(threadHref(threadId, { panel: 'files', path }))}
          onGroupStarted={(groupId) => navigate(studioHref({ kind: 'group', groupId }))}
          onAddHarness={() => navigate(MARKETPLACE_HARNESSES)}
        />
      </div>
    </div>
  );
}
