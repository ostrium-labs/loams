// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/sidebar.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to Loams tokens, rounded-none and rounded-pill, and cx, with Base UI underneath; no cva, lucide or Effect. Loams' changes are Apache-2.0.

// A resizable, collapsible sidebar and its parts (header, scrolling content, groups, menu
// rows, footer), and the resize handle it uses, which a panel or a drawer can use too.
//
// Upstream keeps the open state and width in a provider with a cookie, an Effect Schema
// localStorage codec and a mobile sheet. Here the sidebar is controlled: the caller owns
// `width` and `open` (and persists them, wrapped in try/catch), and a narrow window shows
// the content in a `Sheet` (overlay.tsx). The rail's pointer drag is kept; the handle is also
// a focusable window splitter (WAI-ARIA), so the arrow keys, Home and End resize it.

import {
  type ComponentProps,
  type KeyboardEvent,
  type PointerEvent,
  type ReactNode,
  useId,
  useRef,
} from 'react';
import { cx } from './cx';
import { ScrollArea } from './scroll-area';

/* ResizeHandle ------------------------------------------------------------------------ */

/**
 * A focusable splitter. `edge` says which side of the resized region the handle sits on:
 * `end` for a region before it (a left sidebar, a drawer above), `start` for a region after
 * it (a right-hand panel, a drawer below). Arrow keys move it by `step`; Home and End jump to
 * the limits.
 */
export function ResizeHandle({
  label,
  value,
  min,
  max,
  onResize,
  onResizeEnd,
  orientation = 'vertical',
  edge = 'end',
  step = 24,
  controls,
  className,
}: {
  /** The accessible name, for example "Resize sidebar". */
  label: string;
  value: number;
  min: number;
  max: number;
  onResize: (value: number) => void;
  /** Called once when a drag ends, with the final size (to persist it). */
  onResizeEnd?: (value: number) => void;
  /** `vertical` sits between columns and resizes a width; `horizontal` resizes a height. */
  orientation?: 'vertical' | 'horizontal';
  edge?: 'start' | 'end';
  step?: number;
  /** The id of the region it resizes. */
  controls?: string;
  className?: string;
}) {
  const clamp = (n: number) => Math.min(max, Math.max(min, Math.round(n)));
  const drag = useRef<{ pointer: number; from: number; start: number; last: number } | null>(null);
  const vertical = orientation === 'vertical';
  // Moving right (or down) grows a region before the handle and shrinks one after it.
  const sign = edge === 'end' ? 1 : -1;

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const grow = vertical ? 'ArrowRight' : 'ArrowDown';
    const shrink = vertical ? 'ArrowLeft' : 'ArrowUp';
    let next: number | null = null;
    if (e.key === grow) next = value + sign * step;
    else if (e.key === shrink) next = value - sign * step;
    else if (e.key === 'Home') next = min;
    else if (e.key === 'End') next = max;
    if (next === null) return;
    e.preventDefault();
    const clamped = clamp(next);
    onResize(clamped);
    onResizeEnd?.(clamped);
  };

  const at = (e: PointerEvent) => (vertical ? e.clientX : e.clientY);
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    drag.current = { pointer: e.pointerId, from: at(e), start: value, last: value };
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // jsdom and some embedded views have no pointer capture; the drag still works.
    }
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || d.pointer !== e.pointerId) return;
    const next = clamp(d.start + sign * (at(e) - d.from));
    if (next !== d.last) {
      d.last = next;
      onResize(next);
    }
  };
  const end = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d || d.pointer !== e.pointerId) return;
    drag.current = null;
    try {
      if (e.currentTarget.hasPointerCapture(e.pointerId)) {
        e.currentTarget.releasePointerCapture(e.pointerId);
      }
    } catch {
      // Already released.
    }
    if (d.last !== d.start) onResizeEnd?.(d.last);
  };

  return (
    // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- a focusable window splitter has no native element
    <div
      role="separator"
      aria-orientation={orientation}
      aria-label={label}
      aria-valuemin={min}
      aria-valuemax={max}
      aria-valuenow={value}
      aria-controls={controls}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={end}
      onPointerCancel={end}
      onLostPointerCapture={end}
      className={cx(
        'relative shrink-0 touch-none bg-rule-soft outline-none hover:bg-accent focus-visible:bg-accent ' +
          "after:absolute after:content-['']",
        vertical
          ? 'w-px cursor-col-resize after:inset-y-0 after:-left-1 after:w-2'
          : 'h-px cursor-row-resize after:inset-x-0 after:-top-1 after:h-2',
        className,
      )}
      data-slot="resize-handle"
    />
  );
}

/* Sidebar ----------------------------------------------------------------------------- */

/**
 * A sidebar column with a resize handle on its inner edge. Closed, it renders nothing; the
 * caller's toggle (a SidebarTrigger-like button with aria-expanded) brings it back.
 */
