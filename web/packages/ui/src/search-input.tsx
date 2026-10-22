// A search field (UI upgrade phase 1): the outlined control box with a search icon, a clear
// button while it holds text, and Escape to clear. Not vendored.

import { Input as InputPrimitive } from '@base-ui/react/input';
import { Search, X } from 'lucide-react';
import { type KeyboardEvent, type Ref, useRef } from 'react';
import { cx } from './cx';
import { CONTROL_BOX } from './input';

export interface SearchInputProps {
  value: string;
  onValueChange: (value: string) => void;
  /** The accessible name (default "Search"); the placeholder is not a name. */
  label?: string;
  placeholder?: string;
  /** `compact` is 28px at 12px, for toolbars and sidebars; `default` is 32px. */
  size?: 'compact' | 'default';
  autoFocus?: boolean;
  disabled?: boolean;
  className?: string;
  inputRef?: Ref<HTMLInputElement>;
  /** Called on Enter, with the current value. */
  onSubmit?: (value: string) => void;
}

/**
 * Escape clears the text when there is some (and stops there, so an enclosing dialog or
 * inspector stays open); with no text it does nothing, and the Escape reaches the page.
 */
export function SearchInput({
  value,
  onValueChange,
  label = 'Search',
  placeholder = 'Search',
  size = 'default',
  autoFocus,
  disabled,
  className,
  inputRef,
  onSubmit,
}: SearchInputProps) {
  const own = useRef<HTMLInputElement | null>(null);
  const compact = size === 'compact';
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Escape' && value !== '') {
      event.preventDefault();
      event.stopPropagation();
      onValueChange('');
    } else if (event.key === 'Enter' && onSubmit) {
      event.preventDefault();
      onSubmit(value);
    }
  };
  return (
    <span
      className={cx(CONTROL_BOX, 'items-center', compact ? 'h-7 rounded-sm' : 'h-8', className)}
      data-slot="search-input"
      data-size={size}
    >
      <Search
        aria-hidden="true"
        className={cx(
          'pointer-events-none shrink-0 text-faint',
          compact ? 'ms-2 size-3.5' : 'ms-2.5 size-4',
        )}
      />
      <InputPrimitive
        ref={(node: HTMLInputElement | null) => {
          own.current = node;
          if (typeof inputRef === 'function') inputRef(node);
          else if (inputRef) inputRef.current = node;
        }}
        type="search"
        aria-label={label}
        autoComplete="off"
        spellCheck={false}
        autoFocus={autoFocus}
        disabled={disabled}
        placeholder={placeholder}
        value={value}
        onValueChange={(next) => onValueChange(next)}
        onKeyDown={onKeyDown}
        className={cx(
          'h-full w-full min-w-0 border-0 bg-transparent font-[inherit] text-inherit outline-none placeholder:text-faint',
          '[&::-webkit-search-cancel-button]:appearance-none [&::-webkit-search-decoration]:appearance-none',
          compact ? 'px-2 text-small' : 'px-2 text-[length:inherit]',
        )}
      />
      {value !== '' && !disabled && (
        <button
          type="button"
          aria-label="Clear search"
          // Escape clears from the keyboard, as a native search field's cancel button is
          // pointer-only; out of the tab order, so the box keeps one focus ring.
          tabIndex={-1}
          className={cx(
            'me-1 inline-flex shrink-0 cursor-pointer items-center justify-center rounded-xs border-0 bg-transparent p-0 text-faint',
            'transition-colors duration-(--op-duration-fast) hover:bg-raised hover:text-ink',
            compact ? 'size-5' : 'size-6',
          )}
          onClick={() => {
            onValueChange('');
            own.current?.focus();
          }}
        >
          <X aria-hidden="true" className="size-3.5" />
        </button>
      )}
    </span>
  );
}
