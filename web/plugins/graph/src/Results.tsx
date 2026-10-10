// Results (§48 §18.2): a RowSet as a table of typed cells, the graph view of its Node,
// Relationship and Path values, the `truncated` banner with "Stream all", write
// counters and notifications; and the plan tree of an Explain or a Profile.

import type { graph } from '@loams/proto';
import { Button, type Column, Table } from '@loams/ui';
import { useId, useMemo, useState } from 'react';
import { STREAM_MAX_ROWS } from './client.js';
import { GraphView } from './GraphView.js';
import { collectElements, valueText, valueType } from './values.js';

export interface RowsResult {
  kind: 'rows';
  columns: string[];
  columnTypes: string[];
  rows: graph.Row[];
  truncated: boolean;
  counters?: graph.Counters;
  notifications: graph.Notification[];
  elapsedNanos: bigint;
  /** Set once "Stream all" has run: every row up to STREAM_MAX_ROWS. */
  streamed?: boolean;
}

export interface PlanResult {
  kind: 'plan';
  plan: graph.Plan;
  profile: boolean;
}

export type Result = RowsResult | PlanResult;

/** Rows per table page: the table draws this many at once, whatever the result holds. */
export const TABLE_PAGE = 1000;

const fmt = new Intl.NumberFormat('en');

function elapsed(nanos: bigint): string {
  const ms = Number(nanos) / 1e6;
  return ms < 1 ? `${(ms * 1000).toFixed(0)} µs` : `${ms.toFixed(ms < 10 ? 2 : 0)} ms`;
}

function Cell({ value }: { value: graph.Value | undefined }) {
  const type = valueType(value);
  const text = valueText(value);
  return (
    <span
      className={`inline-block max-w-[420px] truncate align-bottom font-mono text-[13px] ${
        type === 'NULL' ? 'text-faint' : ''
      }`}
      data-type={type}
      title={`${type}: ${text}`}
    >
      {text}
    </span>
  );
}

const COUNTER_LABELS: [keyof graph.Counters, string][] = [
  ['nodesCreated', 'nodes created'],
  ['nodesDeleted', 'nodes deleted'],
  ['relationshipsCreated', 'relationships created'],
  ['relationshipsDeleted', 'relationships deleted'],
  ['propertiesSet', 'properties set'],
  ['labelsAdded', 'labels added'],
  ['labelsRemoved', 'labels removed'],
];

function countersText(c: graph.Counters | undefined): string | undefined {
  if (!c) return undefined;
  const parts = COUNTER_LABELS.filter(([k]) => (c[k] as bigint) > 0n).map(
    ([k, label]) => `${c[k]} ${label}`,
  );
  return parts.length ? parts.join(', ') : undefined;
}

export function Results({
  result,
  streaming,
  onStreamAll,
}: {
  result: Result;
  streaming: boolean;
  onStreamAll: () => void;
}) {
  if (result.kind === 'plan') return <PlanView result={result} />;
  return <RowsView result={result} streaming={streaming} onStreamAll={onStreamAll} />;
}

