// The run group's grid (SD1 Task 25; design §53 §5.5 step 2): one column per variant, with
// its status, last message, files changed, `+/−`, elapsed time, tokens and check. Messages
// are text. A running member's time runs on from its start, once a second.

import { Button, cx } from "@loams/ui";
import {
  CircleAlert,
  CircleCheck,
  CircleDashed,
  Clock,
  LoaderCircle,
  MessageSquareText,
  ShieldQuestion,
} from "lucide-react";
import { useEffect, useState } from "react";

import {
  type CheckStatus,
  formatElapsed,
  type GroupMemberView,
  type MemberStatus,
  memberElapsedMs,
} from "./model";

export const MEMBER_STATUS_LABEL: Record<MemberStatus, string> = {
  queued: "Queued",
  running: "Running",
  "awaiting-input": "Awaiting input",
  done: "Done",
  errored: "Errored",
  cancelled: "Cancelled",
};

const STATUS_CLASS: Record<MemberStatus, string> = {
  queued: "text-muted",
  running: "text-accent-fg",
  "awaiting-input": "text-info",
  done: "text-grow",
  errored: "text-danger",
  cancelled: "text-faint",
};

function StatusIcon({ status }: { status: MemberStatus }) {
  const props = { size: 14, "aria-hidden": true, strokeWidth: 2 } as const;
  switch (status) {
    case "running":
      return <LoaderCircle {...props} className="motion-safe:animate-spin" />;
    case "awaiting-input":
      return <ShieldQuestion {...props} />;
    case "done":
      return <CircleCheck {...props} />;
    case "errored":
      return <CircleAlert {...props} />;
    default:
      return <CircleDashed {...props} />;
  }
}

export function checkLabel(state: CheckStatus, exitCode?: number): string | null {
  switch (state) {
    case "pending":
      return "Check waiting";
    case "running":
      return "Check running";
    case "passed":
      return "Check passed";
    case "failed":
      return exitCode !== undefined ? `Check failed (exit ${exitCode})` : "Check failed";
    default:
      return null;
  }
}

const CHECK_CLASS: Record<CheckStatus, string> = {
  none: "text-faint",
  pending: "text-muted",
  running: "text-accent-fg",
  passed: "text-grow",
  failed: "text-danger",
};

/** The time now, refreshed every `ms` while `live`. */
export function useNow(live: boolean, ms = 1000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    const timer = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(timer);
  }, [live, ms]);
  return now;
}

const tokenFormat = new Intl.NumberFormat("en-US");

