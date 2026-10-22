// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/tooltip.tsx at commit 33aa4614, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to the design 59 tokens and cx, with Base UI underneath; no cva, lucide or Effect; the root is TooltipRoot and the Loams Tooltip wrapper keeps its props; the window-controls-overlay padding is a constant 5 and the glass variant is dropped; the layer sits above menus (75) and below toasts (80). Loams' changes are Apache-2.0.

import { Tooltip as TooltipPrimitive } from '@base-ui/react/tooltip';
import {
  type ComponentProps,
  createContext,
  type ReactElement,
  type ReactNode,
  type RefObject,
  use,
  useEffect,
  useRef,
} from 'react';
import { cx, type WithClass } from './cx';

// Base UI's default, restated because a padding object zeroes the sides it omits.
const COLLISION_PADDING = 5;

/** Shares hover delays between tooltips; the shell mounts it once. */
export const TooltipProvider = TooltipPrimitive.Provider;

type TooltipActionsRef = RefObject<TooltipPrimitive.Root.Actions | null>;
const TooltipHoverContext = createContext<TooltipActionsRef | null>(null);
const TooltipScrollContext = createContext<RefObject<{
  trigger: HTMLElement;
  actionsRef: TooltipActionsRef;
} | null> | null>(null);

/** Closes a hovered tooltip when the area under it scrolls. */
export function TooltipScrollDismissArea({ onScrollCapture, ...props }: ComponentProps<'div'>) {
  const hovered = useRef<{ trigger: HTMLElement; actionsRef: TooltipActionsRef } | null>(null);
  return (
    <TooltipScrollContext value={hovered}>
      <div
        {...props}
        onScrollCapture={(event) => {
          onScrollCapture?.(event);
          const tooltip = hovered.current;
          if (!tooltip || tooltip.trigger.contains(tooltip.trigger.ownerDocument.activeElement)) {
            return;
          }
          hovered.current = null;
          // Base UI also cancels delayed hover opens through this action.
          tooltip.actionsRef.current?.close();
        }}
      />
    </TooltipScrollContext>
  );
}

export function TooltipRoot<Payload>(props: TooltipPrimitive.Root.Props<Payload>) {
  const hovered = use(TooltipScrollContext);
  const localActionsRef = useRef<TooltipPrimitive.Root.Actions | null>(null);
  const actionsRef = props.actionsRef ?? localActionsRef;
  useEffect(
    () => () => {
      if (hovered?.current?.actionsRef === actionsRef) hovered.current = null;
    },
    [actionsRef, hovered],
  );

  if (!hovered) return <TooltipPrimitive.Root {...props} />;
  return (
    <TooltipHoverContext value={actionsRef}>
      <TooltipPrimitive.Root
        {...props}
        actionsRef={actionsRef}
        onOpenChange={(open, details) => {
          props.onOpenChange?.(open, details);
          if (!open && !details.isCanceled && hovered.current?.actionsRef === actionsRef) {
            hovered.current = null;
          }
        }}
      />
    </TooltipHoverContext>
  );
}

export function TooltipTrigger(props: TooltipPrimitive.Trigger.Props) {
  const hovered = use(TooltipScrollContext);
  const actionsRef = use(TooltipHoverContext);
  if (!hovered || !actionsRef) {
    return <TooltipPrimitive.Trigger data-slot="tooltip-trigger" {...props} />;
  }
  return (
    <TooltipPrimitive.Trigger
      data-slot="tooltip-trigger"
      {...props}
      onMouseEnter={(event) => {
        props.onMouseEnter?.(event);
        hovered.current = { trigger: event.currentTarget, actionsRef };
      }}
    />
  );
}

function TooltipPositioner(props: TooltipPrimitive.Positioner.Props) {
  return (
    <TooltipPrimitive.Positioner
      className="pointer-events-none z-[76] h-(--positioner-height) w-(--positioner-width) max-w-(--available-width) transition-[top,left,right,bottom,transform] data-instant:transition-none motion-reduce:transition-none"
      collisionPadding={COLLISION_PADDING}
      data-slot="tooltip-positioner"
      {...props}
    />
  );
}

export function TooltipPopup({
  className,
  align = 'center',
  sideOffset = 4,
  side = 'top',
  variant = 'default',
  anchor,
  children,
  ...props
}: WithClass<TooltipPrimitive.Popup.Props> & {
  align?: TooltipPrimitive.Positioner.Props['align'];
  side?: TooltipPrimitive.Positioner.Props['side'];
  sideOffset?: TooltipPrimitive.Positioner.Props['sideOffset'];
  /** `code` sets the text in the code face, for a tooltip that shows code or an id. */
  variant?: 'default' | 'code';
  anchor?: TooltipPrimitive.Positioner.Props['anchor'];
}) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPositioner align={align} anchor={anchor} side={side} sideOffset={sideOffset}>
        <TooltipPrimitive.Popup
          className={cx(
            'relative flex h-(--popup-height,auto) w-(--popup-width,auto) origin-(--transform-origin) text-balance rounded-sm bg-ink text-small text-bg shadow-pop ' +
              'transition-[width,height,scale,opacity] duration-(--op-duration-fast) ease-out data-ending-style:duration-(--op-duration-exit) data-ending-style:ease-in motion-reduce:transition-none ' +
              'data-ending-style:scale-98 data-starting-style:scale-98 data-ending-style:opacity-0 data-starting-style:opacity-0 data-instant:duration-0',
            variant === 'code'
              ? 'max-w-120 wrap-anywhere text-left font-mono leading-relaxed' // mono-ok: code
              : 'max-w-80 wrap-anywhere whitespace-normal leading-snug',
            className,
          )}
          data-slot="tooltip-popup"
          {...props}
        >
          <TooltipPrimitive.Viewport
            className="relative size-full overflow-clip px-(--viewport-inline-padding) py-1 [--viewport-inline-padding:--spacing(2)] data-instant:transition-none **:data-current:data-ending-style:opacity-0 **:data-current:data-starting-style:opacity-0 **:data-previous:data-ending-style:opacity-0 **:data-previous:data-starting-style:opacity-0 **:data-current:w-[calc(var(--popup-width)-2*var(--viewport-inline-padding)-2px)] **:data-previous:w-[calc(var(--popup-width)-2*var(--viewport-inline-padding)-2px)] **:data-previous:truncate **:data-current:opacity-100 **:data-previous:opacity-100 **:data-current:transition-opacity **:data-previous:transition-opacity"
            data-slot="tooltip-viewport"
          >
            {children}
          </TooltipPrimitive.Viewport>
        </TooltipPrimitive.Popup>
      </TooltipPositioner>
    </TooltipPrimitive.Portal>
  );
}

/* The Loams wrapper ------------------------------------------------------------------- */

/** A short label on hover or focus. Never the only place a control's name lives. */
export function Tooltip({ content, children }: { content: ReactNode; children: ReactElement }) {
  return (
    <TooltipRoot>
      <TooltipTrigger render={children} />
      <TooltipPopup>{content}</TooltipPopup>
    </TooltipRoot>
  );
}
