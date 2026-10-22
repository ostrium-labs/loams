// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/command.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to Loams tokens, rounded-none and rounded-pill, and cx, with Base UI underneath; no cva, lucide or Effect; the palette is an elevated, rounded surface with the enter and exit timing (UI1); one focus ring on the search field and the opening rows rise in order (UI2). Loams' changes are Apache-2.0.

// The command palette primitive: Base UI's Dialog around an inline, always-open Base UI
// Autocomplete (no cmdk). T3 Code's ui/autocomplete.tsx was folded in here before CF1; its
// popup parts are src/autocomplete.tsx since CF1 Task 4. The @legendapp/list virtualisation
// is replaced by TanStack Virtual, the library behind
// @loams/ui's VirtualList, so a long list (every file in a project) stays cheap.
//
// The palette owns the keyboard: the first row is highlighted from the start, the arrow
// keys move the highlight (the input keeps focus), Enter runs the highlighted row, and
// Escape closes. Ranking is the caller's: pass `filter={null}` and pre-ranked `items` to
// take over, or keep the default substring match on `text` and `keywords`.

import { Autocomplete as BaseAutocomplete } from '@base-ui/react/autocomplete';
import { Dialog as BaseDialog } from '@base-ui/react/dialog';
import { useVirtualizer } from '@tanstack/react-virtual';
import { type CSSProperties, Fragment, type ReactNode, useRef, useState } from 'react';
import { cx } from './cx';
import { Kbd, KbdGroup } from './kbd';

export interface CommandItem {
  /** Stable and unique; the row's key and what `onSelect` gets back. */
  id: string;
  /** The row's plain text: what the default filter matches and what is read out. */
  text: string;
  /** A richer rendering of the row; `text` when absent. */
  label?: ReactNode;
  /** Rows with the same group are listed together under a heading (in list order). */
  group?: string;
  /** Secondary text at the end of the row (a path, a project). */
  detail?: ReactNode;
  icon?: ReactNode;
  /** Keys shown at the end of the row, for example ['Mod', 'N']. */
  shortcut?: string[];
  /** More words the default filter matches. */
  keywords?: string[];
  disabled?: boolean;
}

/** The default match: every word of the query appears in the text or the keywords. */
export function matchCommand(item: CommandItem, query: string): boolean {
  const hay = [item.text, ...(item.keywords ?? [])].join(' ').toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => hay.includes(word));
}

const ROW =
  'flex min-h-9 cursor-pointer select-none items-center gap-2.5 rounded-sm px-2.5 py-1.5 text-body ' +
  'text-ink outline-none data-highlighted:bg-raised data-disabled:cursor-default data-disabled:opacity-50 ' +
  '[&>svg]:size-4 [&>svg]:shrink-0 [&>svg]:text-muted';
const HEADING = 'px-2.5 pt-2.5 pb-1 text-micro font-medium text-faint';
const ROW_HEIGHT = 36;
/** How long after opening a row that mounts still counts as one the palette opened with (ms). */
const ENTER_WINDOW = 200;

