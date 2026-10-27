// Heavy tables and lists (docs/design/console-ui-stack.md, decision 5): TanStack
// Table for sorting, TanStack Virtual for long lists, in the same look as `Table`.

import {
  type ColumnDef,
  createSortedRowModel,
  type RowData,
  rowSortingFeature,
  sortFn_alphanumeric,
  sortFn_basic,
  tableFeatures,
  useTable,
} from '@tanstack/react-table';
import { useVirtualizer } from '@tanstack/react-virtual';
import { type ReactNode, useMemo, useRef } from 'react';
import { cx } from './cx';
import type { Column } from './table';

const features = tableFeatures({
  rowSortingFeature,
  sortedRowModel: createSortedRowModel(),
  sortFns: { alphanumeric: sortFn_alphanumeric, basic: sortFn_basic },
});

export type SortableColumn<T> = Column<T> & {
  /** The value this column sorts by; leave it out for a column that does not sort. */
  sortValue?: (row: T) => string | number;
};

/**
 * `Table` with sortable columns: a header button cycles ascending, descending.
 * Same props as `Table`, plus `sortValue` on the columns that sort.
 */
export function DataTable<T extends RowData>({
  columns,
  rows,
  rowKey,
  empty,
  caption,
  onRowClick,
}: {
  columns: SortableColumn<T>[];
  rows: T[];
  rowKey: (row: T) => string;
  empty?: ReactNode;
  caption?: string;
  onRowClick?: (row: T) => void;
}) {
  const defs = useMemo(
    () =>
      columns.map((c): ColumnDef<typeof features, T> => ({
        id: c.key,
        accessorFn: c.sortValue ?? (() => ''),
        enableSorting: c.sortValue !== undefined,
        sortFn: 'alphanumeric',
        sortDescFirst: false,
        header: () => c.header,
        cell: ({ row }) => c.cell(row.original),
      })),
    [columns],
  );
  const table = useTable({
    features,
    columns: defs,
    data: rows,
    getRowId: rowKey,
    enableSortingRemoval: false,
  });
  if (rows.length === 0 && empty) return <>{empty}</>;
  const byId = new Map(columns.map((c) => [c.key, c]));
  return (
    <div className="loams-table-wrap">
      <table className="loams-table">
        {caption && <caption className="sr-only">{caption}</caption>}
        <thead>
          {table.getHeaderGroups().map((group) => (
            <tr key={group.id}>
              {group.headers.map((h) => {
                const col = byId.get(h.column.id);
                const sorted = h.column.getIsSorted();
                return (
                  <th
                    key={h.id}
                    className={col?.numeric ? 'loams-num' : undefined}
                    style={{ width: col?.width }}
                    aria-sort={
                      sorted === 'asc' ? 'ascending' : sorted === 'desc' ? 'descending' : undefined
                    }
                  >
                    {h.column.getCanSort() ? (
                      <button
                        type="button"
                        className="loams-sort"
                        onClick={h.column.getToggleSortingHandler()}
                      >
                        <table.FlexRender header={h} />
                        <span aria-hidden="true" className={cx(!sorted && 'opacity-40')}>
                          {sorted === 'desc' ? '↓' : '↑'}
                        </span>
                      </button>
                    ) : (
                      <table.FlexRender header={h} />
                    )}
                  </th>
                );
              })}
            </tr>
          ))}
        </thead>
        <tbody>
          {table.getRowModel().rows.map((row) => (
            <tr
              key={row.id}
              data-href={onRowClick ? '' : undefined}
              onClick={
                onRowClick
                  ? (e) => {
                      if (
                        (e.target as HTMLElement).closest(
                          'a, button, input, select, textarea, label',
                        )
                      )
                        return;
                      onRowClick(row.original);
                    }
                  : undefined
              }
            >
              {row.getAllCells().map((cell) => (
                <td
                  key={cell.id}
                  className={byId.get(cell.column.id)?.numeric ? 'loams-num' : undefined}
                >
                  <table.FlexRender cell={cell} />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/**
 * A long list that renders only the rows in view. Rows are `estimate` pixels tall
 * (measured after render if they differ); the list scrolls inside `height`.
 */
export function VirtualList<T>({
  items,
  itemKey,
  render,
  estimate = 36,
  height = 480,
  label,
}: {
  items: T[];
  itemKey: (item: T) => string;
  render: (item: T, index: number) => ReactNode;
  estimate?: number;
  height?: number | string;
  label: string;
}) {
  const scroller = useRef<HTMLElement>(null);
  const v = useVirtualizer({
    count: items.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => estimate,
    getItemKey: (i) => {
      const item = items[i];
      return item === undefined ? i : itemKey(item);
    },
    overscan: 8,
  });
  return (
    <section
      ref={scroller}
      className="overflow-auto border border-rule-soft"
      style={{ height }}
      aria-label={label}
      // oxlint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- a scrolling region must be reachable by keyboard
      tabIndex={0}
    >
      <ul className="relative m-0 list-none p-0" style={{ height: v.getTotalSize() }}>
        {v.getVirtualItems().map((row) => {
          const item = items[row.index];
          if (item === undefined) return null;
          return (
            <li
              key={row.key}
              ref={v.measureElement}
              data-index={row.index}
              className="absolute top-0 left-0 w-full"
              style={{ transform: `translateY(${row.start}px)` }}
            >
              {render(item, row.index)}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
