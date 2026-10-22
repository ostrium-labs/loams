// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/ui/button.tsx at commit 33aa4614, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/ui/LICENSE.t3code). Modified by The Loams Authors:
// the cva table became the Loams .loams-btn classes (painted in styles.css on the design 59 tokens), so buttonClass keeps working outside React; Loams variant and size names kept; InlineButton dropped; a loading state and IconButton added (UI1). Loams' changes are Apache-2.0.

import { mergeProps } from '@base-ui/react/merge-props';
import { useRender } from '@base-ui/react/use-render';
import { isValidElement, type MouseEvent, type ReactElement, type ReactNode } from 'react';
import { cx } from './cx';
import { Spinner } from './spinner';
import { Tooltip } from './tooltip';

export type ButtonVariant = 'primary' | 'secondary' | 'quiet' | 'danger' | 'danger-solid' | 'link';
export type ButtonSize = 'xs' | 'sm' | 'md' | 'lg' | 'icon' | 'icon-sm' | 'icon-xs';

/** The class list for a button, for links styled as buttons and callers outside React. */
export function buttonClass({
  variant = 'secondary',
  size = 'md',
  className,
}: {
  variant?: ButtonVariant;
  size?: ButtonSize;
  className?: string;
} = {}): string {
  return cx('loams-btn', `loams-btn-${variant}`, size !== 'md' && `loams-btn-${size}`, className);
}

export type ButtonProps = useRender.ComponentProps<'button'> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /**
   * Working on the action: a spinner replaces the leading icon, the label stays (so the button
   * keeps its width), and presses are ignored. The button stays focusable (`aria-disabled`, not
   * `disabled`), so focus does not jump while it works.
   */
  loading?: boolean;
};

/** True when the button renders as a <button>: its own, or a `render` that is one. */
export function nativeButton(render: unknown): boolean {
  return !render || (isValidElement(render) && render.type === 'button');
}

const SPINNER_SIZE: Record<ButtonSize, 'xs' | 'sm' | 'md'> = {
  xs: 'xs',
  sm: 'sm',
  md: 'sm',
  lg: 'md',
  icon: 'md',
  'icon-sm': 'sm',
  'icon-xs': 'xs',
};

function withoutClick<P extends { onClick?: unknown }>({ onClick: _onClick, ...rest }: P) {
  return rest;
}

const ignorePress = (event: MouseEvent) => {
  event.preventDefault();
  event.stopPropagation();
};

/**
 * The one action control. `type` defaults to "button"; `render` swaps the element.
 * States: hover, pressed (`:active`), focus-visible (the 2px accent ring), disabled, `loading`.
 */
export function Button({
  variant,
  size,
  className,
  render,
  loading = false,
  ...props
}: ButtonProps) {
  const icon = size === 'icon' || size === 'icon-sm' || size === 'icon-xs';
  const defaults = {
    className: buttonClass({ variant, size, className }),
    'data-slot': 'button',
    'data-variant': variant ?? 'secondary',
    type: nativeButton(render) ? ('button' as const) : undefined,
  };
  const busy = loading
    ? {
        'aria-busy': true,
        'aria-disabled': true,
        'data-loading': '',
        onClick: ignorePress,
      }
    : {};
  const spinner = loading ? (
    // aria-busy says it is working; the spinner itself stays out of the button's name.
    <span aria-hidden="true" className="loams-btn-spinner">
      <Spinner size={SPINNER_SIZE[size ?? 'md']} label="" />
    </span>
  ) : null;
  return useRender({
    defaultTagName: 'button',
    // While loading the caller's onClick is left out (mergeProps would chain it), and the
    // busy handler cancels the press, so a submit button does not submit either.
    props: mergeProps<'button'>(
      defaults,
      loading ? withoutClick(props) : props,
      busy,
      loading
        ? {
            children: icon ? (
              spinner
            ) : (
              <>
                {spinner}
                {props.children}
              </>
            ),
          }
        : {},
    ),
    render,
  });
}

export type IconButtonProps = Omit<ButtonProps, 'size' | 'aria-label'> & {
  /** The accessible name, and the tooltip unless `tooltip` is false. Required: an icon has no name. */
  label: string;
  size?: 'md' | 'sm' | 'xs';
  /** Show the label in a tooltip on hover and focus (default), or a richer tooltip. */
  tooltip?: boolean | ReactNode;
};

const ICON_SIZE = { md: 'icon', sm: 'icon-sm', xs: 'icon-xs' } as const;

/**
 * A square button holding one icon (lucide-react at 16px, 14px when `sm`). `label` names it
 * for assistive technology and shows as its tooltip. Quiet by default, as toolbars want.
 */
export function IconButton({
  label,
  size = 'md',
  variant = 'quiet',
  tooltip = true,
  ...props
}: IconButtonProps): ReactElement {
  const button = <Button aria-label={label} size={ICON_SIZE[size]} variant={variant} {...props} />;
  if (tooltip === false) return button;
  return <Tooltip content={tooltip === true ? label : tooltip}>{button}</Tooltip>;
}
