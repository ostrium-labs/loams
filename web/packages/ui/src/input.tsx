// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/input.tsx at commit 33aa4614, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to the design 59 tokens and cx, with Base UI underneath; no cva, lucide or Effect; exported as InputControl (Input is field.tsx's labelled input); hover, invalid-focus and disabled states (UI1); one height class per size. Loams' changes are Apache-2.0.

import { Input as InputPrimitive } from '@base-ui/react/input';
import type * as React from 'react';
import { cx, type WithClass } from './cx';

/** The outlined box every text control shares (inputs, textareas, number fields). */
export const CONTROL_BOX =
  'relative inline-flex w-full rounded-md border border-input bg-bg text-body text-ink ' +
  'transition-colors duration-(--op-duration-fast) motion-reduce:transition-none ' +
  'hover:not-has-disabled:not-has-aria-invalid:border-muted ' +
  'has-focus-visible:outline-2 has-focus-visible:outline-offset-2 has-focus-visible:outline-focus ' +
  'has-aria-invalid:border-danger has-aria-invalid:has-focus-visible:outline-danger ' +
  'has-disabled:cursor-not-allowed has-disabled:opacity-60';

export type InputControlProps = Omit<
  WithClass<InputPrimitive.Props> & React.RefAttributes<HTMLInputElement>,
  'size'
> & {
  size?: 'sm' | 'compact' | 'default' | 'lg' | number;
  font?: 'default' | 'mono';
  unstyled?: boolean;
  nativeInput?: boolean;
};

/** A text input in its outlined box. `nativeInput` renders a plain <input> for render props. */
export function InputControl({
  className,
  size = 'default',
  font = 'default',
  unstyled = false,
  nativeInput = false,
  ...props
}: InputControlProps) {
  const inputClassName = cx(
    // The console has no preflight: the input takes the box's face, size and colour itself.
    // One height per size: cx does not merge classes, so h-9 beside h-7 would win by CSS order.
    'w-full min-w-0 rounded-[inherit] border-0 bg-transparent font-[inherit] text-inherit outline-none placeholder:text-faint',
    size === 'compact' || size === 'sm'
      ? 'h-7 px-2.5 text-small leading-7'
      : size === 'lg'
        ? 'h-10 px-3 leading-10 text-[length:inherit]'
        : 'h-9 px-3 leading-9 text-[length:inherit]',
    props.type === 'search' &&
      '[&::-webkit-search-cancel-button]:appearance-none [&::-webkit-search-decoration]:appearance-none [&::-webkit-search-results-button]:appearance-none [&::-webkit-search-results-decoration]:appearance-none',
    props.type === 'number' &&
      '[appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none',
    props.type === 'file' &&
      'text-muted file:me-3 file:bg-transparent file:font-medium file:text-ink file:text-body',
  );
  let inputElement: React.ReactElement;

  if (nativeInput) {
    const { style, onValueChange: _onValueChange, ...nativeInputProps } = props;
    const nativeStyle = typeof style === 'function' ? undefined : style;

    inputElement = (
      <input
        className={inputClassName}
        data-slot="input"
        size={typeof size === 'number' ? size : undefined}
        style={nativeStyle}
        {...(nativeInputProps as React.ComponentProps<'input'>)}
      />
    );
  } else {
    inputElement = (
      <InputPrimitive
        className={inputClassName}
        data-slot="input"
        size={typeof size === 'number' ? size : undefined}
        {...props}
      />
    );
  }

  return (
    <span
      className={
        cx(
          !unstyled && CONTROL_BOX,
          !unstyled && (size === 'compact' || size === 'sm') && 'rounded-sm',
          font === 'mono' && /* mono-ok: code */ 'font-mono tabular-nums',
          className,
        ) || undefined
      }
      data-size={size}
      data-slot="input-control"
    >
      {inputElement}
    </span>
  );
}