export function CommandPalette({
  open,
  onOpenChange,
  items,
  onSelect,
  label = 'Command palette',
  placeholder = 'Type a command or search…',
  query,
  onQueryChange,
  filter,
  empty = 'No results.',
  footer,
  virtualizeAbove = 60,
  listHeight = 360,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  items: readonly CommandItem[];
  /** Runs the chosen row. The palette stays open; close it here if the action needs to. */
  onSelect: (item: CommandItem) => void;
  /** The dialog's and the input's accessible name. */
  label?: string;
  placeholder?: string;
  /** The query, when the caller controls it (to rank, or to switch to a file search). */
  query?: string;
  onQueryChange?: (query: string) => void;
  /** `null` turns filtering off (the caller ranks `items`); defaults to `matchCommand`. */
  filter?: ((item: CommandItem, query: string) => boolean) | null;
  empty?: ReactNode;
  /** Hints under the list (↑↓ to move, ↵ to run). */
  footer?: ReactNode;
  /** Above this many rows the list is virtualised; group headings then show inline. */
  virtualizeAbove?: number;
  /** The list's maximum height in pixels. */
  listHeight?: number;
}) {
  const virtual = items.length > virtualizeAbove;
  const scroller = useRef<HTMLDivElement | null>(null);
  const scrollTo = useRef<((index: number) => void) | null>(null);
  const match = filter === undefined ? matchCommand : filter;
  const openedAt = useRef(Number.NEGATIVE_INFINITY);
  const wasOpen = useRef(false);
  if (open && !wasOpen.current) openedAt.current = performance.now();
  wasOpen.current = open;
  return (
    <BaseDialog.Root open={open} onOpenChange={(next) => onOpenChange(next)}>
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className="loams-dialog-scrim" />
        <BaseDialog.Popup
          className={
            'fixed top-[12vh] left-1/2 z-[71] flex w-[min(640px,calc(100vw-32px))] -translate-x-1/2 flex-col ' +
            'overflow-hidden rounded-lg border border-rule bg-elevated text-ink shadow-modal outline-none ' +
            'transition-[opacity,scale] duration-(--op-duration-enter) ease-out data-ending-style:duration-(--op-duration-exit) data-ending-style:ease-in motion-reduce:transition-none ' +
            'data-starting-style:scale-[0.98] data-starting-style:opacity-0 data-ending-style:scale-[0.98] data-ending-style:opacity-0'
          }
          data-slot="command-dialog"
        >
          <BaseDialog.Title className="sr-only">{label}</BaseDialog.Title>
          <BaseAutocomplete.Root<CommandItem>
            items={items as CommandItem[]}
            open
            inline
            autoHighlight="always"
            keepHighlight
            virtualized={virtual}
            itemToStringValue={(item) => item.text}
            filter={match === null ? null : (item: CommandItem, q: string) => match(item, q)}
            {...(query !== undefined ? { value: query } : {})}
            onValueChange={(next) => onQueryChange?.(next)}
            onItemHighlighted={(_item, details) => {
              if (details.reason === 'keyboard' && details.index >= 0) {
                scrollTo.current?.(details.index);
              }
            }}
          >
            <div className="border-b border-rule-soft p-2">
              {/* One ring: the field's own 1px border turns the focus colour while the
                  input has focus (it always does while the palette is open). No outline. */}
              <div
                className={
                  'flex h-10 items-center gap-0.5 rounded-md border border-transparent bg-raised ps-3 ' +
                  'transition-colors duration-(--op-duration-fast) ease-out motion-reduce:transition-none ' +
                  'has-[input:focus]:border-focus has-[input:focus]:bg-bg'
                }
                data-slot="command-field"
              >
                <span className="flex shrink-0 text-faint">
                  <SearchGlyph />
                </span>
                <BaseAutocomplete.Input
                  autoFocus
                  aria-label={label}
                  placeholder={placeholder}
                  className={
                    'h-full min-w-0 flex-1 border-0 bg-transparent px-2.5 font-[inherit] text-emphasis text-ink ' +
                    'outline-none placeholder:text-faint'
                  }
                  data-slot="command-input"
                />
              </div>
            </div>
            <BaseAutocomplete.Empty className="px-4 py-6 text-center text-body text-muted empty:hidden">
              {empty}
            </BaseAutocomplete.Empty>
            <BaseAutocomplete.List
              ref={scroller}
              className="overflow-y-auto overscroll-contain p-1.5 empty:hidden"
              style={{ maxHeight: listHeight }}
              data-slot="command-list"
            >
              {virtual ? (
                <VirtualRows scroller={scroller} scrollTo={scrollTo} onSelect={onSelect} />
              ) : (
                <Rows onSelect={onSelect} openedAt={openedAt} />
              )}
            </BaseAutocomplete.List>
          </BaseAutocomplete.Root>
          {footer && (
            <div className="flex items-center justify-between gap-2 border-t border-rule-soft px-3 py-2 text-small text-muted">
              {footer}
            </div>
          )}
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}

/** The rows, with a heading where the group changes. */
function Rows({
  onSelect,
  openedAt,
}: {
  onSelect: (item: CommandItem) => void;
  openedAt: { current: number };
}) {
  const filtered = BaseAutocomplete.useFilteredItems<CommandItem>();
  return filtered.map((item, i) => (
    <Fragment key={item.id}>
      {item.group && item.group !== filtered[i - 1]?.group && (
        // A heading inside a flat list cannot wrap its rows in a group, so it is presentation.
        <div role="presentation" className={HEADING}>
          {item.group}
        </div>
      )}
      <Row item={item} index={i} onSelect={onSelect} openedAt={openedAt} />
    </Fragment>
  ));
}

/** A window of rows, positioned in a spacer as tall as the whole list. */
function VirtualRows({
  scroller,
  scrollTo,
  onSelect,
}: {
  scroller: { current: HTMLDivElement | null };
  scrollTo: { current: ((index: number) => void) | null };
  onSelect: (item: CommandItem) => void;
}) {
  const filtered = BaseAutocomplete.useFilteredItems<CommandItem>();
  const v = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW_HEIGHT,
    getItemKey: (i) => filtered[i]?.id ?? i,
    overscan: 8,
    // Before layout (and in tests) render a first screenful rather than nothing.
    initialRect: { width: 600, height: 360 },
  });
  scrollTo.current = (index) => v.scrollToIndex(index, { align: 'auto' });
  if (filtered.length === 0) return null;
  return (
    <div className="relative w-full" style={{ height: v.getTotalSize() }}>
      {v.getVirtualItems().map((row) => {
        const item = filtered[row.index];
        if (!item) return null;
        return (
          <Row
            key={row.key}
            item={item}
            index={row.index}
            onSelect={onSelect}
            groupInline
            style={{
              position: 'absolute',
              top: 0,
              left: 0,
              width: '100%',
              height: row.size,
              transform: `translateY(${row.start}px)`,
            }}
          />
        );
      })}
    </div>
  );
}

