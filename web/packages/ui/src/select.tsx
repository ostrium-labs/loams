// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/select.tsx at commit 33aa4614, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to the design 59 tokens and cx, with Base UI underneath; no cva, lucide or Effect; the root is SelectRoot (Select is field.tsx's native select). Loams' changes are Apache-2.0.

import { mergeProps } from '@base-ui/react/merge-props';
import { Select as SelectPrimitive } from '@base-ui/react/select';
import { useRender } from '@base-ui/react/use-render';
import type { ReactNode } from 'react';
import { nativeButton } from './button';
import { cx, type WithClass } from './cx';

export const SelectRoot = SelectPrimitive.Root;

export const ChevronDown = ({ className }: { className?: string }) => (
  <svg
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    strokeWidth={2}
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
    className={className}
  >
    <path d="M4 6l4 4 4-4" />
  </svg>
);

const ChevronUp = ({ className }: { className?: string }) => (
  <svg
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    strokeWidth={2}
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
    className={className}
  >
    <path d="M4 10l4-4 4 4" />
  </svg>
);

const TRIGGER_SIZE = {
  compact: 'h-7 gap-1 px-2 text-small',
  default: 'h-8 px-3',
  lg: 'h-9 px-3',
} as const;
const TRIGGER_VARIANT = {
  default:
    'w-full min-w-36 border-input bg-bg text-ink hover:border-muted data-popup-open:border-muted',
  ghost:
    'border-transparent bg-transparent text-muted hover:bg-raised hover:text-ink data-pressed:bg-raised',
} as const;

const TRIGGER_BASE =
  'relative inline-flex cursor-pointer select-none items-center justify-between gap-2 rounded-md border text-left text-body outline-none ' +
  'transition-colors duration-(--op-duration-fast) motion-reduce:transition-none ' +
  'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus ' +
  'aria-invalid:border-danger data-disabled:pointer-events-none data-disabled:opacity-60 ' +
  '[&_svg]:pointer-events-none [&_svg]:shrink-0';

export type SelectTriggerSize = keyof typeof TRIGGER_SIZE;
export type SelectTriggerVariant = keyof typeof TRIGGER_VARIANT;

export function SelectTrigger({
  className,
  size = 'default',
  variant = 'default',
  unstyled = false,
  children,
  icon,
  ...props
}: WithClass<SelectPrimitive.Trigger.Props> & {
  size?: SelectTriggerSize;
  variant?: SelectTriggerVariant;
  /** Leaves the look to `className` alone (SelectMenu's callers that bring their own). */
  unstyled?: boolean;
  icon?: ReactNode;
}) {
  return (
    <SelectPrimitive.Trigger
      className={
        unstyled
          ? className
          : cx(TRIGGER_BASE, TRIGGER_VARIANT[variant], TRIGGER_SIZE[size], className)
      }
      data-slot="select-trigger"
      {...props}
    >
      {children}
      <SelectPrimitive.Icon className="flex" data-slot="select-icon">
        {icon ?? <ChevronDown className="-me-1 size-3 text-muted" />}
      </SelectPrimitive.Icon>
    </SelectPrimitive.Trigger>
  );
}

/** A button that looks like a select trigger, for pickers that open something else. */
export function SelectButton({
  className,
  size = 'default',
  children,
  render,
  ...props
}: useRender.ComponentProps<'button'> & { size?: SelectTriggerSize }) {
  const defaults = {
    className: cx(TRIGGER_BASE, TRIGGER_VARIANT.default, TRIGGER_SIZE[size], className),
    'data-slot': 'select-trigger',
    type: nativeButton(render) ? ('button' as const) : undefined,
  };
  return useRender({
    defaultTagName: 'button',
    props: {
      ...mergeProps<'button'>(defaults, props),
      children: (
        <>
          <span className="min-w-0 flex-1 truncate text-left">{children}</span>
          <ChevronDown className="-me-1 size-3 shrink-0 text-muted" />
        </>
      ),
    },
    render,
  });
}

export function SelectValue({ className, ...props }: WithClass<SelectPrimitive.Value.Props>) {
  return (
    <SelectPrimitive.Value
      className={cx('flex-1 truncate data-placeholder:text-faint', className)}
      data-slot="select-value"
      {...props}
    />
  );
}

/** The popup surface every picker shares (select, combobox, autocomplete). */
export const POPUP_SURFACE =
  'rounded-panel border border-rule bg-elevated text-ink shadow-pop ' +
  'transition-opacity duration-(--op-duration-enter) ease-out data-ending-style:duration-(--op-duration-exit) data-ending-style:ease-in motion-reduce:transition-none';

