// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/kbd.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to Loams tokens, rounded-none and rounded-pill, and cx, with Base UI underneath; no cva, lucide or Effect. Loams' changes are Apache-2.0.

// A key and a chord of keys, for shortcut hints in menus, the palette and tooltips.
// The group is itself a <kbd>, so a chord reads as one input (HTML's nested-kbd form).

import type { ComponentProps } from 'react';
import { cx } from './cx';

export function Kbd({ className, ...props }: ComponentProps<'kbd'>) {
  return (
    <kbd
      className={cx(
        'pointer-events-none inline-flex h-5 min-w-5 select-none items-center justify-center gap-1 ' +
          'rounded-sm border border-rule-soft bg-surface px-1 font-sans text-micro text-muted',
        className,
      )}
      data-slot="kbd"
      {...props}
    />
  );
}

export function KbdGroup({ className, ...props }: ComponentProps<'kbd'>) {
  return (
    <kbd
      className={cx('inline-flex items-center gap-1 font-sans', className)}
      data-slot="kbd-group"
      {...props}
    />
  );
}
