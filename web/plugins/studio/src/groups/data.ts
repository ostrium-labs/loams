// A run group from `loams.agentd.v1` as @loams/studio-ui's group view (SD1 Task 25; Task 12's
// RunGroupService, T12-5): the wire's enums become the view's names, a member's last message
// comes from its thread (`WatchThreads`), and its waiting approvals from `WatchApprovals`.
// Resolve requests are built here, from what the person confirmed in the dialog.

import type { MessageInitShape } from '@bufbuild/protobuf';
import { toJson } from '@bufbuild/protobuf';
import { ValueSchema } from '@bufbuild/protobuf/wkt';
import {
  type Approval,
  ChangeRequestProvider,
  type Chat,
  CheckState,
  HarnessId,
  MergeStrategy,
  ResolveAction,
  type ResolveRunGroupRequestSchema,
  type RunGroup,
  type RunGroupMember,
  RunGroupState,
  RunMemberStatus,
} from '@loams/proto/agentd';
import type {
  CheckStatus,
  GroupMemberView,
  GroupView,
  MemberStatus,
  PendingApprovalView,
  ResolveChoice,
  ResolveError,
} from '@loams/studio-ui/groups';
import { gitErrorOf } from '../git/errors.js';

export const HARNESS_LABEL: Partial<Record<HarnessId, string>> = {
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

/** A variant's name: its harness, and its model when one was chosen. */
export function memberLabel(m: Pick<RunGroupMember, 'harness' | 'model'>): string {
  const harness = HARNESS_LABEL[m.harness] ?? 'Harness';
  return m.model ? `${harness} · ${m.model}` : harness;
}

const STATUS: Record<RunMemberStatus, MemberStatus> = {
  [RunMemberStatus.UNSPECIFIED]: 'queued',
  [RunMemberStatus.QUEUED]: 'queued',
  [RunMemberStatus.RUNNING]: 'running',
  [RunMemberStatus.AWAITING_INPUT]: 'awaiting-input',
  [RunMemberStatus.DONE]: 'done',
  [RunMemberStatus.ERRORED]: 'errored',
  [RunMemberStatus.CANCELLED]: 'cancelled',
};

const CHECK: Record<CheckState, CheckStatus> = {
  [CheckState.UNSPECIFIED]: 'none',
  [CheckState.PENDING]: 'pending',
  [CheckState.RUNNING]: 'running',
  [CheckState.PASSED]: 'passed',
  [CheckState.FAILED]: 'failed',
};

const ACTION_NAME: Record<ResolveAction, 'merge' | 'pr' | 'keep'> = {
  [ResolveAction.UNSPECIFIED]: 'keep',
  [ResolveAction.MERGE]: 'merge',
  [ResolveAction.CHANGE_REQUEST]: 'pr',
  [ResolveAction.KEEP]: 'keep',
};

/** One line of a thread's last message, for the grid. */
function lastLine(chat: Chat | undefined): string | undefined {
  const text = chat?.lastMessagePreview?.trim();
  return text ? text.slice(0, 400) : undefined;
}

export function groupView(
  group: RunGroup,
  chats: ReadonlyMap<string, Chat>,
  approvals: readonly Approval[],
): GroupView {
  const waiting = new Map<string, number>();
  for (const a of approvals) waiting.set(a.chatId, (waiting.get(a.chatId) ?? 0) + 1);
  const members = group.members.map((m): GroupMemberView => ({
    chatId: m.chatId,
    label: memberLabel(m),
    status: STATUS[m.status] ?? 'queued',
    ...(lastLine(chats.get(m.chatId)) ? { lastMessage: lastLine(chats.get(m.chatId)) } : {}),
    branch: m.branch,
    cwd: m.cwd,
    filesChanged: m.filesChanged,
    additions: m.additions,
    deletions: m.deletions,
    elapsedMs: Number(m.elapsedMs),
    ...(m.startedAtMs !== undefined ? { startedAtMs: Number(m.startedAtMs) } : {}),
    ...(m.tokens !== undefined ? { tokens: Number(m.tokens) } : {}),
    check: {
      state: CHECK[m.check?.state ?? CheckState.UNSPECIFIED] ?? 'none',
      ...(m.check?.exitCode !== undefined ? { exitCode: m.check.exitCode } : {}),
      logTail: m.check?.logTail ?? [],
    },
    detached: m.detached,
    archived: m.archived,
    pendingApprovals: waiting.get(m.chatId) ?? 0,
  }));
  const resolution = group.resolution;
  return {
    id: group.id,
    prompt: group.prompt,
    base: group.base,
    repoPath: group.repoPath,
    state:
      group.state === RunGroupState.RESOLVED || resolution
        ? 'resolved'
        : group.state === RunGroupState.READY
          ? 'ready'
          : 'running',
    createdAtMs: Number(group.createdAtMs),
    hasCheck: Boolean(group.checkActionId),
    members,
    ...(resolution
      ? {
          resolution: {
            action: ACTION_NAME[resolution.action] ?? 'keep',
            winnerChatId: resolution.winnerChatId,
            result: resolution.result,
            ...(resolution.sha ? { sha: resolution.sha } : {}),
            ...(resolution.url ? { url: resolution.url } : {}),
          },
        }
      : {}),
  };
}

export function pendingView(approvals: readonly Approval[]): PendingApprovalView[] {
  return approvals.map((a) => ({
    id: a.id,
    chatId: a.chatId,
    tool: a.tool,
    args: a.args ? toJson(ValueSchema, a.args) : null,
  }));
}

const STRATEGY: Record<ResolveChoice['strategy'], MergeStrategy> = {
  merge: MergeStrategy.MERGE,
  squash: MergeStrategy.SQUASH,
  rebase: MergeStrategy.REBASE,
  ff: MergeStrategy.FAST_FORWARD,
};

const PROVIDER: Record<ResolveChoice['pr']['provider'], ChangeRequestProvider> = {
  forgejo: ChangeRequestProvider.FORGEJO,
  github: ChangeRequestProvider.GITHUB,
};

/** What the person confirmed about the winner's checkout. */
export interface ConfirmedWinner {
  /** The winner's branch now (its worktree's). */
  branch: string;
  cwd: string;
  /** The checkout diff's checksum shown in the dialog. */
  checksum?: string;
  /** The branch's head shown in the dialog. */
  head?: string;
}

/** `ResolveRunGroup`'s request for a confirmed choice (T12-8). */
export function resolveRequest(
  group: Pick<GroupView, 'id' | 'base' | 'repoPath'>,
  choice: ResolveChoice,
  winner: ConfirmedWinner,
): MessageInitShape<typeof ResolveRunGroupRequestSchema> {
  const common = {
    groupId: group.id,
    winnerChatId: choice.winnerChatId,
    archiveOthers: choice.archiveOthers,
    removeOtherWorktrees: choice.removeOtherWorktrees,
    force: choice.force,
  };
  switch (choice.action) {
    case 'merge':
      return {
        ...common,
        action: ResolveAction.MERGE,
        ...(winner.checksum ? { expectedChecksum: winner.checksum } : {}),
        merge: {
          repoPath: group.repoPath,
          source: winner.branch,
          target: group.base,
          strategy: STRATEGY[choice.strategy],
          deleteSource: false,
          removeWorktree: false,
          expectedSourceHead: winner.head ?? '',
        },
      };
    case 'pr':
      return {
        ...common,
        action: ResolveAction.CHANGE_REQUEST,
        ...(winner.checksum ? { expectedChecksum: winner.checksum } : {}),
        changeRequest: {
          cwd: winner.cwd,
          provider: PROVIDER[choice.pr.provider],
          base: group.base,
          head: winner.branch,
          title: choice.pr.title,
          body: choice.pr.body,
          draft: choice.pr.draft,
        },
      };
    case 'keep':
      return { ...common, action: ResolveAction.KEEP };
  }
}

const KNOWN_REASONS = ['group_not_ready', 'stale_diff', 'merge_conflict', 'push_rejected'];

/** A resolve's failure: the ErrorInfo reason, or the ndjson message's prefix. */
export function resolveErrorOf(e: unknown): ResolveError {
  const error = gitErrorOf(e);
  if (error.reason) return error;
  const prefix = error.message.split(':')[0] ?? '';
  return KNOWN_REASONS.includes(prefix) ? { ...error, reason: prefix } : error;
}