function Row({
  item,
  index,
  onSelect,
  groupInline = false,
  openedAt,
  style,
}: {
  item: CommandItem;
  index: number;
  onSelect: (item: CommandItem) => void;
  groupInline?: boolean;
  /** When the palette opened; rows that mount just after it rise in order. */
  openedAt?: { current: number };
  style?: CSSProperties;
}) {
  // One of the rows the palette opens with (mounted within ENTER_WINDOW of opening) rises
  // into place, `--loams-i` ordering it (styles.css, capped at 8 steps). Rows a query brings
  // in later just appear, so typing never waits on an animation.
  const [enter] = useState(
    () => openedAt !== undefined && performance.now() - openedAt.current < ENTER_WINDOW,
  );
  return (
    <BaseAutocomplete.Item
      value={item}
      index={index}
      disabled={item.disabled}
      onClick={() => onSelect(item)}
      className={ROW}
      style={enter ? ({ ...style, '--loams-i': index } as CSSProperties) : style}
      data-enter={enter ? '' : undefined}
      data-slot="command-item"
    >
      {item.icon}
      <span className="min-w-0 flex-1 truncate">{item.label ?? item.text}</span>
      {(item.detail || (groupInline && item.group)) && (
        <span className="min-w-0 shrink truncate text-small text-faint">
          {item.detail ?? item.group}
        </span>
      )}
      {item.shortcut && item.shortcut.length > 0 && <Shortcut keys={item.shortcut} />}
    </BaseAutocomplete.Item>
  );
}

/** A shortcut's keys, as a chord. */
export function Shortcut({ keys, className }: { keys: readonly string[]; className?: string }) {
  return (
    <KbdGroup className={cx('ms-auto', className)}>
      {keys.map((k) => (
        <Kbd key={k}>{k}</Kbd>
      ))}
    </KbdGroup>
  );
}

function SearchGlyph() {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      className="size-4"
      aria-hidden="true"
    >
      <circle cx="11" cy="11" r="7" />
      <path d="m20 20-3.5-3.5" />
    </svg>
  );
}