export function Sidebar({
  label,
  width,
  onWidthChange,
  onWidthCommit,
  min = 200,
  max = 480,
  open = true,
  side = 'left',
  resizable = true,
  className,
  children,
}: {
  /** The navigation landmark's name, for example "Threads". */
  label: string;
  width: number;
  onWidthChange: (width: number) => void;
  /** Called when a resize ends, to persist the width. */
  onWidthCommit?: (width: number) => void;
  min?: number;
  max?: number;
  open?: boolean;
  side?: 'left' | 'right';
  resizable?: boolean;
  className?: string;
  children: ReactNode;
}) {
  const id = useId();
  if (!open) return null;
  const handle = resizable && (
    <ResizeHandle
      label={`Resize ${label.toLowerCase()}`}
      value={width}
      min={min}
      max={max}
      onResize={onWidthChange}
      onResizeEnd={onWidthCommit}
      edge={side === 'left' ? 'end' : 'start'}
      controls={id}
    />
  );
  return (
    <>
      {side === 'right' && handle}
      <div
        className={cx('flex shrink-0 flex-col overflow-hidden bg-surface text-ink', className)}
        style={{ width }}
        data-side={side}
        data-slot="sidebar"
      >
        <nav id={id} aria-label={label} className="flex min-h-0 flex-1 flex-col">
          {children}
        </nav>
      </div>
      {side === 'left' && handle}
    </>
  );
}

export function SidebarHeader({ className, ...props }: ComponentProps<'div'>) {
  return (
    <div
      className={cx('flex shrink-0 flex-col gap-2 border-b border-rule-soft p-2', className)}
      data-slot="sidebar-header"
      {...props}
    />
  );
}

export function SidebarFooter({ className, ...props }: ComponentProps<'div'>) {
  return (
    <div
      className={cx('flex shrink-0 flex-col gap-2 border-t border-rule-soft p-2', className)}
      data-slot="sidebar-footer"
      {...props}
    />
  );
}

/**
 * The scrolling middle. `fixedHeader` stays put above it. Rows take focus themselves, so
 * the viewport is not a tab stop of its own.
 */
export function SidebarContent({
  className,
  fixedHeader,
  children,
}: {
  className?: string;
  fixedHeader?: ReactNode;
  children: ReactNode;
}) {
  return (
    <>
      {fixedHeader ? <div className="w-full shrink-0">{fixedHeader}</div> : null}
      <ScrollArea
        fade
        fadePadding={false}
        hideScrollbars
        viewportTabIndex={-1}
        className="h-auto min-h-0 flex-1"
      >
        <div
          // Reordered rows must not pull the viewport to their new position.
          className={cx('flex w-full min-w-0 flex-col [overflow-anchor:none]', className)}
          data-slot="sidebar-content"
        >
          {children}
        </div>
      </ScrollArea>
    </>
  );
}

/** A labelled section of rows; `action` sits at the end of the label (an add button). */
export function SidebarGroup({
  label,
  action,
  className,
  children,
}: {
  label?: ReactNode;
  action?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section
      aria-labelledby={label ? id : undefined}
      className={cx('relative flex w-full min-w-0 flex-col p-2', className)}
      data-slot="sidebar-group"
    >
      {label && (
        <div className="flex h-7 items-center justify-between gap-2 px-2">
          <h3 id={id} className="m-0 truncate text-micro font-medium text-faint">
            {label}
          </h3>
          {action}
        </div>
      )}
      {children}
    </section>
  );
}

export function SidebarMenu({ className, ...props }: ComponentProps<'ul'>) {
  return (
    <ul
      className={cx('m-0 flex w-full min-w-0 list-none flex-col gap-px p-0', className)}
      data-slot="sidebar-menu"
      {...props}
    />
  );
}

export function SidebarMenuItem({ className, ...props }: ComponentProps<'li'>) {
  return (
    <li
      className={cx('group/menu-item relative', className)}
      data-slot="sidebar-menu-item"
      {...props}
    />
  );
}

const ROW =
  'flex h-8 w-full min-w-0 cursor-pointer items-center gap-2 overflow-hidden rounded-sm border-0 ' +
  'bg-transparent px-2 text-left font-[inherit] text-body text-muted no-underline outline-none ' +
  'hover:bg-raised hover:text-ink focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent ' +
  'disabled:cursor-default disabled:opacity-50 aria-[current=page]:bg-raised aria-[current=page]:text-ink ' +
  '[&>span:last-child]:truncate [&>svg]:size-4 [&>svg]:shrink-0';

/** A row. `active` marks the current page (aria-current). Give `href` for a link row. */
export function SidebarMenuButton({
  active = false,
  href,
  className,
  children,
  ...props
}: Omit<ComponentProps<'button'>, 'type'> & {
  active?: boolean;
  href?: string;
}) {
  const shared = {
    'aria-current': active ? ('page' as const) : undefined,
    className: cx(ROW, className),
    'data-slot': 'sidebar-menu-button',
  };
  if (href !== undefined) {
    return (
      <a href={href} {...shared} onClick={props.onClick as ComponentProps<'a'>['onClick']}>
        {children}
      </a>
    );
  }
  return (
    <button type="button" {...shared} {...props}>
      {children}
    </button>
  );
}

/** Nested rows under a row (a project's threads). */
export function SidebarMenuSub({ className, ...props }: ComponentProps<'ul'>) {
  return (
    <ul
      className={cx(
        'm-0 ms-3.5 flex min-w-0 list-none flex-col gap-px border-l border-rule-soft py-0.5 ps-2',
        className,
      )}
      data-slot="sidebar-menu-sub"
      {...props}
    />
  );
}
