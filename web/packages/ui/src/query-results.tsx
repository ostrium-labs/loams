// The results grid every query page shares: column names with their types, the row count and
// time, Copy as CSV or JSON, and only the rows in view drawn (a fixed row height and a scroll
// window), so a 100 000-row result costs no more to draw than a page of it. The table keeps
// `aria-rowcount` and each row's `aria-rowindex`, so assistive technology knows the whole size.

import { type ReactNode, useId, useState } from 'react';
import { Button } from './button';

export interface ResultColumn {
  name: string;
  type?: string;
}

export const RESULT_ROW_HEIGHT = 28;
export const RESULT_VIEW_ROWS = 15;
const OVERSCAN = 10;
/** Longest cell text drawn; the rest is in the cell's title and in a copy. */
export const MAX_CELL_TEXT = 300;

const fmt = new Intl.NumberFormat('en');

export function cellText(v: unknown): string {
  if (v === null || v === undefined) return 'NULL';
  if (typeof v === 'string') return v;
  if (typeof v === 'bigint') return v.toString();
  return JSON.stringify(v) ?? String(v);
}

/** The slice of rows to draw for a scroll offset. */
export function resultWindow(scrollTop: number, total: number): { first: number; last: number } {
  const first = Math.max(0, Math.floor(scrollTop / RESULT_ROW_HEIGHT) - OVERSCAN);
  const last = Math.min(
    total,
    Math.ceil(scrollTop / RESULT_ROW_HEIGHT) + RESULT_VIEW_ROWS + OVERSCAN,
  );
  return { first, last };
}

