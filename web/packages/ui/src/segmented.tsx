// A segmented control: a row of pills where exactly one is pressed (the console's
// Console | Studio switcher, design §53 §4.2). Base UI's toggle group gives the
// roving focus and `aria-pressed`; pressing the pressed option keeps it pressed.
// UI1: the pressed chip slides between options (SelectionIndicator; instant under reduced
// motion), with the chip shadow instead of the popover shadow.

import { Toggle } from '@base-ui/react/toggle';
import { ToggleGroup } from '@base-ui/react/toggle-group';
import type { ReactNode } from 'react';
import { cx } from './cx';
import { SelectionGroup, SelectionIndicator } from './motion';

export interface SegmentedOption<V extends string> {
  value: V;
  label: ReactNode;
  /** Shown before the label. */
  icon?: ReactNode;
  /** A tooltip and keyboard hint, for example "Studio (Ctrl+Shift+M)". */
  title?: string;
}

export function Segmented<V extends string>({
  label,
  value,
  onValueChange,
  options,
  className,
  keyShortcuts,
}: {
  /** The group's accessible name. */
  label: string;
  value: V;
  onValueChange: (value: V) => void;
  options: SegmentedOption<V>[];
  className?: string;
  /** `aria-keyshortcuts` on the group, for a shortcut that moves between options. */
  keyShortcuts?: string;
}) {
  return (
    <ToggleGroup
      aria-label={label}
      aria-keyshortcuts={keyShortcuts}
      value={[value]}
      onValueChange={(next) => {
        const picked = next[0] as V | undefined;
        if (picked !== undefined && picked !== value) onValueChange(picked);
      }}
      className={cx(
        'inline-flex items-center gap-0.5 rounded-pill border border-rule bg-raised p-0.5',
        className,
      )}
    >
      <SelectionGroup>
        {options.map((o) => (
          <Toggle
            key={o.value}
            value={o.value}
            title={o.title}
            className={
              'relative inline-flex h-7 cursor-pointer items-center gap-1.5 rounded-pill border-0 bg-transparent ' +
              'px-3 font-[inherit] text-small font-medium text-muted transition-colors duration-(--op-duration-fast) ' +
              'hover:text-ink data-pressed:text-ink motion-reduce:transition-none ' +
              'focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-focus'
            }
          >
            {o.value === value && (
              <SelectionIndicator
                layoutId="segment"
                className="inset-0 rounded-pill bg-row-selected shadow-chip"
              />
            )}
            <span className="relative inline-flex items-center gap-1.5">
              {o.icon}
              <span>{o.label}</span>
            </span>
          </Toggle>
        ))}
      </SelectionGroup>
    </ToggleGroup>
  );
}
