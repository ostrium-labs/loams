// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/switch.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to Loams tokens, rounded-none and rounded-pill, and cx, with Base UI underneath; no cva, lucide or Effect. Loams' changes are Apache-2.0.

// An on/off switch for settings that apply at once. A form that is submitted keeps the
// native Checkbox (field.tsx).

import { Switch as BaseSwitch } from '@base-ui/react/switch';
import { type ReactNode, useId } from 'react';
import { cx } from './cx';

/**
 * `mixed` shows the thumb centred on a muted track for a selection whose targets disagree
 * (the macOS mixed-state convention). It is presentational: the caller still decides what a
 * click sets, usually on for everyone.
 */
export function Switch({
  checked,
  onCheckedChange,
  label,
  hideLabel = false,
  mixed = false,
  disabled,
  size = 'md',
  className,
}: {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  /** The switch's name; shown beside it unless `hideLabel`. */
  label: ReactNode;
  hideLabel?: boolean;
  mixed?: boolean;
  disabled?: boolean;
  size?: 'sm' | 'md';
  className?: string;
}) {
  const id = useId();
  return (
    <span className={cx('inline-flex items-center gap-2', className)}>
      <BaseSwitch.Root
        id={id}
        checked={checked}
        onCheckedChange={(next) => onCheckedChange(next)}
        disabled={disabled}
        aria-labelledby={`${id}-label`}
        className={cx(
          'inline-flex shrink-0 cursor-pointer items-center rounded-pill border border-rule p-px ' +
            'transition-colors duration-150 ease-op focus-visible:outline-2 focus-visible:outline-accent ' +
            'data-checked:border-grow data-checked:bg-grow data-unchecked:bg-raised ' +
            'data-disabled:cursor-not-allowed data-disabled:opacity-50 data-[mixed]:bg-raised',
          size === 'sm' ? 'h-4 w-7' : 'h-5 w-9',
        )}
        data-slot="switch"
        data-mixed={mixed ? '' : undefined}
        // Base UI copies every key it is given, even `undefined`, over its own aria-checked.
        // Pass it only when mixed, so the real state survives for screen readers.
        {...(mixed ? { 'aria-checked': 'mixed' as const } : {})}
      >
        <BaseSwitch.Thumb
          className={cx(
            'pointer-events-none block rounded-pill bg-bg shadow-pop transition-transform duration-150 ease-op',
            size === 'sm'
              ? 'size-3.5 data-checked:translate-x-3'
              : 'size-4.5 data-checked:translate-x-4',
            mixed && (size === 'sm' ? 'translate-x-1.5!' : 'translate-x-2!'),
          )}
        />
      </BaseSwitch.Root>
      <span id={`${id}-label`} className={cx('text-body text-ink', hideLabel && 'sr-only')}>
        {label}
      </span>
    </span>
  );
}
