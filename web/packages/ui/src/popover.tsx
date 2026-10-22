// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/popover.tsx at commit 33aa4614, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to the design 59 tokens and cx, with Base UI underneath; no cva, lucide or Effect; the root is PopoverRoot and the Loams Popover wrapper keeps its props; the thread-details "panel" variant is dropped; the layer sits between dialogs (71) and toasts (80); the elevated surface and the enter and exit timing (UI1). Loams' changes are Apache-2.0.

import { Popover as PopoverPrimitive } from '@base-ui/react/popover';
import type { ReactElement, ReactNode } from 'react';
import { cx, type WithClass } from './cx';

/** Menus, popovers and preview cards: above dialogs and sheets (71), below toasts (80). */
export const OVERLAY_LAYER = 'z-[75]';

/** The surface menus, popovers and preview cards share (the corner and text size are each part's). */
export const OVERLAY_SURFACE =
  'border border-rule bg-elevated text-ink shadow-pop outline-none ' +
  // Opens from its trigger (origin-(--transform-origin)) in 160ms, closes in 110ms; transform
  // and opacity only; reduced motion snaps (UI1-1).
  'transition-[scale,opacity] duration-(--op-duration-enter) ease-out data-ending-style:duration-(--op-duration-exit) data-ending-style:ease-in motion-reduce:transition-none ' +
  'data-starting-style:scale-97 data-ending-style:scale-98 data-ending-style:opacity-0 data-starting-style:opacity-0 ' +
  '[-webkit-app-region:no-drag]';

export const PopoverCreateHandle = PopoverPrimitive.createHandle;

export const PopoverRoot = PopoverPrimitive.Root;

export function PopoverTrigger({ className, children, ...props }: PopoverPrimitive.Trigger.Props) {
  return (
    <PopoverPrimitive.Trigger className={className} data-slot="popover-trigger" {...props}>
      {children}
    </PopoverPrimitive.Trigger>
  );
}

const POPUP_WIDTH = {
  auto: '',
  sm: 'w-64',
  md: 'w-80',
  lg: 'w-96',
} as const;

const VIEWPORT_PADDING = {
  default: 'py-4 [--viewport-inline-padding:--spacing(4)]',
  compact: 'py-2 [--viewport-inline-padding:--spacing(3)]',
  none: 'rounded-[calc(var(--op-radius)-1px)] py-0 [--viewport-inline-padding:0px]',
} as const;

export function PopoverPopup({
  children,
  className,
  padding = 'default',
  width = 'auto',
  side = 'bottom',
  align = 'center',
  sideOffset = 4,
  alignOffset = 0,
  collisionAvoidance,
  tooltipStyle = false,
  keepMounted = false,
  anchor,
  ...props
}: WithClass<PopoverPrimitive.Popup.Props> & {
  padding?: keyof typeof VIEWPORT_PADDING;
  side?: PopoverPrimitive.Positioner.Props['side'];
  align?: PopoverPrimitive.Positioner.Props['align'];
  sideOffset?: PopoverPrimitive.Positioner.Props['sideOffset'];
  alignOffset?: PopoverPrimitive.Positioner.Props['alignOffset'];
  collisionAvoidance?: PopoverPrimitive.Positioner.Props['collisionAvoidance'];
  tooltipStyle?: boolean;
  keepMounted?: PopoverPrimitive.Portal.Props['keepMounted'];
  anchor?: PopoverPrimitive.Positioner.Props['anchor'];
  width?: keyof typeof POPUP_WIDTH;
}) {
  // Viewport rekeys its children when the active trigger clears on close. Persistent
  // single-trigger forms need a stable container to retain drafts and submit guards.
  const Viewport = keepMounted ? 'div' : PopoverPrimitive.Viewport;
  return (
    <PopoverPrimitive.Portal keepMounted={keepMounted}>
      <PopoverPrimitive.Positioner
        align={align}
        alignOffset={alignOffset}
        anchor={anchor}
        collisionAvoidance={collisionAvoidance}
        className={cx(
          'h-(--positioner-height) w-(--positioner-width) max-w-(--available-width) transition-transform data-instant:transition-none motion-reduce:transition-none',
          OVERLAY_LAYER,
        )}
        data-slot="popover-positioner"
        side={side}
        sideOffset={sideOffset}
      >
        <PopoverPrimitive.Popup
          className={cx(
            'relative flex h-(--popup-height,auto) w-(--popup-width,auto) origin-(--transform-origin)',
            OVERLAY_SURFACE,
            tooltipStyle ? 'w-fit text-balance rounded-sm text-small' : 'rounded-panel text-body',
            width !== 'auto' && ['max-w-[calc(100vw-2rem)]', POPUP_WIDTH[width]].join(' '),
            className,
          )}
          data-slot="popover-popup"
          {...props}
        >
          <Viewport
            className={cx(
              'relative size-full max-h-(--available-height) overflow-clip px-(--viewport-inline-padding) data-instant:transition-none',
              tooltipStyle && padding === 'default'
                ? 'py-1 [--viewport-inline-padding:--spacing(2)]'
                : VIEWPORT_PADDING[padding],
              !tooltipStyle && 'not-data-transitioning:overflow-y-auto',
            )}
            data-slot="popover-viewport"
          >
            {children}
          </Viewport>
        </PopoverPrimitive.Popup>
      </PopoverPrimitive.Positioner>
    </PopoverPrimitive.Portal>
  );
}

export function PopoverClose({ ...props }: PopoverPrimitive.Close.Props) {
  return <PopoverPrimitive.Close data-slot="popover-close" {...props} />;
}

export function PopoverTitle({ className, ...props }: WithClass<PopoverPrimitive.Title.Props>) {
  return (
    <PopoverPrimitive.Title
      className={cx('m-0 text-body font-semibold leading-none', className)}
      data-slot="popover-title"
      {...props}
    />
  );
}

/* The Loams wrapper ------------------------------------------------------------------- */

/** Non-modal content anchored to a trigger: details, a small form, a picker. */
export function Popover({
  trigger,
  title,
  children,
  align = 'start',
  open,
  onOpenChange,
}: {
  trigger: ReactElement;
  title?: ReactNode;
  children: ReactNode;
  align?: 'start' | 'center' | 'end';
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  return (
    <PopoverRoot open={open} onOpenChange={onOpenChange ? (o) => onOpenChange(o) : undefined}>
      <PopoverTrigger render={trigger} />
      <PopoverPopup align={align} sideOffset={6} padding="none" className="min-w-44">
        <div className="grid gap-2 p-3">
          {title && <PopoverTitle>{title}</PopoverTitle>}
          {children}
        </div>
      </PopoverPopup>
    </PopoverRoot>
  );
}
