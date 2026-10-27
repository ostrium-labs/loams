// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/chat/MessagesTimeline.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// the timeline as a scroll container over stable rows (rows/rows.tsx): virtualised with TanStack Virtual past a few hundred rows instead of LegendList, following the end within the re-arm band, anchoring the first visible row when history is prepended, find-in-thread highlights, inline approval cards at the end, and the restore confirmation; agentd transcript entries as Props, no atoms, minimap, canvas or composer inset. Loams' changes are Apache-2.0.

// The chat timeline (SD1 Task 18; §53 §5.4). The page and the dock's compact variant
// render the same component over the same store (D917).

import type { SessionMessageEntry, UserInputAnswer } from "@loams/agentd-client";
import { ConfirmDialog, cx, motion, useMotionPreset } from "@loams/ui";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ApprovalDecision } from "../approvals/ApprovalActions";
import { PendingApprovalCard } from "../approvals/PendingApproval";
import { type TimelineActions, TimelineActionsContext, TimelineRowView } from "./rows/rows";
import {
  captureTimelineAnchor,
  readTimelinePosition,
  rememberTimelinePosition,
  resolveTimelineIsAtEnd,
  restoreTimelineAnchor,
  rowsWerePrepended,
  type TimelineAnchor,
} from "./scroll/timelineScrollAnchoring";
import type { PendingApproval } from "./session.logic";
import {
  computeStableTimelineRows,
  deriveTimelineRows,
  EMPTY_STABLE_ROWS,
  type StableTimelineRowsState,
  type TimelineRow,
} from "./timeline.logic";

/** Past this many rows the timeline renders only the rows in view. */
export const VIRTUALIZE_AFTER = 200;
const ESTIMATED_ROW_HEIGHT = 72;

export interface TimelineProps {
  entries: ReadonlyArray<SessionMessageEntry>;
  /** `compact` is the agent dock's variant: tighter, no restore. */
  variant?: "page" | "compact";
  /** Turn ids (user message ids) with a checkpoint (`ListCheckpoints`). */
  checkpoints?: ReadonlySet<string>;
  /** Approvals the daemon holds for this thread (G17). */
  approvals?: ReadonlyArray<PendingApproval>;
  /** Older history is still loading above. */
  historyPending?: boolean;
  /** Remembers the scroll position per thread for this session. */
  threadKey?: string;
  onAnswer?: (requestId: string, answers: UserInputAnswer[]) => Promise<unknown>;
  onResolveApproval?: (approval: PendingApproval, decision: ApprovalDecision) => Promise<unknown>;
  /** Restores the checkout to before `turnId`; the timeline confirms first. */
  onRestoreTurn?: (turnId: string) => Promise<unknown>;
  onViewTurnDiff?: (turnId: string) => void;
  onOpenLink?: (href: string) => void;
  onOpenFile?: (path: string) => void;
  onCopy?: (text: string) => Promise<void>;
  /** Find in thread: the rows that match, and the active one (scrolled into view). */
  findMatches?: ReadonlyArray<string>;
  activeMatchId?: string | null;
  /** Shown when there are no rows. */
  empty?: ReactNode;
  /** Called with a row's id each time it renders (tests and profiling). */
  renderProbe?: (rowId: string) => void;
  className?: string;
}

/** Rows for `entries`, reusing every unchanged row object across renders. */
export function useTimelineRows(
  entries: ReadonlyArray<SessionMessageEntry>,
  checkpoints: ReadonlySet<string> | undefined,
): TimelineRow[] {
  const stable = useRef<StableTimelineRowsState>(EMPTY_STABLE_ROWS);
  return useMemo(() => {
    const rows = deriveTimelineRows({ entries, ...(checkpoints ? { checkpoints } : {}) });
    stable.current = computeStableTimelineRows(rows, stable.current);
    return stable.current.result;
  }, [entries, checkpoints]);
}

function useLatest<T>(value: T) {
  const ref = useRef(value);
  ref.current = value;
  return ref;
}