function MemberColumn({
  member,
  now,
  hasCheck,
  winner,
  onContinue,
}: {
  member: GroupMemberView;
  now: number;
  hasCheck: boolean;
  winner: boolean;
  onContinue?: (chatId: string) => void;
}) {
  const check = checkLabel(member.check.state, member.check.exitCode);
  return (
    <article
      data-member={member.chatId}
      aria-label={member.label}
      className={cx(
        "flex min-w-[15rem] flex-col gap-3 rounded-lg border bg-surface p-4",
        winner ? "border-accent" : "border-rule-soft",
        member.archived && "opacity-70",
      )}
    >
      <header className="flex flex-col gap-1">
        <h3 className="m-0 truncate text-emphasis font-semibold text-ink" title={member.label}>
          {member.label}
        </h3>
        <p
          className={cx(
            "m-0 flex items-center gap-1.5 text-body font-medium",
            STATUS_CLASS[member.status],
          )}
        >
          <StatusIcon status={member.status} />
          <span>{MEMBER_STATUS_LABEL[member.status]}</span>
          {winner && <span className="ml-auto text-accent-fg text-micro font-medium">Winner</span>}
        </p>
        <p
          className={/* mono-ok: path */ "m-0 truncate font-mono text-micro text-faint"}
          title={member.cwd}
        >
          {member.branch}
        </p>
      </header>
      <p className="m-0 flex min-h-[3.6em] gap-1.5 text-body text-muted">
        <MessageSquareText size={14} aria-hidden="true" className="mt-0.5 shrink-0 text-faint" />
        <span className="line-clamp-3 break-words">{member.lastMessage || "No message yet."}</span>
      </p>
      <dl className="m-0 grid grid-cols-2 gap-x-3 gap-y-1 text-small">
        <dt className="sr-only">Files changed</dt>
        <dd className="m-0 text-ink">
          {member.filesChanged} {member.filesChanged === 1 ? "file" : "files"}
        </dd>
        <dt className="sr-only">Lines</dt>
        <dd className="m-0 flex gap-1.5 tabular-nums">
          <span className="text-grow">+{member.additions}</span>
          <span className="text-danger">−{member.deletions}</span>
        </dd>
        <dt className="sr-only">Elapsed</dt>
        <dd className="m-0 flex items-center gap-1 text-muted tabular-nums">
          <Clock size={12} aria-hidden="true" />
          {formatElapsed(memberElapsedMs(member, now))}
        </dd>
        <dt className="sr-only">Tokens</dt>
        <dd className="m-0 text-muted tabular-nums">
          {member.tokens !== undefined ? `Tokens ${tokenFormat.format(member.tokens)}` : "Tokens —"}
        </dd>
      </dl>
      {(check || hasCheck) && (
        <p className={cx("m-0 text-small font-medium", CHECK_CLASS[member.check.state])}>
          {check ?? "No check run yet"}
        </p>
      )}
      {member.pendingApprovals > 0 && (
        <p className="m-0 text-small text-info">
          {member.pendingApprovals === 1
            ? "1 approval waiting"
            : `${member.pendingApprovals} approvals waiting`}
        </p>
      )}
      {onContinue && (
        <div className="mt-auto pt-1">
          <Button size="sm" variant="secondary" onClick={() => onContinue(member.chatId)}>
            Continue as thread
          </Button>
        </div>
      )}
    </article>
  );
}

export function GroupGrid({
  members,
  hasCheck,
  winnerChatId,
  onContinue,
}: {
  readonly members: readonly GroupMemberView[];
  readonly hasCheck: boolean;
  readonly winnerChatId?: string;
  /** Opens a member as an ordinary thread (§53 §5.5: "continue a variant as a thread"). */
  readonly onContinue?: (chatId: string) => void;
}) {
  const live = members.some((m) => m.status === "running" || m.status === "awaiting-input");
  const now = useNow(live);
  return (
    <div className="grid grid-cols-[repeat(auto-fit,minmax(15rem,1fr))] gap-3">
      {members.map((member) => (
        <MemberColumn
          key={member.chatId}
          member={member}
          now={now}
          hasCheck={hasCheck}
          winner={member.chatId === winnerChatId}
          {...(onContinue ? { onContinue } : {})}
        />
      ))}
    </div>
  );
}

/** Each member's check result with its log tail, as text (§53 §5.5 "check results"). */
export function CheckResults({ members }: { readonly members: readonly GroupMemberView[] }) {
  const ran = members.filter((m) => m.check.state !== "none");
  if (ran.length === 0) {
    return <p className="m-0 py-2 text-body text-muted">No check has run yet.</p>;
  }
  return (
    <div className="flex flex-col gap-3">
      {ran.map((m) => (
        <section key={m.chatId} aria-label={`${m.label} check`} className="flex flex-col gap-1">
          <h4 className="m-0 flex items-baseline gap-2 text-body font-medium text-ink">
            {m.label}
            <span className={cx("text-small", CHECK_CLASS[m.check.state])}>
              {checkLabel(m.check.state, m.check.exitCode)}
            </span>
          </h4>
          {m.check.logTail.length > 0 ? (
            <pre className="m-0 max-h-56 overflow-auto rounded-md border border-rule-soft bg-bg p-2 font-mono text-micro leading-snug text-ink">
              {m.check.logTail.join("\n")}
            </pre>
          ) : (
            <p className="m-0 text-small text-faint">No output.</p>
          )}
        </section>
      ))}
    </div>
  );
}
