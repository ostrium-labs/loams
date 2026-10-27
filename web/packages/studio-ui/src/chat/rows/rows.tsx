// The timeline's rows (SD1 Task 18): one component per `TimelineRow` kind, written for
// Studio on @loams/ui tokens over the vendored work-log pieces (WorkLog.tsx,
// tools/presentation.ts). Everything a model or tool wrote renders as text or through
// the sanitised markdown (§53 D919).

import type { UserInputAnswer } from "@loams/agentd-client";
import { AnimatePresence, Button, cx, motion, Spinner, useMotionPreset } from "@loams/ui";
import {
  AlertTriangle,
  Bot,
  Brain,
  CheckCircle2,
  ChevronRight,
  FileDiff,
  FilePen,
  FileText,
  GitFork,
  Globe,
  ImageIcon,
  ListTodo,
  RotateCcw,
  Search,
  TerminalSquare,
  Wrench,
  XCircle,
} from "lucide-react";
import { createContext, memo, type ReactNode, use, useState } from "react";
import { QuestionCard } from "../../approvals/Question";
import { Markdown } from "../../markdown/Markdown";
import { DiffStatLabel, hasNonZeroStat } from "../DiffStatLabel";
import { MessageCopyButton } from "../MessageCopyButton";
import { TasksBadge } from "../plan/TasksBadge";
import { formatDuration } from "../session.logic";
import { shouldCollapseUserMessage, type TimelineRow } from "../timeline.logic";
import {
  liveThoughtLine,
  resolveToolPresentation,
  summarizeToolGroup,
  type ToolGroupAction,
  type ToolPart,
} from "../tools/presentation";
import { WorkLogBlock, WorkLogButton, WorkLogDetails, WorkLogList, WorkLogRow } from "./WorkLog";

/** What rows can do. Stable across renders, so a memoised row re-renders only on its own data. */
export interface TimelineActions {
  compact: boolean;
  answer?: ((requestId: string, answers: UserInputAnswer[]) => Promise<unknown>) | undefined;
  restoreTurn?: ((turnId: string) => void) | undefined;
  viewTurnDiff?: ((turnId: string) => void) | undefined;
  openLink?: ((href: string) => void) | undefined;
  openFile?: ((path: string) => void) | undefined;
  copy?: ((text: string) => Promise<void>) | undefined;
  probe?: ((rowId: string) => void) | undefined;
}

export const TimelineActionsContext = createContext<TimelineActions>({ compact: false });

const ACTION_ICONS: Record<ToolGroupAction, typeof Wrench> = {
  command: TerminalSquare,
  read: FileText,
  edit: FilePen,
  "code-search": Search,
  search: Globe,
  fetch: Globe,
  plan: ListTodo,
  delegate: Bot,
  other: Wrench,
};

function ToolIcon({ part }: { part: ToolPart }) {
  const presentation = resolveToolPresentation(part);
  if (presentation.status === "inProgress") return <Spinner size="xs" label="Running" />;
  if (presentation.status === "failed") {
    return <XCircle className="size-3.5 text-danger" aria-label="Failed" />;
  }
  const Icon = ACTION_ICONS[presentation.action];
  return <Icon className="size-3.5" aria-hidden />;
}

function ToolLabel({ part }: { part: ToolPart }) {
  const presentation = resolveToolPresentation(part);
  return (
    <span>
      <span className={presentation.status === "failed" ? "text-danger" : "text-ink"}>
        {presentation.label}
      </span>{" "}
      <span className={/* mono-ok: code */ "font-mono text-xs text-muted"}>
        {presentation.detail}
      </span>
    </span>
  );
}