export function Timeline(props: TimelineProps) {
  const {
    entries,
    variant = "page",
    checkpoints,
    approvals = [],
    historyPending = false,
    threadKey,
    findMatches,
    activeMatchId = null,
    empty,
    className,
  } = props;
  const compact = variant === "compact";
  const rows = useTimelineRows(entries, checkpoints);
  const latest = useLatest(props);
  const [confirmTurn, setConfirmTurn] = useState<string | null>(null);
  const [restoring, setRestoring] = useState(false);

  const has = {
    answer: props.onAnswer !== undefined,
    restore: props.onRestoreTurn !== undefined,
    diff: props.onViewTurnDiff !== undefined,
    link: props.onOpenLink !== undefined,
    file: props.onOpenFile !== undefined,
    copy: props.onCopy !== undefined,
    probe: props.renderProbe !== undefined,
  };
  // One stable object while the set of handlers stays the same, so memoised rows
  // re-render only when their own row changes.
  const actions = useMemo<TimelineActions>(
    () => ({
      compact,
      answer: has.answer
        ? (requestId, answers) => latest.current.onAnswer?.(requestId, answers) ?? Promise.resolve()
        : undefined,
      restoreTurn: has.restore ? (turnId) => setConfirmTurn(turnId) : undefined,
      viewTurnDiff: has.diff ? (turnId) => latest.current.onViewTurnDiff?.(turnId) : undefined,
      openLink: has.link ? (href) => latest.current.onOpenLink?.(href) : undefined,
      openFile: has.file ? (path) => latest.current.onOpenFile?.(path) : undefined,
      copy: has.copy ? (text) => latest.current.onCopy?.(text) ?? Promise.resolve() : undefined,
      probe: has.probe ? (rowId) => latest.current.renderProbe?.(rowId) : undefined,
    }),
    [compact, latest, has.answer, has.restore, has.diff, has.link, has.file, has.copy, has.probe],
  );

  const matchSet = useMemo(() => new Set(findMatches ?? []), [findMatches]);
  const scroller = useRef<HTMLDivElement | null>(null);
  const virtual = rows.length > VIRTUALIZE_AFTER;
  const virtualizer = useVirtualizer({
    count: virtual ? rows.length : 0,
    getScrollElement: () => scroller.current,
    estimateSize: () => ESTIMATED_ROW_HEIGHT,
    getItemKey: (index) => rows[index]?.id ?? index,
    overscan: 6,
  });

  // Scroll state: following the end, and the first visible row's place on screen.
  const remembered = threadKey ? readTimelinePosition(threadKey) : undefined;
  const atEnd = useRef(remembered?.atEnd ?? true);
  const anchor = useRef<TimelineAnchor | null>(remembered?.anchor ?? null);
  const previousRows = useRef<TimelineRow[]>([]);

  const rowGeometry = useCallback((): { id: string; top: number; height: number }[] => {
    const el = scroller.current;
    if (!el) return [];
    if (virtual) {
      return virtualizer.getVirtualItems().map((item) => ({
        id: String(item.key),
        top: item.start,
        height: item.size,
      }));
    }
    const base = el.getBoundingClientRect().top - el.scrollTop;
    return [...el.querySelectorAll<HTMLElement>("[data-row-id]")].map((node) => {
      const rect = node.getBoundingClientRect();
      return { id: node.dataset.rowId ?? "", top: rect.top - base, height: rect.height };
    });
  }, [virtual, virtualizer]);

  const rowTop = useCallback(
    (rowId: string): number | null => {
      const el = scroller.current;
      if (!el) return null;
      if (virtual) {
        const index = rows.findIndex((row) => row.id === rowId);
        if (index < 0) return null;
        return virtualizer.getOffsetForIndex(index, "start")?.[0] ?? null;
      }
      const node = el.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(rowId)}"]`);
      if (!node) return null;
      return node.getBoundingClientRect().top - el.getBoundingClientRect().top + el.scrollTop;
    },
    [rows, virtual, virtualizer],
  );

  const onScroll = useCallback(() => {
    const el = scroller.current;
    if (!el) return;
    atEnd.current = resolveTimelineIsAtEnd(el);
    anchor.current = captureTimelineAnchor(rowGeometry(), el.scrollTop);
    if (threadKey) {
      rememberTimelinePosition(threadKey, { anchor: anchor.current, atEnd: atEnd.current });
    }
  }, [rowGeometry, threadKey]);

  // After each change of rows: keep the anchor on a prepend, else follow the end.
  // oxlint-disable-next-line react/exhaustive-deps -- runs when the rows or cards change
  useLayoutEffect(() => {
    const el = scroller.current;
    const previous = previousRows.current;
    previousRows.current = rows;
    if (!el) return;
    if (
      previous.length > 0 &&
      rowsWerePrepended(previous, rows) &&
      !atEnd.current &&
      anchor.current
    ) {
      const top = rowTop(anchor.current.rowId);
      if (top !== null) el.scrollTop = restoreTimelineAnchor(anchor.current, top);
      return;
    }
    if (atEnd.current) {
      el.scrollTop = el.scrollHeight;
    } else if (previous.length === 0 && anchor.current) {
      const top = rowTop(anchor.current.rowId);
      if (top !== null) el.scrollTop = restoreTimelineAnchor(anchor.current, top);
    }
  }, [rows, approvals.length]);

  // Bring the active find match into view.
  useEffect(() => {
    if (!activeMatchId) return;
    const el = scroller.current;
    if (!el) return;
    if (virtual) {
      const index = rows.findIndex((row) => row.id === activeMatchId);
      if (index >= 0) virtualizer.scrollToIndex(index, { align: "center" });
      return;
    }
    const node = el.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(activeMatchId)}"]`);
    node?.scrollIntoView?.({ block: "center" });
    atEnd.current = false;
  }, [activeMatchId, rows, virtual, virtualizer]);

  const resolve = useCallback(
    (approval: PendingApproval, decision: ApprovalDecision) =>
      latest.current.onResolveApproval?.(approval, decision) ?? Promise.resolve(),
    [latest],
  );

  // Rows that arrive while the thread is open rise in; the history the thread opens with,
  // rows loaded above it and rows that only change in place do not move.
  const entering = useEnteringRows(rows, threadKey);
  const rise = useMotionPreset("fadeUp");

  const renderRow = (row: TimelineRow) => (
    <TimelineRowView row={row} matched={matchSet.has(row.id)} active={activeMatchId === row.id} />
  );

  return (
    <TimelineActionsContext value={actions}>
      <div
        ref={scroller}
        onScroll={onScroll}
        className={cx("min-h-0 flex-1 overflow-y-auto", compact ? "text-[0.8rem]" : "", className)}
        role="log"
        aria-label="Thread timeline"
        aria-live="off"
        data-timeline={variant}
        // oxlint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- a scrolling region must be reachable by keyboard
        tabIndex={0}
      >
        <div className={cx("mx-auto w-full", compact ? "py-1" : "max-w-3xl py-4")}>
          {historyPending ? (
            <p className="px-4 py-2 text-center text-xs text-faint">Loading earlier messages…</p>
          ) : null}
          {rows.length === 0 && approvals.length === 0 ? (empty ?? null) : null}
          {virtual ? (
            <div className="relative w-full" style={{ height: virtualizer.getTotalSize() }}>
              {virtualizer.getVirtualItems().map((item) => {
                const row = rows[item.index];
                if (!row) return null;
                return (
                  <div
                    key={item.key}
                    ref={virtualizer.measureElement}
                    data-index={item.index}
                    className="absolute top-0 left-0 w-full"
                    style={{ transform: `translateY(${item.start}px)` }}
                  >
                    {renderRow(row)}
                  </div>
                );
              })}
            </div>
          ) : (
            rows.map((row) =>
              entering.has(row.id) ? (
                <motion.div
                  key={row.id}
                  initial={rise.initial}
                  animate={rise.animate}
                  transition={rise.transition}
                >
                  {renderRow(row)}
                </motion.div>
              ) : (
                <div key={row.id}>{renderRow(row)}</div>
              ),
            )
          )}
          {approvals.length > 0 ? (
            <div className={cx("flex flex-col gap-2 py-2", compact ? "px-2" : "px-4")}>
              {approvals.map((approval, index) => (
                <PendingApprovalCard
                  key={approval.id}
                  approval={approval}
                  pendingCount={approvals.length}
                  position={index + 1}
                  autoFocus={index === 0}
                  {...(props.onResolveApproval ? { onResolve: resolve } : {})}
                />
              ))}
            </div>
          ) : null}
        </div>
      </div>
      <ConfirmDialog
        open={confirmTurn !== null}
        onClose={() => {
          if (!restoring) setConfirmTurn(null);
        }}
        object="to before this turn"
        confirm="Restore"
        pending={restoring}
        impact="The checkout goes back to how it was before this turn started. Changes made since, by the agent or by you, are discarded. The thread's messages stay."
        onConfirm={() => {
          const turnId = confirmTurn;
          if (!turnId) return;
          setRestoring(true);
          Promise.resolve(latest.current.onRestoreTurn?.(turnId)).then(
            () => {
              setRestoring(false);
              setConfirmTurn(null);
            },
            () => setRestoring(false),
          );
        }}
      />
    </TimelineActionsContext>
  );
}

/**
 * The ids of rows appended after the timeline first showed rows (for `threadKey`). The first
 * non-empty set is the baseline; a row counts as new only when it lands after every row seen
 * so far, so history loaded above and rows re-derived in place never animate.
 */
function useEnteringRows(rows: TimelineRow[], threadKey: string | undefined): ReadonlySet<string> {
  const state = useRef<{
    key: string | undefined;
    seen: Set<string> | null;
    entering: Set<string>;
  }>({ key: threadKey, seen: null, entering: new Set() });
  const s = state.current;
  if (s.key !== threadKey) {
    s.key = threadKey;
    s.seen = null;
    s.entering = new Set();
  }
  if (s.seen === null) {
    if (rows.length > 0) s.seen = new Set(rows.map((row) => row.id));
    return s.entering;
  }
  let lastSeen = -1;
  rows.forEach((row, index) => {
    if (s.seen?.has(row.id)) lastSeen = index;
  });
  for (let i = lastSeen + 1; i < rows.length; i++) {
    const id = rows[i]?.id;
    if (id !== undefined && !s.seen.has(id)) s.entering.add(id);
  }
  for (const row of rows) s.seen.add(row.id);
  return s.entering;
}
