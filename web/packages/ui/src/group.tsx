// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/group.tsx at commit 33aa4614, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to the design 59 tokens and cx, with Base UI underneath; no cva, lucide or Effect; exported as ButtonGroup (upstream's alias). Loams' changes are Apache-2.0.

import type * as React from 'react';
import { cx } from './cx';
import { Separator } from './separator';

const ORIENTATION = {
  horizontal:
    '*:data-slot:has-[~[data-slot]]:rounded-e-none *:data-slot:has-[~[data-slot]]:border-e-0 ' +
    '*:[[data-slot]~[data-slot]]:rounded-s-none *:[[data-slot]~[data-slot]]:border-s-0',
  vertical:
    'flex-col *:data-slot:has-[~[data-slot]]:rounded-b-none *:data-slot:has-[~[data-slot]]:border-b-0 ' +
    '*:[[data-slot]~[data-slot]]:rounded-t-none *:[[data-slot]~[data-slot]]:border-t-0',
} as const;

/** Buttons and inputs joined edge to edge. Name it with aria-label. */
export function ButtonGroup({
  className,
  orientation = 'horizontal',
  children,
  ...props
}: {
  className?: string;
  orientation?: keyof typeof ORIENTATION;
  children: React.ReactNode;
} & React.ComponentProps<'div'>) {
  return (
    // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- a fieldset is a form section with a legend; this is a row of joined controls
    <div
      className={cx(
        'flex w-fit *:focus-visible:z-1 *:has-focus-visible:z-1 has-[>[data-slot=group]]:gap-2',
        ORIENTATION[orientation],
        className,
      )}
      data-orientation={orientation}
      data-slot="group"
      role="group"
      {...props}
    >
      {children}
    </div>
  );
}

/** A hairline between joined controls. */
export function ButtonGroupSeparator({
  className,
  orientation = 'vertical',
  ...props
}: { className?: string } & React.ComponentProps<typeof Separator>) {
  return (
    <Separator
      className={cx('pointer-events-none relative z-2 bg-input', className)}
      orientation={orientation}
      {...props}
    />
  );
}