function ToolDetails({ part }: { part: ToolPart }) {
  const { openFile } = use(TimelineActionsContext);
  const stats = part.diffStats ?? [];
  return (
    <WorkLogDetails>
      {part.output ? (
        <pre className="m-0 max-h-64 overflow-auto rounded-md border border-rule-soft bg-raised px-2 py-1 font-mono text-xs whitespace-pre-wrap text-ink">
          {part.output}
        </pre>
      ) : (
        <span className="text-xs text-faint">{part.resolved ? "No output." : "Running…"}</span>
      )}
      {part.outputBytes && part.output && part.outputBytes > part.output.length ? (
        <span className="text-xs text-faint">
          Showing a summary of {Math.round(part.outputBytes / 1024)} KB of output.
        </span>
      ) : null}
      {stats.length > 0 ? (
        <ul className="m-0 list-none p-0">
          {stats.map((stat) => (
            <li key={stat.path} className="flex items-center gap-2 text-xs">
              <FileButton path={stat.path} onOpen={openFile} />
              {hasNonZeroStat(stat) ? (
                <DiffStatLabel
                  additions={stat.additions}
                  deletions={stat.deletions}
                  layout="inline"
                />
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </WorkLogDetails>
  );
}

function FileButton({
  path,
  onOpen,
}: {
  path: string;
  onOpen?: ((path: string) => void) | undefined;
}) {
  if (!onOpen)
    return (
      <span className={/* mono-ok: path */ "min-w-0 truncate font-mono text-ink"}>{path}</span>
    );
  return (
    <button
      type="button"
      onClick={() => onOpen(path)}
      title={`Open ${path}`}
      className={
        /* mono-ok: path */ "min-w-0 cursor-pointer truncate border-0 bg-transparent p-0 text-left font-mono text-ink hover:text-accent-fg hover:underline"
      }
    >
      {path}
    </button>
  );
}

/** A disclosure's chevron: it turns with the hover timing. */
const CHEVRON =
  "size-3.5 shrink-0 text-faint transition-transform duration-(--op-duration-fast) ease-out motion-reduce:transition-none";

/** A disclosure's body: it fades in when opened and out when closed, never on first render. */
function Reveal({ open, children }: { open: boolean; children: ReactNode }) {
  const fade = useMotionPreset("fade");
  return (
    <AnimatePresence initial={false}>
      {open ? (
        <motion.div
          initial={fade.initial}
          animate={fade.animate}
          exit={fade.exit}
          transition={fade.transition}
        >
          {children}
        </motion.div>
      ) : null}
    </AnimatePresence>
  );
}

function SingleToolRow({ part }: { part: ToolPart }) {
  const [open, setOpen] = useState(false);
  return (
    <WorkLogRow
      icon={<ToolIcon part={part} />}
      label={<ToolLabel part={part} />}
      role="button"
      tabIndex={0}
      aria-expanded={open}
      onClick={() => setOpen((v) => !v)}
      onKeyDown={(event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          setOpen((v) => !v);
        }
      }}
      trailing={<ChevronRight className={cx(CHEVRON, open && "rotate-90")} aria-hidden />}
    >
      <Reveal open={open}>
        <ToolDetails part={part} />
      </Reveal>
    </WorkLogRow>
  );
}

function WorkRow({ parts }: { parts: ToolPart[] }) {
  const [expanded, setExpanded] = useState(false);
  const running = parts.some((part) => resolveToolPresentation(part).status === "inProgress");
  if (parts.length === 1 && parts[0]) {
    return (
      <WorkLogBlock>
        <SingleToolRow part={parts[0]} />
      </WorkLogBlock>
    );
  }
  const { summary, hasFailure } = summarizeToolGroup(parts);
  return (
    <WorkLogBlock>
      <WorkLogButton
        icon={
          running ? (
            <Spinner size="xs" label="Running" />
          ) : hasFailure ? (
            <AlertTriangle className="size-3.5 text-danger" aria-hidden />
          ) : (
            <Wrench className="size-3.5" aria-hidden />
          )
        }
        label={<span className="text-ink">{summary}</span>}
        aria-expanded={expanded}
        onClick={() => setExpanded((v) => !v)}
        trailing={<ChevronRight className={cx(CHEVRON, expanded && "rotate-90")} aria-hidden />}
      />
      <Reveal open={expanded}>
        <div className="ms-3 border-l border-rule-soft ps-2">
          <WorkLogList>
            {parts.map((part) => (
              <SingleToolRow key={part.id} part={part} />
            ))}
          </WorkLogList>
        </div>
      </Reveal>
    </WorkLogBlock>
  );
}

function ReasoningRow({ text, streaming }: { text: string; streaming: boolean }) {
  const [open, setOpen] = useState(false);
  const { openLink, openFile } = use(TimelineActionsContext);
  const line = streaming ? liveThoughtLine(text) : "";
  return (
    <WorkLogBlock>
      <WorkLogButton
        icon={
          streaming ? (
            <Spinner size="xs" label="Thinking" />
          ) : (
            <Brain className="size-3.5" aria-hidden />
          )
        }
        label={
          <span>
            <span className="text-ink">{streaming ? "Thinking" : "Thought"}</span>
            {line ? <span className="text-muted"> · {line}</span> : null}
          </span>
        }
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        trailing={<ChevronRight className={cx(CHEVRON, open && "rotate-90")} aria-hidden />}
      />
      <Reveal open={open}>
        <div className="ms-7 border-l border-rule-soft ps-3 text-muted">
          <Markdown
            text={text}
            isStreaming={streaming}
            onOpenLink={openLink}
            onOpenFile={openFile}
          />
        </div>
      </Reveal>
    </WorkLogBlock>
  );
}

function UserRow({ text }: { text: string }) {
  const { openLink, openFile, compact } = use(TimelineActionsContext);
  const collapsible = shouldCollapseUserMessage(text);
  const [expanded, setExpanded] = useState(false);
  return (
    <div className={cx("flex justify-end", compact ? "py-1" : "py-2")}>
      <div className="max-w-[85%] min-w-0 rounded-lg border border-rule-soft bg-raised px-3 py-2">
        <div className={cx(collapsible && !expanded && "max-h-48 overflow-hidden")}>
          <Markdown text={text} lineBreaks onOpenLink={openLink} onOpenFile={openFile} />
        </div>
        {collapsible ? (
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className="mt-1 cursor-pointer border-0 bg-transparent p-0 text-xs text-accent-fg"
          >
            {expanded ? "Show less" : "Show more"}
          </button>
        ) : null}
      </div>
    </div>
  );
}

function TurnRow({ row }: { row: Extract<TimelineRow, { kind: "turn" }> }) {
  const { openFile, restoreTurn, viewTurnDiff, compact } = use(TimelineActionsContext);
  const { turn } = row;
  const [open, setOpen] = useState(false);
  const totals = turn.files.reduce(
    (sum, file) => ({
      additions: sum.additions + file.additions,
      deletions: sum.deletions + file.deletions,
    }),
    { additions: 0, deletions: 0 },
  );
  return (
    <div
      className="my-1 rounded-lg border border-rule-soft bg-surface"
      data-turn-summary={turn.turnId}
    >
      <div className="flex flex-wrap items-center gap-2 px-3 py-1.5 text-sm">
        {turn.files.length > 0 ? (
          <button
            type="button"
            aria-expanded={open}
            onClick={() => setOpen((v) => !v)}
            className="inline-flex cursor-pointer items-center gap-1.5 border-0 bg-transparent p-0 text-ink"
          >
            <ChevronRight className={cx(CHEVRON, open && "rotate-90")} aria-hidden />
            <FileDiff className="size-3.5 text-muted" aria-hidden />
            Changed {turn.files.length} {turn.files.length === 1 ? "file" : "files"}
          </button>
        ) : (
          <span className="inline-flex items-center gap-1.5 text-muted">
            <CheckCircle2 className="size-3.5" aria-hidden />
            No files changed
          </span>
        )}
        {hasNonZeroStat(totals) ? (
          <DiffStatLabel
            additions={totals.additions}
            deletions={totals.deletions}
            layout="inline"
            className="text-xs"
          />
        ) : null}
        <span className="ml-auto flex items-center gap-1.5">
          {viewTurnDiff && turn.files.length > 0 ? (
            <Button size="sm" variant="quiet" onClick={() => viewTurnDiff(turn.turnId)}>
              View diff
            </Button>
          ) : null}
          {restoreTurn && row.canRestore && !compact ? (
            <Button size="sm" variant="quiet" onClick={() => restoreTurn(turn.turnId)}>
              <RotateCcw className="mr-1 size-3" aria-hidden />
              Restore to before this turn
            </Button>
          ) : null}
        </span>
      </div>
      <Reveal open={open}>
        <ul className="m-0 list-none border-t border-rule-soft px-3 py-1.5">
          {turn.files.map((file) => (
            <li key={file.path} className="flex items-center gap-2 py-0.5 text-xs">
              <FileButton path={file.path} onOpen={openFile} />
              {hasNonZeroStat(file) ? (
                <DiffStatLabel
                  additions={file.additions}
                  deletions={file.deletions}
                  layout="inline"
                />
              ) : null}
            </li>
          ))}
        </ul>
      </Reveal>
    </div>
  );
}

function RowBody({ row }: { row: TimelineRow }): ReactNode {
  const actions = use(TimelineActionsContext);
  switch (row.kind) {
    case "user":
      return <UserRow text={row.text} />;
    case "system":
      return <p className="my-2 text-center text-xs text-muted">{row.text}</p>;
    case "text":
      return (
        <div className="py-1">
          <Markdown
            text={row.part.text}
            isStreaming={row.streaming}
            onOpenLink={actions.openLink}
            onOpenFile={actions.openFile}
          />
        </div>
      );
    case "reasoning":
      return <ReasoningRow text={row.part.text} streaming={row.streaming} />;
    case "work":
      return <WorkRow parts={row.parts} />;
    case "subagent": {
      const status = row.part.subagentStatus ?? (row.part.resolved ? "done" : "running");
      return (
        <WorkLogBlock>
          <WorkLogRow
            icon={
              status === "running" ? (
                <Spinner size="xs" label="Subagent running" />
              ) : status === "failed" ? (
                <XCircle className="size-3.5 text-danger" aria-label="Failed" />
              ) : (
                <Bot className="size-3.5" aria-hidden />
              )
            }
            label={
              <span>
                <span className="text-ink">
                  {status === "running"
                    ? "Subagent working"
                    : status === "failed"
                      ? "Subagent failed"
                      : "Subagent done"}
                </span>{" "}
                <span className={/* mono-ok: code */ "font-mono text-xs text-muted"}>
                  {resolveToolPresentation(row.part).detail}
                </span>
                {row.part.subagentTail ? (
                  <span className="text-muted"> · {row.part.subagentTail}</span>
                ) : null}
              </span>
            }
          />
        </WorkLogBlock>
      );
    }
    case "plan":
      return (
        <div className="py-1">
          <TasksBadge items={row.items} />
        </div>
      );
    case "input":
      return (
        <div className="py-1">
          <QuestionCard
            requestId={row.part.requestId}
            questions={row.part.questions}
            resolved={row.part.resolved}
            onAnswer={actions.answer}
          />
        </div>
      );
    case "image":
      return (
        <p className="my-1 inline-flex items-center gap-1.5 text-xs text-muted">
          <ImageIcon className="size-3.5" aria-hidden />
          {row.part.name}
        </p>
      );
    case "error":
      return (
        <div
          role="alert"
          className="my-1 flex items-start gap-2 rounded-lg border border-danger bg-danger-soft px-3 py-2 text-sm text-ink"
        >
          <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-danger" aria-hidden />
          <span className="min-w-0 whitespace-pre-wrap">{row.message}</span>
        </div>
      );
    case "fork":
      return (
        <p className="my-2 inline-flex items-center gap-1.5 text-xs text-muted">
          <GitFork className="size-3.5" aria-hidden />
          Forked from {row.part.sourceTitle}
        </p>
      );
    case "assistant-meta":
      return (
        <div className="flex items-center gap-2 pb-2 text-xs text-faint">
          {row.copyText ? <MessageCopyButton text={row.copyText} copy={actions.copy} /> : null}
          {row.durationMs !== null ? <span>{formatDuration(row.durationMs)}</span> : null}
          {row.aborted ? <span className="text-danger">Stopped</span> : null}
        </div>
      );
    case "turn":
      return <TurnRow row={row} />;
    case "working":
      return (
        <p className="my-2 inline-flex items-center gap-2 text-sm text-muted" aria-live="polite">
          <Spinner size="xs" label="Working" />
          Working…
        </p>
      );
  }
}

/** One row. Memoised on its row object (stable across unrelated deltas) and find state. */
export const TimelineRowView = memo(function TimelineRowView({
  row,
  matched,
  active,
}: {
  row: TimelineRow;
  matched: boolean;
  active: boolean;
}) {
  const actions = use(TimelineActionsContext);
  actions.probe?.(row.id);
  return (
    <div
      data-row-id={row.id}
      data-row-kind={row.kind}
      data-find-match={matched ? (active ? "active" : "") : undefined}
      className={cx(
        actions.compact ? "px-2" : "px-4",
        matched && "bg-accent-soft",
        active && "outline-2 -outline-offset-2 outline-accent",
      )}
    >
      <RowBody row={row} />
    </div>
  );
});
