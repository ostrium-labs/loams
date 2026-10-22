// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/chat/ComposerPendingApprovalPanel.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// an inline timeline card for an agentd approval (G17): the request kind's label, the call's detail as text, the queue position, the risk, and the decisions with Enter, Shift+Enter and Esc while the card has focus; @loams/ui tokens. Loams' changes are Apache-2.0.

// An approval the daemon holds before a tool runs in supervised mode (SD1 Task 18,
// §53 §8.1; Q802 keeps DD1e's approval card's job, drawn here from upstream's panel).
// The call's arguments are untrusted: they show as text only.

import { cx, motion, useMotionPreset } from "@loams/ui";
import { ShieldAlert } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";
import type { PendingApproval } from "../chat/session.logic";
import { ApprovalActions, type ApprovalDecision } from "./ApprovalActions";

const MAX_DETAIL = 2000;

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

/** What the card calls the request, from the tool's name. */
export function approvalRequestKind(
  tool: string,
): "command" | "file-read" | "file-change" | "tool" {
  const name = tool.toLowerCase();
  if (/(bash|shell|exec|command|terminal)/.test(name)) return "command";
  if (/(read|view|cat|open)/.test(name)) return "file-read";
  if (/(write|edit|patch|create|delete|move|rename)/.test(name)) return "file-change";
  return "tool";
}

/** The call's detail: the command, the path, or the arguments as JSON. Plain text. */
export function approvalDetail(approval: Pick<PendingApproval, "tool" | "args">): string {
  const args = asRecord(approval.args);
  const pick = (key: string) => (typeof args?.[key] === "string" ? (args[key] as string) : null);
  const text =
    pick("command") ??
    pick("cmd") ??
    pick("file_path") ??
    pick("path") ??
    (approval.args === undefined || approval.args === null
      ? ""
      : JSON.stringify(approval.args, null, 2));
  return text.length > MAX_DETAIL ? `${text.slice(0, MAX_DETAIL)}…` : text;
}

const KIND_LABEL = {
  command: "Command approval",
  "file-read": "File read approval",
  "file-change": "File change approval",
  tool: "Tool approval",
} as const;

export const PendingApprovalCard = memo(function PendingApprovalCard({
  approval,
  pendingCount = 1,
  position = 1,
  autoFocus = false,
  onResolve,
}: {
  approval: PendingApproval;
  pendingCount?: number;
  position?: number;
  autoFocus?: boolean;
  onResolve?: (approval: PendingApproval, decision: ApprovalDecision) => Promise<unknown>;
}) {
  const [responding, setResponding] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const kind = approvalRequestKind(approval.tool);
  const label = KIND_LABEL[kind];
  const detail = approvalDetail(approval);
  const canRespond = onResolve !== undefined;
  const ref = useRef<HTMLElement | null>(null);
  const rise = useMotionPreset("fadeUp");

  // The first pending approval takes focus so its keys answer it, but never from a field
  // the person is typing in.
  useEffect(() => {
    if (!autoFocus) return;
    const active = document.activeElement;
    if (!active || active === document.body) ref.current?.focus();
  }, [autoFocus]);

  const decide = (decision: ApprovalDecision) => {
    if (!onResolve || responding) return;
    setResponding(true);
    setError(null);
    onResolve(approval, decision).then(
      () => setResponding(false),
      (cause: unknown) => {
        setResponding(false);
        setError(cause instanceof Error ? cause.message : String(cause));
      },
    );
  };

  return (
    <motion.section
      initial={rise.initial}
      animate={rise.animate}
      transition={rise.transition}
      aria-label={label}
      tabIndex={0}
      ref={ref}
      data-approval-card={approval.id}
      className="rounded-lg border border-control-border bg-elevated shadow-chip focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget) return;
        if (event.key === "Enter") {
          event.preventDefault();
          decide(event.shiftKey ? "always" : "once");
        } else if (event.key === "Escape") {
          event.preventDefault();
          decide("deny");
        }
      }}
    >
      <div className="flex items-center gap-2 px-3 pt-2.5 text-small text-muted">
        <ShieldAlert className="size-3.5 shrink-0 text-warn" aria-hidden />
        <span className="shrink-0 font-medium text-ink">{label}</span>
        <span className={/* mono-ok: id */ "min-w-0 truncate font-mono"}>{approval.tool}</span>
        {approval.risk === "destructive" || approval.risk === "write" ? (
          <span
            className={cx(
              "shrink-0 rounded-pill px-1.5",
              approval.risk === "destructive" ? "bg-danger-soft text-danger" : "bg-raised text-ink",
            )}
          >
            {approval.risk}
          </span>
        ) : null}
        {pendingCount > 1 ? (
          <span className="ml-auto shrink-0 tabular-nums">
            {position}/{pendingCount}
          </span>
        ) : null}
      </div>
      {detail ? (
        <pre className="mx-3 mt-2 mb-0 max-h-32 overflow-auto rounded-md border border-rule-soft bg-bg px-2.5 py-1.5 font-mono text-small whitespace-pre-wrap text-ink">
          {detail}
        </pre>
      ) : null}
      <div className="px-3 pt-2.5 pb-3">
        <ApprovalActions isResponding={responding} canRespond={canRespond} onDecide={decide} />
        {error ? (
          <p role="alert" className="m-0 mt-1 text-xs text-danger">
            {error}
          </p>
        ) : null}
      </div>
    </motion.section>
  );
});
