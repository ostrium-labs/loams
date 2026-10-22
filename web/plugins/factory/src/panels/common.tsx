import { type Column, Empty, formatNumber, formatRelative, Table } from '@loams/ui';
import type { ReactNode } from 'react';
import { ago } from '../model.js';

export const rel = (iso?: string) => ago(iso, formatRelative);
export const num = (n?: number) => (n === undefined ? '' : formatNumber(n));
export const mono = (s?: string | number) => (
  <span className={/* mono-ok: id */ 'font-mono text-xs'}>{s === undefined ? '' : String(s)}</span>
);
export const dash = (s?: ReactNode) => s ?? '';

/** A table with an empty state, for the rows a panel got. */
export function DataTable<T>({
  rows,
  columns,
  caption,
  empty,
  rowKey,
}: {
  rows: T[];
  columns: Column<T>[];
  caption: string;
  empty: string;
  rowKey: (row: T, i: number) => string;
}) {
  const keyed = rows.map((r, i) => ({ r, k: rowKey(r, i) }));
  return (
    <Table
      caption={caption}
      rows={keyed}
      rowKey={(x) => x.k}
      empty={<Empty title={empty} />}
      columns={columns.map((c) => ({ ...c, cell: (x: { r: T }) => c.cell(x.r) }))}
    />
  );
}

/** Column keys for a numeric-metric row, in a stable order, without the date or internal columns. */
export function metricKeys(rows: Record<string, unknown>[]): string[] {
  const keys = new Set<string>();
  // A leading underscore marks an upstream's internal column (OpenPanel's `_avg_…`).
  for (const r of rows)
    for (const [k, v] of Object.entries(r))
      if (typeof v === 'number' && !k.startsWith('_')) keys.add(k);
  return [...keys];
}

/** A metric key as words: `nb_visits` -> "visits", `inProgress` -> "in progress" (`p95` stays). */
export const label = (k: string) =>
  k
    .replace(/^nb_/, '')
    .replace(/_/g, ' ')
    .replace(/([a-z])([A-Z])/g, '$1 $2')
    .toLowerCase();
