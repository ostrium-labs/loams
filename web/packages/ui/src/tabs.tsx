// Tabs on Base UI, styled with the token utilities (docs/design/console-ui-stack.md,
// decision 2). Moved out of overlay.tsx unchanged in CF1 Task 5; not vendored (T3 Code
// has no tabs primitive). UI1: the type scale, a focus ring, and an underline that slides
// to the selected tab (SelectionIndicator; instant under reduced motion).

import { Tabs as BaseTabs } from '@base-ui/react/tabs';
import type { ReactNode } from 'react';
import { SelectionGroup, SelectionIndicator } from './motion';

export interface Tab<V extends string = string> {
  value: V;
  label: ReactNode;
  panel?: ReactNode;
}

/**
 * Tabs with arrow-key movement between them. Give each tab a `panel`, or leave the
 * panels out and render the active view yourself (for tabs that are routes).
 */
export function Tabs<V extends string>({
  value,
  onValueChange,
  tabs,
  label,
}: {
  value: V;
  onValueChange: (value: V) => void;
  tabs: Tab<V>[];
  label: string;
}) {
  return (
    <BaseTabs.Root value={value} onValueChange={(v) => onValueChange(v as V)}>
      <SelectionGroup>
        <BaseTabs.List className="relative flex gap-1 border-b border-rule" aria-label={label}>
          {tabs.map((t) => (
            <BaseTabs.Tab
              key={t.value}
              value={t.value}
              className={
                'relative -mb-px inline-flex h-9 cursor-pointer items-center rounded-t-sm border-0 bg-transparent px-3 ' +
                'font-[inherit] text-body font-medium text-muted transition-colors duration-(--op-duration-fast) ' +
                'hover:text-ink data-active:text-ink motion-reduce:transition-none ' +
                'focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus'
              }
            >
              {t.label}
              {t.value === value && (
                <SelectionIndicator
                  layoutId="tab"
                  className="inset-x-2 bottom-0 h-0.5 rounded-pill bg-accent"
                />
              )}
            </BaseTabs.Tab>
          ))}
        </BaseTabs.List>
      </SelectionGroup>
      {tabs.map(
        (t) =>
          t.panel !== undefined && (
            <BaseTabs.Panel key={t.value} value={t.value} className="pt-4 outline-none">
              {t.panel}
            </BaseTabs.Panel>
          ),
      )}
    </BaseTabs.Root>
  );
}