function csvField(v: unknown): string {
  if (v === null || v === undefined) return '';
  const s = cellText(v);
  return /[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

export function toCsv(columns: ResultColumn[], rows: readonly (readonly unknown[])[]): string {
  const head = columns.map((c) => csvField(c.name)).join(',');
  return [head, ...rows.map((r) => columns.map((_, i) => csvField(r[i])).join(','))].join('\n');
}

export function toJsonRows(columns: ResultColumn[], rows: readonly (readonly unknown[])[]): string {
  const objects = rows.map((r) =>
    Object.fromEntries(
      columns.map((c, i) => [c.name, typeof r[i] === 'bigint' ? String(r[i]) : (r[i] ?? null)]),
    ),
  );
  return JSON.stringify(objects, null, 2);
}

/** A short label for an engine type: `FixedSizeList(512 x Float32)` stays, `Timestamp(ms, "UTC")` too. */
function isNumericType(type?: string): boolean {
  return (
    !!type &&
    /^(u?int|float|double|decimal|numeric|real|bigint|smallint|tinyint|integer|number)/i.test(
      type.replace(/^Nullable\(/, ''),
    )
  );
}

async function writeClipboard(text: string, copy?: (text: string) => void | Promise<void>) {
  if (copy) return copy(text);
  await globalThis.navigator?.clipboard?.writeText(text);
}

export interface QueryResultsProps {
  columns: ResultColumn[];
  rows: readonly (readonly unknown[])[];
  elapsedMs?: number;
  /** The server cut the result at its row limit. */
  truncated?: boolean;
  /** For a statement with no result set. */
  rowsAffected?: number;
  /** More facts for the status line (`1 000 rows read`). */
  extra?: ReactNode;
  /** Writes to the clipboard; defaults to `navigator.clipboard`. */
  copy?: (text: string) => void | Promise<void>;
  /** Shown in place of the grid when there are columns but no rows. */
  emptyText?: string;
}

export function QueryResults({
  columns,
  rows,
  elapsedMs,
  truncated,
  rowsAffected,
  extra,
  copy,
  emptyText = 'The query returned no rows.',
}: QueryResultsProps) {
  const [top, setTop] = useState(0);
  const [copied, setCopied] = useState<string>();
  const caption = useId();
  const { first, last } = resultWindow(top, rows.length);
  const before = first * RESULT_ROW_HEIGHT;
  const after = Math.max(0, (rows.length - last) * RESULT_ROW_HEIGHT);

  const doCopy = async (kind: 'CSV' | 'JSON') => {
    try {
      await writeClipboard(kind === 'CSV' ? toCsv(columns, rows) : toJsonRows(columns, rows), copy);
      setCopied(`Copied ${fmt.format(rows.length)} rows as ${kind}.`);
    } catch {
      setCopied('The browser did not allow the copy.');
    }
  };

  const count =
    columns.length === 0
      ? `${fmt.format(rowsAffected ?? 0)} ${rowsAffected === 1 ? 'row' : 'rows'} affected`
      : `${fmt.format(rows.length)} ${rows.length === 1 ? 'row' : 'rows'}`;

  return (
    <section className="loams-results" aria-label="Results">
      <div className="loams-results-bar">
        <p className="loams-results-status" role="status">
          <strong>{count}</strong>
          {elapsedMs !== undefined && <span> · {fmt.format(Math.round(elapsedMs))} ms</span>}
          {truncated && (
            <span className="loams-results-truncated"> · truncated by the row limit</span>
          )}
          {extra && <span> · {extra}</span>}
        </p>
        <span className="loams-results-copied" aria-live="polite">
          {copied}
        </span>
        {columns.length > 0 && rows.length > 0 && (
          <div className="loams-results-actions">
            <Button size="sm" variant="quiet" onClick={() => void doCopy('CSV')}>
              Copy CSV
            </Button>
            <Button size="sm" variant="quiet" onClick={() => void doCopy('JSON')}>
              Copy JSON
            </Button>
          </div>
        )}
      </div>
      {columns.length > 0 && (
        <section
          className="loams-results-scroll"
          style={{ maxHeight: RESULT_ROW_HEIGHT * (RESULT_VIEW_ROWS + 1) + 2 }}
          onScroll={(e) => setTop(e.currentTarget.scrollTop)}
          // oxlint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- a scroll region must take focus
          tabIndex={0}
          aria-labelledby={caption}
        >
          <div aria-hidden="true" style={{ height: before }} />
          <table
            className="loams-results-table"
            aria-rowcount={rows.length + 1}
            aria-colcount={columns.length}
          >
            <caption id={caption} className="sr-only">
              Result rows
            </caption>
            <thead>
              <tr aria-rowindex={1}>
                <th scope="col" className="loams-results-rownum" aria-label="Row number">
                  #
                </th>
                {columns.map((c, i) => (
                  <th
                    // oxlint-disable-next-line react/no-array-index-key -- columns can repeat names
                    key={i}
                    scope="col"
                    className={isNumericType(c.type) ? 'loams-num' : undefined}
                  >
                    <span className="loams-results-col">{c.name}</span>
                    {c.type && (
                      <span className="loams-results-type" title={c.type}>
                        {c.type}
                      </span>
                    )}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.slice(first, last).map((row, j) => {
                const r = first + j;
                return (
                  <tr key={r} aria-rowindex={r + 2} style={{ height: RESULT_ROW_HEIGHT }}>
                    <td className="loams-results-rownum">{r + 1}</td>
                    {columns.map((c, k) => {
                      const v = row[k];
                      const text = cellText(v);
                      const shown =
                        text.length > MAX_CELL_TEXT ? `${text.slice(0, MAX_CELL_TEXT)}…` : text;
                      return (
                        <td
                          // oxlint-disable-next-line react/no-array-index-key -- positional cells
                          key={k}
                          className={
                            v === null || v === undefined
                              ? 'loams-results-null'
                              : isNumericType(c.type)
                                ? 'loams-num'
                                : undefined
                          }
                          title={text.length > 40 ? text.slice(0, 2000) : undefined}
                        >
                          {shown}
                        </td>
                      );
                    })}
                  </tr>
                );
              })}
            </tbody>
          </table>
          <div aria-hidden="true" style={{ height: after }} />
          {rows.length === 0 && <p className="loams-results-empty">{emptyText}</p>}
        </section>
      )}
    </section>
  );
}
