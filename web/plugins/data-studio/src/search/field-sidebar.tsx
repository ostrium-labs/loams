// The field list beside the results (Kibana's field sidebar): every schema field with
// its type; opening one shows its top values (or its range) in the documents the
// current filter matches, with buttons to filter for or out a value.

import type { JsonObject, JsonValue } from '@bufbuild/protobuf';
import { Button, cx, SearchInput } from '@loams/ui';
import { ChevronDown, ChevronRight, Columns3, Minus, Plus } from 'lucide-react';
import { useEffect, useState } from 'react';
import { bucketsOf, type DataClient } from '../client.js';
import { errorText } from '../shared.js';
import type { FieldInfo } from './kql.js';
import { TypeIcon } from './query-input.js';
import { TYPE_LABEL } from './suggest.js';

export interface FieldSidebarProps {
  client: DataClient;
  ns: string;
  coll: string;
  fields: FieldInfo[];
  /** The filter of the last search, so values are those of the matching documents. */
  filter?: JsonObject;
  /** Adds a filter pill for `field` = `value` (negated: not that value). */
  onFilter: (field: string, value: string, negate: boolean) => void;
  /** Adds `field:*` (the field has a value). */
  onExists: (field: string) => void;
  columns: string[];
  onToggleColumn: (field: string) => void;
}

export function FieldSidebar(p: FieldSidebarProps) {
  const [filterText, setFilterText] = useState('');
  const [openField, setOpenField] = useState<string>();
  const f = filterText.trim().toLowerCase();
  const shown = p.fields.filter((x) => !f || x.name.toLowerCase().includes(f));
  return (
    <aside aria-label="Fields" className="flex min-w-0 flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 className="m-0 text-small font-medium text-muted">Fields</h2>
        <span className="text-small text-faint">{p.fields.length}</span>
      </div>
      {p.fields.length > 8 && (
        <SearchInput
          size="compact"
          label="Filter fields"
          placeholder="Filter fields"
          value={filterText}
          onValueChange={setFilterText}
        />
      )}
      <ul className="m-0 flex list-none flex-col gap-0.5 p-0">
        {shown.map((field) => (
          <FieldRow
            key={field.name}
            {...p}
            field={field}
            open={openField === field.name}
            onOpen={() => setOpenField(openField === field.name ? undefined : field.name)}
          />
        ))}
      </ul>
    </aside>
  );
}

type Detail =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'values'; buckets: { key: string; count: number }[]; total: number }
  | { state: 'range'; min: string; avg: string; max: string; count: number };

