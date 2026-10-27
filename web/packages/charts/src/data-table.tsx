import type { Row } from './types';
import { timeOf } from './window';

/** Visually hidden but read by screen readers (mirrors `.sr-only` in @loams/ui). */
export const srOnly = {
  position: 'absolute',
  width: 1,
  height: 1,
  padding: 0,
  margin: -1,
  overflow: 'hidden',
  clip: 'rect(0, 0, 0, 0)',
  whiteSpace: 'nowrap',
  border: 0,
} as const;

/** Most rows the hidden table lists; a live chart keeps thousands, a listener wants the latest. */
export const TABLE_ROW_LIMIT = 50;

function cell(value: unknown, isTime: boolean): string {
  if (value === null || value === undefined) return '';
  if (isTime) {
    const t = timeOf(value);
    if (t !== undefined) return new Date(t).toISOString();
  }
  if (typeof value === 'number') return Number.isInteger(value) ? String(value) : value.toFixed(3);
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}

/** The chart's data as a table, the screen-reader fallback for the canvas. */
export function DataTable({
  rows,
  caption,
  timeField,
  limit = TABLE_ROW_LIMIT,
}: {
  rows: readonly Row[];
  caption: string;
  timeField?: string;
  limit?: number;
}) {
  const shown = rows.length > limit ? rows.slice(rows.length - limit) : rows;
  const columns: string[] = [];
  for (const row of shown)
    for (const key of Object.keys(row)) if (!columns.includes(key)) columns.push(key);
  const note = rows.length > shown.length ? ` (latest ${shown.length} of ${rows.length} rows)` : '';
  // The hiding sits on a wrapper: a table keeps its min-content width whatever its
  // `width`, which would widen the page on a phone.
  return (
    <div style={srOnly}>
      <table>
        <caption>
          {caption}
          {note}
        </caption>
        <thead>
          <tr>
            {columns.map((c) => (
              <th key={c} scope="col">
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {shown.length === 0 ? (
            <tr>
              <td colSpan={Math.max(1, columns.length)}>No data yet</td>
            </tr>
          ) : (
            shown.map((row, i) => (
              // Rows have no identity of their own; position in the window is the key.
              // oxlint-disable-next-line react/no-array-index-key -- see above
              <tr key={i}>
                {columns.map((c) => (
                  <td key={c}>{cell(row[c], c === timeField)}</td>
                ))}
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  );
}