export function SelectPopup({
  className,
  children,
  side = 'bottom',
  sideOffset = 4,
  align = 'start',
  alignOffset = 0,
  alignItemWithTrigger = true,
  matchTriggerWidth = true,
  anchor,
  ...props
}: WithClass<SelectPrimitive.Popup.Props> & {
  side?: SelectPrimitive.Positioner.Props['side'];
  sideOffset?: SelectPrimitive.Positioner.Props['sideOffset'];
  align?: SelectPrimitive.Positioner.Props['align'];
  alignOffset?: SelectPrimitive.Positioner.Props['alignOffset'];
  alignItemWithTrigger?: SelectPrimitive.Positioner.Props['alignItemWithTrigger'];
  matchTriggerWidth?: boolean;
  anchor?: SelectPrimitive.Positioner.Props['anchor'];
}) {
  return (
    <SelectPrimitive.Portal>
      <SelectPrimitive.Positioner
        align={align}
        alignItemWithTrigger={alignItemWithTrigger}
        alignOffset={alignOffset}
        anchor={anchor}
        className="z-[75] select-none"
        data-slot="select-positioner"
        side={side}
        sideOffset={sideOffset}
      >
        <SelectPrimitive.Popup
          className="origin-(--transform-origin) rounded-panel text-ink outline-none"
          data-slot="select-popup"
          {...props}
        >
          <SelectPrimitive.ScrollUpArrow
            className="top-0 z-50 flex h-6 w-full cursor-default items-center justify-center rounded-t-panel bg-surface"
            data-slot="select-scroll-up-arrow"
          >
            <ChevronUp className="relative size-4" />
          </SelectPrimitive.ScrollUpArrow>
          <div
            className={cx(
              'relative h-full',
              POPUP_SURFACE,
              matchTriggerWidth && 'min-w-(--anchor-width)',
            )}
          >
            <SelectPrimitive.List
              className={cx('max-h-(--available-height) overflow-y-auto p-1', className)}
              data-slot="select-list"
            >
              {children}
            </SelectPrimitive.List>
          </div>
          <SelectPrimitive.ScrollDownArrow
            className="bottom-0 z-50 flex h-6 w-full cursor-default items-center justify-center rounded-b-panel bg-surface"
            data-slot="select-scroll-down-arrow"
          >
            <ChevronDown className="relative size-4" />
          </SelectPrimitive.ScrollDownArrow>
        </SelectPrimitive.Popup>
      </SelectPrimitive.Positioner>
    </SelectPrimitive.Portal>
  );
}

/** One row in a picker's popup. */
export const POPUP_ITEM =
  'flex min-h-7 cursor-default items-center gap-2 rounded-sm px-2 py-1 text-body outline-none ' +
  'data-selected:bg-row-selected data-highlighted:bg-raised data-disabled:pointer-events-none data-disabled:opacity-50 ' +
  '[&_svg]:pointer-events-none [&_svg]:shrink-0';

export function SelectItem({
  className,
  children,
  hideIndicator: _hideIndicator = false,
  ...props
}: WithClass<SelectPrimitive.Item.Props> & { hideIndicator?: boolean }) {
  return (
    <SelectPrimitive.Item
      className={cx(
        POPUP_ITEM,
        'in-data-[side=none]:min-w-[calc(var(--anchor-width)+1.25rem)]',
        className,
      )}
      data-slot="select-item"
      {...props}
    >
      <SelectPrimitive.ItemText
        className="min-w-0 flex-1 [&_svg]:text-muted"
        data-slot="select-item-text"
      >
        {children}
      </SelectPrimitive.ItemText>
    </SelectPrimitive.Item>
  );
}

export function SelectGroup(props: WithClass<SelectPrimitive.Group.Props>) {
  return <SelectPrimitive.Group data-slot="select-group" {...props} />;
}

export const GROUP_LABEL = 'px-2 py-1.5 text-small font-medium text-muted';

export function SelectGroupLabel({
  className,
  ...props
}: WithClass<SelectPrimitive.GroupLabel.Props>) {
  return (
    <SelectPrimitive.GroupLabel
      className={cx(GROUP_LABEL, className)}
      data-slot="select-group-label"
      {...props}
    />
  );
}

/* The Loams wrapper ------------------------------------------------------------------- */

export interface Option<V extends string = string> {
  value: V;
  label: ReactNode;
  disabled?: boolean;
}

/**
 * A select on Base UI: a styled list that matches the menus, for choices that need
 * more than plain text. The native `Select` (field.tsx) stays for plain forms.
 */
export function SelectMenu<V extends string>({
  value,
  onValueChange,
  options,
  label,
  placeholder = 'Choose…',
  className,
  leading,
  icon,
}: {
  value: V | null;
  onValueChange: (value: V) => void;
  options: Option<V>[];
  /** The visible or accessible label. */
  label: string;
  placeholder?: string;
  /** Classes for the trigger, in place of the default control look. */
  className?: string;
  /** Shown before the value in the trigger (a mark, a dot). */
  leading?: ReactNode;
  /** The trigger's affordance; a chevron by default. */
  icon?: ReactNode;
}) {
  return (
    <SelectRoot
      value={value}
      onValueChange={(v) => v !== null && onValueChange(v as V)}
      items={options.map((o) => ({ value: o.value, label: o.label }))}
    >
      <SelectTrigger
        className={className}
        unstyled={className !== undefined}
        size="lg"
        aria-label={label}
        icon={
          <span className="flex text-faint" aria-hidden="true">
            {icon ?? <ChevronDown className="size-3" />}
          </span>
        }
      >
        {leading}
        <SelectValue placeholder={placeholder} className="min-w-0 text-left" />
      </SelectTrigger>
      <SelectPopup sideOffset={4} alignItemWithTrigger={false}>
        {options.map((o) => (
          <SelectItem key={o.value} value={o.value} disabled={o.disabled}>
            {o.label}
          </SelectItem>
        ))}
      </SelectPopup>
    </SelectRoot>
  );
}
