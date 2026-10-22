// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/toggle-group.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to Loams tokens, rounded-none and rounded-pill, and cx, with Base UI underneath; no cva, lucide or Effect. Loams' changes are Apache-2.0.

// Toggle buttons: a lone Toggle (pin, wrap lines) or a ToggleGroup of them, with roving
// focus and aria-pressed from Base UI. T3 Code's toggle.tsx is folded in here. Its
// "segmented" variant is not: an exactly-one-pressed pill row is @loams/ui's Segmented.

import { Toggle as BaseToggle } from '@base-ui/react/toggle';
import { ToggleGroup as BaseToggleGroup } from '@base-ui/react/toggle-group';
import { type ComponentProps, createContext, type ReactNode, use } from 'react';
import { cx } from './cx';

type Size = 'sm' | 'md';
const SizeContext = createContext<Size>('md');

const TOGGLE =
  'inline-flex shrink-0 cursor-pointer select-none items-center justify-center gap-1.5 rounded-pill ' +
  'border border-transparent bg-transparent text-muted transition-colors duration-150 ease-op ' +
  'hover:bg-raised hover:text-ink focus-visible:outline-2 focus-visible:outline-accent ' +
  'data-pressed:border-rule data-pressed:bg-bg data-pressed:text-ink ' +
  'data-disabled:cursor-not-allowed data-disabled:opacity-50';
const SIZES: Record<Size, string> = {
  sm: 'h-7 min-w-7 px-2 text-small',
  md: 'h-8 min-w-8 px-2.5 text-body',
};

export function ToggleGroup<V extends string>({
  label,
  value,
  onValueChange,
  multiple = false,
  orientation = 'horizontal',
  size = 'md',
  disabled,
  className,
  children,
}: {
  /** The group's accessible name. */
  label: string;
  value: V[];
  onValueChange: (value: V[]) => void;
  /** More than one toggle may be pressed; otherwise pressing one releases the others. */
  multiple?: boolean;
  orientation?: 'horizontal' | 'vertical';
  size?: Size;
  disabled?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <BaseToggleGroup
      aria-label={label}
      value={value}
      onValueChange={(next) => onValueChange(next as V[])}
      multiple={multiple}
      orientation={orientation}
      disabled={disabled}
      className={cx(
        'inline-flex w-fit gap-0.5',
        orientation === 'vertical' && 'flex-col',
        className,
      )}
      data-slot="toggle-group"
    >
      <SizeContext value={size}>{children}</SizeContext>
    </BaseToggleGroup>
  );
}

/** A toggle button. In a ToggleGroup give it a `value`; alone, use `pressed`. */
export function Toggle({
  className,
  size,
  ...props
}: Omit<ComponentProps<typeof BaseToggle>, 'className'> & { className?: string; size?: Size }) {
  const inherited = use(SizeContext);
  return (
    <BaseToggle
      className={cx(TOGGLE, SIZES[size ?? inherited], className)}
      data-slot="toggle"
      {...props}
    />
  );
}