function RowsView({
  result,
  streaming,
  onStreamAll,
}: {
  result: RowsResult;
  streaming: boolean;
  onStreamAll: () => void;
}) {
  const elements = useMemo(() => collectElements(result.rows), [result.rows]);
  const hasGraph = elements.nodes.size > 0 || elements.relationships.size > 0;
  const [tab, setTab] = useState<'table' | 'graph'>('table');
  const view = hasGraph ? tab : 'table';
  const [page, setPage] = useState(0);
  const pages = Math.max(1, Math.ceil(result.rows.length / TABLE_PAGE));
  const current = Math.min(page, pages - 1);
  const base = useId();
  const counters = countersText(result.counters);

  const columns: Column<{ i: number; row: graph.Row }>[] = result.columns.map((name, c) => {
    const type = result.columnTypes[c];
    return {
      key: `${c}`,
      header: (
        <span>
          {name}
          {type && type !== 'ANY' && (
            <span className="ml-1 font-normal text-faint text-xs">{type}</span>
          )}
        </span>
      ),
      cell: ({ row }) => <Cell value={row.values[c]} />,
    };
  });
  const shown = result.rows
    .slice(current * TABLE_PAGE, (current + 1) * TABLE_PAGE)
    .map((row, j) => ({ i: current * TABLE_PAGE + j, row }));

  const table = (
    <>
      <Table
        columns={columns}
        rows={shown}
        rowKey={(r) => String(r.i)}
        caption="Result rows"
        empty={<p className="text-muted">No rows.</p>}
      />
      {pages > 1 && (
        <nav className="flex items-center justify-end gap-2 pt-2" aria-label="Result pages">
          <span className="mr-auto text-sm text-muted">
            Rows {fmt.format(current * TABLE_PAGE + 1)}–
            {fmt.format(Math.min((current + 1) * TABLE_PAGE, result.rows.length))}
          </span>
          <Button size="sm" disabled={current === 0} onClick={() => setPage(current - 1)}>
            Previous
          </Button>
          <Button size="sm" disabled={current >= pages - 1} onClick={() => setPage(current + 1)}>
            Next
          </Button>
        </nav>
      )}
    </>
  );

  return (
    <section className="flex flex-col gap-3" aria-label="Results">
      <p className="m-0 text-sm text-muted" role="status">
        {fmt.format(result.rows.length)} {result.rows.length === 1 ? 'row' : 'rows'}
        {result.elapsedNanos > 0n && ` in ${elapsed(result.elapsedNanos)}`}
        {counters && `. ${counters}`}
      </p>
      {result.truncated && (
        <div
          className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-solid border-rule bg-accent-soft px-3 py-2"
          role="status"
          data-testid="truncated-banner"
        >
          <span>
            {result.streamed
              ? `Showing the first ${fmt.format(STREAM_MAX_ROWS)} rows, the most the page holds.`
              : `Showing the first ${fmt.format(result.rows.length)} rows. The result has more.`}
          </span>
          {!result.streamed && (
            <Button size="sm" onClick={onStreamAll} disabled={streaming}>
              {streaming ? 'Streaming…' : 'Stream all'}
            </Button>
          )}
        </div>
      )}
      {result.notifications.length > 0 && (
        <ul className="m-0 flex list-none flex-col gap-1 p-0 text-sm" aria-label="Notifications">
          {result.notifications.map((n, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: notifications have no id
            <li key={i}>
              <span className="font-mono">{n.gqlstatus}</span> {n.message}
            </li>
          ))}
        </ul>
      )}
      {hasGraph && (
        <div className="flex gap-1 border-0 border-b border-solid border-rule" role="tablist">
          {(['table', 'graph'] as const).map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              id={`${base}-${t}`}
              aria-selected={view === t}
              aria-controls={`${base}-panel`}
              className={`cursor-pointer border-0 border-b-2 border-solid bg-transparent px-4 py-2 font-sans text-sm ${
                view === t ? 'border-accent text-ink' : 'border-transparent text-muted'
              }`}
              onClick={() => setTab(t)}
            >
              {t === 'table' ? 'Table' : 'Graph'}
            </button>
          ))}
        </div>
      )}
      {hasGraph ? (
        <div id={`${base}-panel`} role="tabpanel" aria-labelledby={`${base}-${view}`}>
          {view === 'graph' ? <GraphView elements={elements} /> : table}
        </div>
      ) : (
        table
      )}
    </section>
  );
}

function Operator({ op, profile }: { op: graph.PlanOperator; profile: boolean }) {
  const detail = op.details.operator ?? op.name;
  return (
    <li>
      <span className="font-mono">{detail}</span>
      {profile && (
        <span className="ml-2 text-muted text-xs">
          {fmt.format(Number(op.rows))} rows · {elapsed(op.elapsedNanos)}
        </span>
      )}
      {op.children.length > 0 && (
        <ul className="m-0 list-none border-0 border-l border-solid border-rule pl-4">
          {op.children.map((c, i) => (
            // biome-ignore lint/suspicious/noArrayIndexKey: operators have no id
            <Operator key={i} op={c} profile={profile} />
          ))}
        </ul>
      )}
    </li>
  );
}

function PlanView({ result }: { result: PlanResult }) {
  const root = result.plan.root;
  return (
    <section className="flex flex-col gap-3" aria-label={result.profile ? 'Profile' : 'Plan'}>
      <h3 className="m-0 text-base">{result.profile ? 'Profile' : 'Plan'}</h3>
      {root ? (
        <ul className="m-0 list-none p-0 text-sm" aria-label="Plan operators">
          <Operator op={root} profile={result.profile} />
        </ul>
      ) : (
        <p className="m-0 text-muted">The engine answered no plan.</p>
      )}
      {result.plan.text && (
        <details>
          <summary className="cursor-pointer text-sm text-muted">Engine text</summary>
          <pre className="m-0 overflow-auto p-3 font-mono text-xs">{result.plan.text}</pre>
        </details>
      )}
    </section>
  );
}
