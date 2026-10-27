// A windowed listbox for lists that can hold millions of rows (workspaces, §19.11). Only
// the rows in view, plus an overscan, are in the DOM. Focus stays on the listbox and the
// active row is named with aria-activedescendant, so the focused element never unmounts
// while the person scrolls with the keyboard. Each option carries aria-posinset and
// aria-setsize, so a screen reader announces "row 12 of 1,000,000".

import { type KeyboardEvent, type ReactNode, useEffect, useId, useRef, useState } from 'react';

export interface VirtualListProps<T> {
  items: T[];
  /** The total, when known; -1 tells assistive tech the size is unknown. */
  total?: number;
  rowHeight: number;
  /** Viewport height in pixels. Fixed, so the window does not depend on layout. */
  height: number;
  overscan?: number;
  label: string;
  getKey: (item: T) => string;
  renderRow: (item: T, active: boolean) => ReactNode;
  selectedKey?: string;
  onSelect: (item: T) => void;
  /** Called when the window nears the end of the loaded rows. */
  onEndReached?: () => void;
  /** How close (in rows) to the end triggers onEndReached. */
  endThreshold?: number;
}

export function visibleRange(
  scrollTop: number,
  height: number,
  rowHeight: number,
  count: number,
  overscan: number,
): [number, number] {
  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan);
  const last = Math.min(count, Math.ceil((scrollTop + height) / rowHeight) + overscan);
  return [first, last];
}

export function VirtualList<T>(p: VirtualListProps<T>) {
  const { items, rowHeight, height, onEndReached } = p;
  const overscan = p.overscan ?? 6;
  const threshold = p.endThreshold ?? 20;
  const ref = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [active, setActive] = useState(0);
  const idBase = useId();
  const [first, last] = visibleRange(scrollTop, height, rowHeight, items.length, overscan);

  // A new result set (a new search) starts at the top.
  const firstKey = items[0] ? p.getKey(items[0]) : '';
  // oxlint-disable-next-line react/exhaustive-deps -- reset only when the head changes
  useEffect(() => {
    setActive(0);
    setScrollTop(0);
    if (ref.current) ref.current.scrollTop = 0;
  }, [firstKey]);

  useEffect(() => {
    if (onEndReached && items.length > 0 && last >= items.length - threshold) onEndReached();
  }, [last, items.length, threshold, onEndReached]);

  const scrollInto = (index: number) => {
    const el = ref.current;
    if (!el) return;
    const top = index * rowHeight;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + rowHeight > el.scrollTop + height) el.scrollTop = top + rowHeight - height;
    setScrollTop(el.scrollTop);
  };

  const move = (index: number) => {
    if (items.length === 0) return;
    const next = Math.max(0, Math.min(items.length - 1, index));
    setActive(next);
    scrollInto(next);
  };

  const pageRows = Math.max(1, Math.floor(height / rowHeight) - 1);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const keys: Record<string, () => void> = {
      ArrowDown: () => move(active + 1),
      ArrowUp: () => move(active - 1),
      PageDown: () => move(active + pageRows),
      PageUp: () => move(active - pageRows),
      Home: () => move(0),
      End: () => move(items.length - 1),
      Enter: () => {
        const item = items[active];
        if (item) p.onSelect(item);
      },
      ' ': () => {
        const item = items[active];
        if (item) p.onSelect(item);
      },
    };
    const fn = keys[e.key];
    if (fn) {
      e.preventDefault();
      fn();
    }
  };

  const rowId = (i: number) => `${idBase}-row-${i}`;
  const rows: ReactNode[] = [];
  const indices: number[] = [];
  for (let i = first; i < last; i++) indices.push(i);
  // The active row stays in the DOM, so aria-activedescendant always names an element.
  if (active < items.length && (active < first || active >= last)) indices.push(active);
  for (const i of indices) {
    const item = items[i]!;
    const key = p.getKey(item);
    const selected = key === p.selectedKey;
    rows.push(
      // Keyboard and focus live on the listbox (aria-activedescendant), not on each option.
      // oxlint-disable-next-line jsx-a11y/interactive-supports-focus -- focus stays on the listbox
      // oxlint-disable-next-line jsx-a11y/click-events-have-key-events -- the listbox handles the keys
      <div
        key={key}
        id={rowId(i)}
        role="option"
        aria-selected={selected}
        aria-posinset={i + 1}
        aria-setsize={p.total ?? -1}
        className={`absolute right-0 left-0 flex cursor-pointer items-center px-2 ${
          i === active ? 'bg-accent-soft' : ''
        } ${selected ? 'font-semibold' : ''}`}
        style={{ top: i * rowHeight, height: rowHeight }}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => {
          setActive(i);
          ref.current?.focus();
          p.onSelect(item);
        }}
      >
        {p.renderRow(item, i === active)}
      </div>,
    );
  }

  return (
    <div
      ref={ref}
      role="listbox"
      tabIndex={0}
      aria-label={p.label}
      aria-activedescendant={items.length ? rowId(active) : undefined}
      className="relative overflow-y-auto rounded-md border border-solid border-rule focus-visible:outline-2 focus-visible:outline-accent"
      style={{ height }}
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      onKeyDown={onKeyDown}
    >
      <div style={{ height: items.length * rowHeight, position: 'relative' }}>{rows}</div>
    </div>
  );
}
