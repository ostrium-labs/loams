// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/chat/ChatHeader.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// the thread header's title and chips as props (harness and model, approval mode and sandbox, branch, usage; G20) with Stop, Find and Open in editor actions (the editor picker is the desktop's Settings › Studio); no project favicon, rename, action menu or environment scope. Loams' changes are Apache-2.0.

// The thread header (SD1 Task 18, §53 §5.4). Everything here is the parent's data; the
// header owns no state. Usage is token counts only (no prices; billing-grade
// metering is a separate design, §63).

import { Button, cx, Spinner } from "@loams/ui";
import { FolderCode, GitBranch, Search, ShieldCheck, Square } from "lucide-react";
import { memo, type ReactNode } from "react";

export interface ThreadUsage {
  inputTokens?: number | undefined;
  outputTokens?: number | undefined;
  cacheTokens?: number | undefined;
}

export interface ThreadHeaderProps {
  title: string;
  harness?: string | undefined;
  model?: string | undefined;
  /** `supervised` or `full-access` (§53 §8.1); absent when the daemon does not say. */
  approvalMode?: string | undefined;
  sandbox?: string | undefined;
  branch?: string | undefined;
  usage?: ThreadUsage | undefined;
  running?: boolean;
  onInterrupt?: (() => void) | undefined;
  onFind?: (() => void) | undefined;
  /** Opens the thread's checkout in the person's editor (the desktop only). */
  onOpenInEditor?: (() => void) | undefined;
  compact?: boolean;
}

const numberFormat = new Intl.NumberFormat(undefined, { notation: "compact" });

export function formatTokens(count: number | undefined): string {
  return count === undefined ? "–" : numberFormat.format(count);
}

function Chip({ title, children }: { title: string; children: ReactNode }) {
  return (
    <span
      title={title}
      className="inline-flex max-w-56 min-w-0 items-center gap-1 rounded-pill border border-rule-soft bg-surface px-2 py-0.5 text-xs text-muted"
    >
      {children}
    </span>
  );
}

export const ThreadHeader = memo(function ThreadHeader({
  title,
  harness,
  model,
  approvalMode,
  sandbox,
  branch,
  usage,
  running = false,
  onInterrupt,
  onFind,
  onOpenInEditor,
  compact = false,
}: ThreadHeaderProps) {
  const totalTokens =
    usage && (usage.inputTokens !== undefined || usage.outputTokens !== undefined)
      ? (usage.inputTokens ?? 0) + (usage.outputTokens ?? 0)
      : undefined;
  const chips =
    compact ||
    !(harness || approvalMode || sandbox || branch || totalTokens !== undefined) ? null : (
      <div className="flex min-w-0 flex-wrap items-center gap-1.5">
        {harness ? (
          <Chip title="Harness and model">
            <span className="truncate">{model ? `${harness} · ${model}` : harness}</span>
          </Chip>
        ) : null}
        {approvalMode || sandbox ? (
          <Chip title="Approval mode and sandbox">
            <ShieldCheck className="size-3 shrink-0" aria-hidden />
            <span className="truncate">
              {[approvalMode, sandbox].filter((part) => part).join(" · ")}
            </span>
          </Chip>
        ) : null}
        {branch ? (
          <Chip title="Branch">
            <GitBranch className="size-3 shrink-0" aria-hidden />
            <span className={/* mono-ok: id */ "truncate font-mono"}>{branch}</span>
          </Chip>
        ) : null}
        {totalTokens !== undefined ? (
          <Chip
            title={`Input ${formatTokens(usage?.inputTokens)}, output ${formatTokens(usage?.outputTokens)}, cache ${formatTokens(usage?.cacheTokens)} tokens`}
          >
            <span className="tabular-nums">{formatTokens(totalTokens)} tokens</span>
          </Chip>
        ) : null}
      </div>
    );
  return (
    <header
      className={cx(
        "flex min-w-0 flex-col gap-1.5 border-b border-rule-soft",
        compact ? "px-2 py-1" : "px-4 py-2",
      )}
      data-thread-header=""
    >
      <div className="flex min-w-0 items-center gap-2">
        <h2 className="m-0 min-w-0 flex-1 truncate text-sm font-semibold text-ink" title={title}>
          {title}
        </h2>
        {running ? (
          <span
            className="inline-flex shrink-0 items-center gap-1 text-xs text-muted"
            aria-live="polite"
          >
            <Spinner size="xs" label="Working" />
            Working
          </span>
        ) : null}
        {onFind ? (
          <Button
            size="icon"
            variant="quiet"
            aria-label="Find in thread"
            title="Find in thread"
            onClick={onFind}
          >
            <Search className="size-3.5" aria-hidden />
          </Button>
        ) : null}
        {onOpenInEditor ? (
          <Button
            size="icon"
            variant="quiet"
            aria-label="Open in editor"
            title="Open in editor"
            onClick={onOpenInEditor}
          >
            <FolderCode className="size-3.5" aria-hidden />
          </Button>
        ) : null}
        {running && onInterrupt ? (
          <Button size="sm" variant="secondary" onClick={onInterrupt}>
            <Square className="mr-1 size-3" aria-hidden />
            Stop
          </Button>
        ) : null}
      </div>
      {chips}
    </header>
  );
});
