// The query bar: one line of the query language with suggestions as you type (an ARIA
// combobox). Arrow keys move through the list, Enter or Tab picks the highlighted item,
// Enter with nothing highlighted runs the query, Escape closes the list, Ctrl+Space opens it.

import { cx } from '@loams/ui';
import {
  Braces,
  Calendar,
  CornerDownLeft,
  Fingerprint,
  Hash,
  History,
  Tag,
  ToggleLeft,
  Type,
  Waypoints,
} from 'lucide-react';
import {
  type KeyboardEvent,
  type ReactNode,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from 'react';
import type { FieldInfo, FieldType } from './kql.js';
import { applySuggestion, type SuggestContext, type Suggestion, suggest } from './suggest.js';

export function TypeIcon({ type, className }: { type: FieldType | undefined; className?: string }) {
  const props = { size: 14, 'aria-hidden': true, className: cx('shrink-0', className) } as const;
  switch (type) {
    case 'text':
      return <Type {...props} />;
    case 'i64':
    case 'f64':
      return <Hash {...props} />;
    case 'date':
      return <Calendar {...props} />;
    case 'bool':
      return <ToggleLeft {...props} />;
    case 'uuid':
      return <Fingerprint {...props} />;
    case 'json':
      return <Braces {...props} />;
    case 'vector':
      return <Waypoints {...props} />;
    default:
      return <Tag {...props} />;
  }
}

export interface QueryInputProps {
  value: string;
  onChange: (text: string) => void;
  onSubmit: (text: string) => void;
  fields: FieldInfo[];
  /** Cached top values (sync); `onValues` asks for them to be fetched. */
  values?: (field: string, prefix: string) => { value: string; count?: number }[] | undefined;
  onContext?: (context: SuggestContext) => void;
  recent?: string[];
  /** The accessible name. */
  label: string;
  placeholder?: string;
  invalid?: boolean;
  describedBy?: string;
  autoFocus?: boolean;
  /** Something to show at the right end, inside the bar (a run button). */
  end?: ReactNode;
  className?: string;
  /** A compact bar (the filter editor). */
  size?: 'md' | 'sm';
  inputRef?: React.RefObject<HTMLInputElement | null>;
}

export function QueryInput({
  value,
  onChange,
  onSubmit,
  fields,
  values,
  onContext,
  recent,
  label,
  placeholder,
  invalid,
  describedBy,
  autoFocus,
  end,
  className,
  size = 'md',
  inputRef,
}: QueryInputProps) {
  const own = useRef<HTMLInputElement>(null);
  const ref = inputRef ?? own;
  const listId = useId();
  const [open, setOpen] = useState(false);
  const [cursor, setCursor] = useState(value.length);
  const [active, setActive] = useState(-1);
  const pendingCursor = useRef<number | undefined>(undefined);

  const result = useMemo(
    () => suggest({ text: value, cursor, fields, values, recent }),
    [value, cursor, fields, values, recent],
  );
  const items = result.items;
  const pickable = items.filter((i) => i.insert !== undefined);

  // Tell the owner what the cursor is on, so it can fetch values.
  const contextKey = JSON.stringify(result.context);
  // oxlint-disable-next-line react/exhaustive-deps -- keyed on the context's content
  useEffect(() => {
    if (open) onContext?.(result.context);
  }, [contextKey, open]);

  useEffect(() => {
    if (pendingCursor.current !== undefined && ref.current) {
      const at = pendingCursor.current;
      pendingCursor.current = undefined;
      ref.current.setSelectionRange(at, at);
    }
  });

  // A new list starts with nothing highlighted.
  // oxlint-disable-next-line react/exhaustive-deps -- reset when the list changes
  useEffect(() => setActive(-1), [contextKey, items.length]);

  const sync = () => {
    const el = ref.current;
    if (el) setCursor(el.selectionStart ?? el.value.length);
  };

  const pick = (s: Suggestion) => {
    const next = applySuggestion(value, s);
    onChange(next.text);
    setCursor(next.cursor);
    pendingCursor.current = next.cursor;
    setOpen(true);
    ref.current?.focus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === ' ' && e.ctrlKey) {
      e.preventDefault();
      setOpen(true);
      return;
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      if (!open) {
        setOpen(true);
        e.preventDefault();
        return;
      }
      if (pickable.length === 0) return;
      e.preventDefault();
      const cur = active < 0 ? -1 : pickable.indexOf(items[active] as Suggestion);
      const n = pickable.length;
      const nextIdx = e.key === 'ArrowDown' ? (cur + 1) % n : cur <= 0 ? n - 1 : cur - 1;
      setActive(items.indexOf(pickable[nextIdx] as Suggestion));
      return;
    }
    if (e.key === 'Escape') {
      if (open) {
        e.preventDefault();
        e.stopPropagation();
        setOpen(false);
      }
      return;
    }
    if (
      (e.key === 'Enter' || e.key === 'Tab') &&
      open &&
      active >= 0 &&
      items[active]?.insert !== undefined
    ) {
      e.preventDefault();
      pick(items[active] as Suggestion);
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      setOpen(false);
      onSubmit(value);
    }
  };

  const showList = open && items.length > 0;
  const activeId = active >= 0 ? `${listId}-${active}` : undefined;
  const small = size === 'sm';

  return (
    <div className={cx('relative min-w-0', className)}>
      <div
        className={cx(
          'flex items-center gap-2 rounded-md border bg-bg pr-1 transition-colors',
          'focus-within:outline-2 focus-within:outline-offset-1 focus-within:outline-accent',
          invalid ? 'border-danger' : 'border-input',
        )}
      >
        <input
          ref={ref}
          type="text"
          role="combobox"
          aria-label={label}
          aria-expanded={showList}
          aria-controls={listId}
          aria-autocomplete="list"
          aria-activedescendant={activeId}
          aria-invalid={invalid || undefined}
          aria-describedby={describedBy}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          // oxlint-disable-next-line jsx-a11y/no-autofocus -- the filter editor opens to type in
          autoFocus={autoFocus}
          placeholder={placeholder}
          value={value}
          onChange={(e) => {
            onChange(e.target.value);
            setCursor(e.target.selectionStart ?? e.target.value.length);
            setOpen(true);
          }}
          onKeyDown={onKeyDown}
          onKeyUp={(e) => {
            if (
              e.key === 'ArrowLeft' ||
              e.key === 'ArrowRight' ||
              e.key === 'Home' ||
              e.key === 'End'
            )
              sync();
          }}
          onClick={() => {
            sync();
            setOpen(true);
          }}
          onFocus={() => {
            sync();
            setOpen(true);
          }}
          onBlur={() => setOpen(false)}
          // The bar draws the focus ring around the input and its button together.
          style={{ outline: 'none' }}
          className={cx(
            /* mono-ok: code */ 'min-w-0 flex-1 border-0 bg-transparent font-mono text-ink outline-none placeholder:text-faint',
            small ? 'h-8 px-2.5 text-small' : 'h-10 px-3 text-body',
          )}
        />
        {end}
      </div>
      {showList && (
        <div className="absolute top-full right-0 left-0 z-30 mt-1 overflow-hidden rounded-md border border-rule bg-bg shadow-pop">
          <div
            id={listId}
            role="listbox"
            aria-label={`${label} suggestions`}
            className="max-h-80 overflow-auto p-1"
          >
            {items.map((s, i) => {
              const hint = s.insert === undefined;
              return (
                <div
                  key={`${s.kind}-${s.label}-${s.from}`}
                  tabIndex={-1}
                  id={`${listId}-${i}`}
                  role="option"
                  aria-selected={i === active}
                  aria-disabled={hint || undefined}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    if (!hint) pick(s);
                  }}
                  onMouseEnter={() => !hint && setActive(i)}
                  className={cx(
                    'flex cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-body',
                    i === active ? 'bg-accent-soft text-ink' : 'text-ink',
                    hint && 'cursor-default text-muted italic',
                  )}
                >
                  <SuggestionIcon s={s} />
                  <span
                    className={cx(
                      'min-w-0 flex-1 truncate',
                      s.kind !== 'hint' && /* mono-ok: code */ 'font-mono',
                    )}
                  >
                    {s.label}
                  </span>
                  {s.detail && <span className="shrink-0 text-small text-muted">{s.detail}</span>}
                  {i === active && (
                    <CornerDownLeft size={12} aria-hidden className="shrink-0 text-faint" />
                  )}
                </div>
              );
            })}
          </div>
          <div className="flex gap-3 border-t border-rule px-3 py-1.5 text-micro text-muted">
            <span>↑↓ to move</span>
            <span>Enter or Tab to pick</span>
            <span>Esc to close</span>
          </div>
        </div>
      )}
    </div>
  );
}

function SuggestionIcon({ s }: { s: Suggestion }) {
  const box =
    'inline-flex size-5 shrink-0 items-center justify-center rounded-xs text-micro font-semibold';
  switch (s.kind) {
    case 'field':
      return (
        <span className={cx(box, 'bg-info-soft text-info')}>
          <TypeIcon type={s.type} />
        </span>
      );
    case 'operator':
      return <span className={cx(box, 'bg-raised text-muted')}>op</span>;
    case 'value':
      return <span className={cx(box, 'bg-grow-soft text-grow')}>v</span>;
    case 'conjunction':
      return <span className={cx(box, 'bg-warn-soft text-warn')}>&amp;</span>;
    case 'recent':
      return (
        <span className={cx(box, 'bg-raised text-muted')}>
          <History size={12} aria-hidden />
        </span>
      );
    default:
      return <span className={box} />;
  }
}
