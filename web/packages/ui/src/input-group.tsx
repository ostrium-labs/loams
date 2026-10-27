// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/input-group.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to Loams tokens, rounded-none and rounded-pill, and cx, with Base UI underneath; no cva, lucide or Effect. Loams' changes are Apache-2.0.

// An input with things attached: a search glyph, a key hint, a clear button. The group
// draws the field's border and focus ring, so the input inside is bare.

import type { ComponentProps, InputHTMLAttributes, MouseEvent } from 'react';
import { cx } from './cx';

export function InputGroup({ className, ...props }: ComponentProps<'div'>) {
  return (
    // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role -- a fieldset is a form section with a legend; this is one composite control
    <div
      role="group"
      className={cx(
        'relative inline-flex h-9 w-full min-w-0 items-center border border-rule bg-bg text-ink ' +
          'transition-colors duration-150 ease-op ' +
          'has-[input:focus-visible]:outline-2 has-[input:focus-visible]:-outline-offset-1 has-[input:focus-visible]:outline-accent ' +
          'has-[input[aria-invalid=true]]:border-danger has-[input:disabled]:opacity-50',
        className,
      )}
      data-slot="input-group"
      {...props}
    />
  );
}

const INTERACTIVE =
  "button, a, input, select, textarea, [role='button'], [role='combobox'], [role='listbox']";

/**
 * A glyph, a hint or a control before or after the input. A press on anything that is not
 * itself interactive focuses the input, the way a click on a field's padding would.
 */
export function InputGroupAddon({
  className,
  align = 'start',
  onMouseDown,
  ...props
}: ComponentProps<'div'> & { align?: 'start' | 'end' }) {
  return (
    // oxlint-disable-next-line jsx-a11y/no-static-element-interactions -- the press only forwards focus to the input, which is the keyboard path
    <div
      className={cx(
        'flex h-full shrink-0 cursor-text select-none items-center gap-1.5 text-faint',
        align === 'start' ? 'order-first ps-2.5' : 'order-last pe-2',
        className,
      )}
      data-align={align}
      data-slot="input-group-addon"
      onMouseDown={(e: MouseEvent<HTMLDivElement>) => {
        onMouseDown?.(e);
        if (e.defaultPrevented) return;
        if ((e.target as HTMLElement).closest(INTERACTIVE)) return;
        e.preventDefault();
        const parent = e.currentTarget.parentElement;
        const input = parent?.querySelector<HTMLInputElement | HTMLTextAreaElement>(
          'input, textarea',
        );
        if (input && !parent?.querySelector('input:focus, textarea:focus')) input.focus();
      }}
      {...props}
    />
  );
}

export function InputGroupInput({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cx(
        'h-full min-w-0 flex-1 border-0 bg-transparent px-2.5 font-[inherit] text-body text-ink ' +
          'outline-none placeholder:text-faint',
        className,
      )}
      data-slot="input-group-input"
      {...props}
    />
  );
}
