// One row in a list, a sidebar section or a picker (UI upgrade phase 1). Not vendored: the
// T3 Code sidebar rows (sidebar.tsx) stay as they are; this is the general row for pages.

import { mergeProps } from '@base-ui/react/merge-props';
import { useRender } from '@base-ui/react/use-render';
import type { MouseEvent, ReactNode } from 'react';
import { cx } from './cx';

const ignorePress = (event: MouseEvent) => {
  event.preventDefault();
  event.stopPropagation();
};

export type ListRowProps = useRender.ComponentProps<'div'> & {
  /** `compact` is the 28px sidebar row at 12px; `default` is 32px at 13px. */
  density?: 'default' | 'compact';
  /**
   * Paints the row as selected. Say so to assistive technology too: `aria-current="page"` on a
   * link to the open page, `aria-selected` on an option; those also paint it.
   */
  selected?: boolean;
  disabled?: boolean;
  /** An icon before the label (lucide-react, 16px; 14px when compact). */
  icon?: ReactNode;
  /** Trailing detail: a count, a time, a shortcut or a badge. */
  meta?: ReactNode;
};

/**
 * A one-line row: icon, a label that truncates, and trailing meta. Render it as the element it
 * is (`render={<a href=… />}`, `render={<button type="button" />}`) so the keyboard and the
 * role come from that element. States: hover, selected, focus-visible (an inset ring), disabled.
 */
export function ListRow({
  className,
  density = 'default',
  selected = false,
  disabled = false,
  icon,
  meta,
  children,
  render,
  ...props
}: ListRowProps) {
  const defaults = {
    className: cx('loams-row', className),
    'data-slot': 'list-row',
    'data-density': density,
    'data-selected': selected ? '' : undefined,
    'data-disabled': disabled ? '' : undefined,
    'aria-disabled': disabled || undefined,
    children: (
      <>
        {icon}
        <span className="loams-row-label">{children}</span>
        {meta !== undefined && <span className="loams-row-meta">{meta}</span>}
      </>
    ),
  };
  const merged = mergeProps<'div'>(defaults, props);
  // A disabled row ignores presses: a caller's onClick (merged above) never runs, and a link
  // row does not follow its href.
  if (disabled) merged.onClick = ignorePress;
  return useRender({ defaultTagName: 'div', props: merged, render });
}