function FieldRow({
  client,
  ns,
  coll,
  field,
  filter,
  open,
  onOpen,
  onFilter,
  onExists,
  columns,
  onToggleColumn,
}: FieldSidebarProps & { field: FieldInfo; open: boolean; onOpen: () => void }) {
  const [detail, setDetail] = useState<Detail>();
  const filterKey = JSON.stringify(filter ?? null);
  const canValues = field.fast && ['keyword', 'uuid', 'bool'].includes(field.type);
  const canRange = field.fast && ['i64', 'f64', 'date'].includes(field.type);

  // oxlint-disable-next-line react/exhaustive-deps -- refetch when the filter's content changes
  useEffect(() => {
    if (!open || (!canValues && !canRange)) return;
    const ctrl = new AbortController();
    setDetail({ state: 'loading' });
    const aggs: JsonObject = {};
    if (canValues) aggs.v = { terms: { field: field.name, size: 5 } };
    if (canRange) aggs.s = { stats: { field: field.name } };
    client.aggregate(ns, coll, aggs, filter, ctrl.signal).then(
      (res) => {
        if (canValues) {
          const v = res.v;
          const other =
            v && typeof v === 'object' && !Array.isArray(v)
              ? Number(v.sum_other_doc_count ?? 0)
              : 0;
          const buckets = bucketsOf(v);
          setDetail({
            state: 'values',
            buckets,
            total: buckets.reduce((a, b) => a + b.count, 0) + other,
          });
        } else {
          const s = res.s as JsonObject | undefined;
          if (!s || s.count === 0) {
            setDetail({ state: 'values', buckets: [], total: 0 });
            return;
          }
          const show = (n: JsonValue | undefined) => formatStat(n, field.type);
          setDetail({
            state: 'range',
            min: show(s.min),
            avg: show(s.avg),
            max: show(s.max),
            count: Number(s.count ?? 0),
          });
        }
      },
      (e) => {
        if (!ctrl.signal.aborted) setDetail({ state: 'error', message: errorText(e) });
      },
    );
    return () => ctrl.abort();
  }, [open, filterKey, client, ns, coll, field.name]);

  const panelId = `field-${field.name}`;
  const isColumn = columns.includes(field.name);
  return (
    <li className="min-w-0">
      <div className="group flex items-center gap-1 rounded-sm hover:bg-row-hover">
        <button
          type="button"
          aria-expanded={open}
          aria-controls={panelId}
          onClick={onOpen}
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 border-0 bg-transparent px-1.5 py-1 text-left text-body text-ink"
        >
          {open ? (
            <ChevronDown size={12} aria-hidden className="shrink-0 text-faint" />
          ) : (
            <ChevronRight size={12} aria-hidden className="shrink-0 text-faint" />
          )}
          <span
            className="inline-flex size-5 shrink-0 items-center justify-center rounded-xs bg-raised text-muted"
            title={TYPE_LABEL[field.type]}
          >
            <TypeIcon type={field.type} />
          </span>
          <span className="min-w-0 flex-1 truncate">{field.name}</span>
        </button>
        {field.type !== 'vector' && (
          <button
            type="button"
            aria-pressed={isColumn}
            aria-label={
              isColumn ? `Remove ${field.name} from the table` : `Add ${field.name} to the table`
            }
            title={isColumn ? 'Remove column' : 'Add column'}
            onClick={() => onToggleColumn(field.name)}
            className={cx(
              'inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-xs border-0 bg-transparent',
              isColumn
                ? 'text-accent-fg'
                : 'text-faint opacity-0 group-hover:opacity-100 focus-visible:opacity-100',
            )}
          >
            <Columns3 size={13} aria-hidden />
          </button>
        )}
      </div>
      {open && (
        <div
          id={panelId}
          className="mb-2 ml-6 flex flex-col gap-1.5 border-l border-rule py-1 pl-3 text-small"
        >
          <span className="text-muted">
            {TYPE_LABEL[field.type]}
            {field.type === 'vector' && ` · ${field.dim} dimensions · ${field.distance ?? ''}`}
            {field.fast && field.type !== 'vector' && ' · fast'}
            {!field.indexed && ' · not indexed'}
          </span>
          {field.type === 'text' && (
            <span className="text-muted">
              Searched by free text; type {field.name}: to search only this field.
            </span>
          )}
          {field.type === 'vector' && (
            <span className="text-muted">Used by the Vector and Hybrid modes.</span>
          )}
          {!canValues && !canRange && field.type !== 'vector' && field.type !== 'text' && (
            <span className="text-muted">Not a fast field, so its values cannot be listed.</span>
          )}
          {detail?.state === 'loading' && <span className="text-muted">Reading values…</span>}
          {detail?.state === 'error' && <span className="text-danger">{detail.message}</span>}
          {detail?.state === 'values' && detail.buckets.length === 0 && (
            <span className="text-muted">No values in the matching documents.</span>
          )}
          {detail?.state === 'values' && detail.buckets.length > 0 && (
            <>
              <span className="text-faint">Top values in the matching documents</span>
              <ul className="m-0 flex list-none flex-col gap-1 p-0">
                {detail.buckets.map((b) => {
                  const pct = detail.total > 0 ? (b.count / detail.total) * 100 : 0;
                  return (
                    <li key={b.key} className="group/v flex min-w-0 flex-col gap-0.5">
                      <div className="flex min-w-0 items-center gap-1">
                        <span className="min-w-0 flex-1 truncate text-ink" title={b.key}>
                          {b.key}
                        </span>
                        <span className="shrink-0 text-muted tabular-nums">
                          {pct < 1 ? '<1' : Math.round(pct)}%
                        </span>
                        <ValueButton
                          label={`Filter for ${field.name} ${b.key}`}
                          onClick={() => onFilter(field.name, b.key, false)}
                        >
                          <Plus size={12} aria-hidden />
                        </ValueButton>
                        <ValueButton
                          label={`Filter out ${field.name} ${b.key}`}
                          onClick={() => onFilter(field.name, b.key, true)}
                        >
                          <Minus size={12} aria-hidden />
                        </ValueButton>
                      </div>
                      <div className="h-1 overflow-hidden rounded-pill bg-raised" aria-hidden>
                        <div
                          className="h-full rounded-pill bg-accent"
                          style={{ width: `${Math.max(2, pct)}%` }}
                        />
                      </div>
                    </li>
                  );
                })}
              </ul>
            </>
          )}
          {detail?.state === 'range' && (
            <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
              <dt className="text-muted">min</dt>
              <dd className="m-0 truncate text-ink tabular-nums">{detail.min}</dd>
              <dt className="text-muted">avg</dt>
              <dd className="m-0 truncate text-ink tabular-nums">{detail.avg}</dd>
              <dt className="text-muted">max</dt>
              <dd className="m-0 truncate text-ink tabular-nums">{detail.max}</dd>
            </dl>
          )}
          {field.type !== 'vector' && field.indexed && (
            <div>
              <Button size="xs" variant="quiet" onClick={() => onExists(field.name)}>
                Filter: has a value
              </Button>
            </div>
          )}
        </div>
      )}
    </li>
  );
}

function ValueButton({
  label,
  onClick,
  children,
}: {
  label: string;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className="inline-flex size-5 shrink-0 cursor-pointer items-center justify-center rounded-xs border border-rule bg-bg text-muted hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
    >
      {children}
    </button>
  );
}

/** A stats value: dates come back as epoch nanoseconds. */
export function formatStat(n: JsonValue | undefined, type: string): string {
  if (typeof n !== 'number' || !Number.isFinite(n)) return '–';
  if (type === 'date') {
    const ms = Math.abs(n) > 1e17 ? n / 1e6 : Math.abs(n) > 1e14 ? n / 1e3 : n;
    const d = new Date(ms);
    return Number.isNaN(d.getTime()) ? String(n) : d.toISOString().replace('.000Z', 'Z');
  }
  return Number.isInteger(n)
    ? n.toLocaleString('en-US')
    : n.toLocaleString('en-US', { maximumFractionDigits: 2 });
}
