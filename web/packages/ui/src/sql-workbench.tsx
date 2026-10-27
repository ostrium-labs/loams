// The pieces around the query editor that every query page shares, and `SqlWorkbench`, which
// puts them together for a page whose engine answers a statement with one result: the
// toolbar (Run, Run selection, Format, History) above the editor, so the completion list,
// which opens downwards, never covers it; the error with the line and column the engine
// named; the results; and, before the first run, example queries built from the schema.

import {
  type ReactNode,
  type Ref,
  useCallback,
  useEffect,
  useId,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from 'react';
import { Button } from './button';
import { Kbd, KbdGroup } from './kbd';
import { Menu, MenuItem, MenuLabel, MenuSeparator } from './menu';
import { type HistoryItem, readHistory, useQueryHistory } from './query-history';
import { QueryResults, type ResultColumn } from './query-results';
import { SqlEditor, type SqlEditorHandle } from './sql-editor';
import { cleanSqlError, type SqlPosition, sqlErrorPosition } from './sql-error';
import { formatSql } from './sql-format';
import type { SqlColumn, SqlDialectName, SqlSchema, SqlTable } from './sql-language';

// ---------- examples ----------

export interface SqlExample {
  label: string;
  sql: string;
}

const SIMPLE = /^[A-Za-z_][\w]*$/;

function quote(name: string, dialect: SqlDialectName): string {
  if (SIMPLE.test(name)) return name;
  return dialect === 'mysql' || dialect === 'clickhouse'
    ? `\`${name.replace(/`/g, '``')}\``
    : `"${name.replace(/"/g, '""')}"`;
}

const isText = (c: SqlColumn) =>
  !!c.type && /(utf8|string|varchar|text|char|enum)/i.test(c.type) && !/^Array|List/i.test(c.type);
const isNumber = (c: SqlColumn) =>
  !!c.type &&
  /^(nullable\()?(u?int|float|double|decimal|numeric|real|bigint|smallint|integer)/i.test(c.type);
const visible = (c: SqlColumn) => !c.name.startsWith('_');

/** The first column matching the preferred names, in their order, else the first that fits. */
function pick(cols: SqlColumn[], prefer: string[], filter: (c: SqlColumn) => boolean) {
  const ok = cols.filter((c) => visible(c) && filter(c));
  for (const p of prefer) {
    const re = new RegExp(`^${p}$`, 'i');
    const hit = ok.find((c) => re.test(c.name));
    if (hit) return hit;
  }
  return ok[0];
}

/** Example queries over the tables a page actually has. */
export function sqlExamples(schema: SqlSchema, dialect: SqlDialectName): SqlExample[] {
  const tables = schema.tables.filter(
    (t) =>
      !/^(information_schema|system|pg_catalog|mysql|performance_schema|metrics_schema)$/i.test(
        t.schema ?? '',
      ),
  );
  const count = dialect === 'clickhouse' ? 'count()' : 'count(*)';
  const out: SqlExample[] = [];
  // The widest table first: a fact table makes better examples than a lookup table.
  tables.sort((a, b) => b.columns.length - a.columns.length);
  const first = tables[0] as SqlTable | undefined;
  if (!first) {
    if (dialect === 'mysql') out.push({ label: 'List the tables', sql: 'SHOW TABLES;' });
    if (dialect === 'clickhouse') out.push({ label: 'List the tables', sql: 'SHOW TABLES' });
    if (dialect === 'datafusion' || dialect === 'postgres')
      out.push({
        label: 'List the tables',
        sql: "SELECT table_schema, table_name\nFROM information_schema.tables\nWHERE table_schema NOT IN ('information_schema', 'pg_catalog')",
      });
    out.push({ label: 'A first query', sql: 'SELECT 1' });
    return out;
  }
  const t = quote(first.name, dialect);
  out.push({ label: `Rows of ${first.name}`, sql: `SELECT *\nFROM ${t}\nLIMIT 10` });
  out.push({ label: `Count ${first.name}`, sql: `SELECT ${count} AS n\nFROM ${t}` });
  const group = pick(
    first.columns,
    [
      'type',
      'kind',
      'status',
      'category',
      'letter',
      'lang',
      'language',
      'country',
      'org',
      'repo',
      'vendor.*',
      'genre',
      'payment_type',
      '.*_type',
    ],
    (c) => isText(c) || /^(u?int8|tinyint)/i.test(c.type ?? '') || /type$/i.test(c.name),
  );
  const measure = pick(
    first.columns,
    [
      '.*amount.*',
      '.*price.*',
      '.*fare.*',
      '.*total.*',
      '.*distance.*',
      '.*duration.*',
      '.*chars.*',
      '.*size.*',
      '.*bytes.*',
    ],
    isNumber,
  );
  if (group) {
    const g = quote(group.name, dialect);
    const avg = measure ? `,\n  avg(${quote(measure.name, dialect)}) AS avg_${measure.name}` : '';
    out.push({
      label: `${first.name} by ${group.name}`,
      sql: `SELECT\n  ${g},\n  ${count} AS n${avg}\nFROM ${t}\nGROUP BY ${g}\nORDER BY n DESC\nLIMIT 10`,
    });
  }
  if (dialect === 'datafusion') {
    const text = pick(
      first.columns,
      ['body', 'text', 'content', 'description', 'abstract', 'summary', 'title'],
      isText,
    );
    const show = pick(first.columns, ['title', 'name', 'headline'], isText) ?? text;
    if (text)
      out.push({
        label: `Full-text search ${first.name}`,
        sql: `SELECT _id, ${show ? quote(show.name, dialect) : '*'}, _score\nFROM text_search('${first.name.replace(/'/g, "''")}', 'search terms', '${text.name}', 10)`,
      });
  }
  const second = tables[1];
  if (second)
    out.push({
      label: `Rows of ${second.name}`,
      sql: `SELECT *\nFROM ${quote(second.name, dialect)}\nLIMIT 10`,
    });
  return out;
}

export function SqlExamples({
  examples,
  onPick,
  title = 'Start with an example',
}: {
  examples: SqlExample[];
  onPick: (sql: string) => void;
  title?: string;
}) {
  if (examples.length === 0) return null;
  return (
    <section className="loams-sql-examples" aria-label="Example queries">
      <p className="loams-sql-examples-title">{title}</p>
      <ul>
        {examples.map((e) => (
          <li key={e.label}>
            <button type="button" className="loams-sql-example" onClick={() => onPick(e.sql)}>
              <span className="loams-sql-example-label">{e.label}</span>
              <code>{e.sql.replace(/\s+/g, ' ')}</code>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

// ---------- error ----------

export function QueryErrorView({
  id,
  title = 'The query failed',
  message,
  statement,
  position,
  detail,
}: {
  id?: string;
  title?: string;
  message: string;
  statement?: string;
  position?: SqlPosition;
  detail?: ReactNode;
}) {
  const line = position && statement?.split('\n')[position.line - 1];
  return (
    <div id={id} className="loams-query-error" role="alert">
      <p className="loams-query-error-title">
        <strong>{title}</strong>
        {position && (
          <span className="loams-query-error-where">
            {' '}
            · line {position.line}, column {position.column}
          </span>
        )}
      </p>
      <p className="loams-query-error-message">{message}</p>
      {detail}
      {typeof line === 'string' && position && (
        <pre className="loams-query-error-span" data-testid="error-span">
          <span className="loams-query-error-gutter" aria-hidden="true">
            {position.line} │{' '}
          </span>
          {line.slice(0, position.column - 1)}
          <mark>
            {line.slice(position.column - 1, position.column - 1 + (position.length ?? 1)) || ' '}
          </mark>
          {line.slice(position.column - 1 + (position.length ?? 1))}
        </pre>
      )}
    </div>
  );
}

// ---------- toolbar ----------

const isMac = () => /Mac|iPhone|iPad/.test(globalThis.navigator?.platform ?? '');

export function RunHint({ selection = true }: { selection?: boolean }) {
  const mod = isMac() ? '⌘' : 'Ctrl';
  return (
    <span className="loams-sql-hint">
      <KbdGroup>
        <Kbd>{mod}</Kbd>
        <Kbd>Enter</Kbd>
      </KbdGroup>{' '}
      runs ·{' '}
      {selection && (
        <>
          <KbdGroup>
            <Kbd>{mod}</Kbd>
            <Kbd>Shift</Kbd>
            <Kbd>Enter</Kbd>
          </KbdGroup>{' '}
          runs the selection ·{' '}
        </>
      )}
      <KbdGroup>
        <Kbd>Ctrl</Kbd>
        <Kbd>Space</Kbd>
      </KbdGroup>{' '}
      completes
    </span>
  );
}

function ago(at: number): string {
  const s = Math.max(0, Math.round((Date.now() - at) / 1000));
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

export function HistoryMenu({
  items,
  onPick,
  onClear,
}: {
  items: HistoryItem[];
  onPick: (text: string) => void;
  onClear?: () => void;
}) {
  return (
    <Menu
      label="Query history"
      align="end"
      trigger={
        <Button size="sm" variant="quiet" disabled={items.length === 0}>
          History{items.length ? ` (${items.length})` : ''}
        </Button>
      }
    >
      <MenuLabel>Recent queries in this browser</MenuLabel>
      {items.slice(0, 15).map((h) => (
        <MenuItem key={`${h.at}-${h.text}`} onSelect={() => onPick(h.text)}>
          <span className="loams-history-item">
            <code>{h.text.replace(/\s+/g, ' ').slice(0, 90)}</code>
            <span className="loams-history-meta">
              {h.ok ? '' : 'failed · '}
              {ago(h.at)}
            </span>
          </span>
        </MenuItem>
      ))}
      {onClear && (
        <>
          <MenuSeparator />
          <MenuItem onSelect={onClear} tone="danger">
            Clear history
          </MenuItem>
        </>
      )}
    </Menu>
  );
}

export function SqlToolbar({
  onRun,
  onRunSelection,
  onFormat,
  onCancel,
  busy,
  disabled,
  history,
  children,
  hint = <RunHint />,
}: {
  onRun: () => void;
  onRunSelection?: () => void;
  onFormat?: () => void;
  onCancel?: () => void;
  busy?: boolean;
  disabled?: boolean;
  history?: ReactNode;
  /** Page-specific controls (Explain, a database picker, a read-only switch). */
  children?: ReactNode;
  hint?: ReactNode;
}) {
  return (
    <div className="loams-sql-toolbar" role="toolbar" aria-label="Query actions">
      <Button variant="primary" size="sm" onClick={onRun} disabled={busy || disabled}>
        {busy ? 'Running…' : 'Run'}
      </Button>
      {onRunSelection && (
        <Button size="sm" onClick={onRunSelection} disabled={busy || disabled}>
          Run selection
        </Button>
      )}
      {busy && onCancel && (
        <Button size="sm" variant="quiet" onClick={onCancel}>
          Cancel
        </Button>
      )}
      {children}
      <span className="loams-sql-toolbar-spacer" />
      {hint}
      {onFormat && (
        <Button size="sm" variant="quiet" onClick={onFormat} disabled={busy}>
          Format
        </Button>
      )}
      {history}
    </div>
  );
}

// ---------- the workbench ----------

export interface QueryOutcome {
  columns: ResultColumn[];
  rows: readonly (readonly unknown[])[];
  truncated?: boolean;
  rowsAffected?: number;
  /** The server's own time; the workbench measures the round trip when absent. */
  elapsedMs?: number;
}

type RunState =
  | { state: 'idle' }
  | { state: 'running'; text: string }
  | { state: 'done'; text: string; result: QueryOutcome; elapsedMs: number }
  | { state: 'error'; text: string; message: string; position?: SqlPosition };

export interface SqlWorkbenchProps {
  dialect: SqlDialectName;
  schema?: SqlSchema;
  /** Runs one statement. Rejects with an Error whose message is the engine's. */
  run: (text: string, signal: AbortSignal) => Promise<QueryOutcome>;
  /** Where the history is kept: a namespace, a database. */
  historyScope: string;
  /** The editor's accessible name. */
  label?: string;
  initial?: string;
  placeholder?: string;
  /** Run stays disabled (the database is suspended, say); `disabledReason` says why. */
  disabled?: boolean;
  disabledReason?: ReactNode;
  toolbarExtra?: ReactNode;
  copy?: (text: string) => void | Promise<void>;
  /** Turns a thrown error into its message. */
  errorText?: (e: unknown) => string;
  /** A new key resets the result (a new branch, a new database). */
  resetKey?: string;
  /** Called with the outcome of each run (for a page that shows more around it). */
  onResult?: (text: string, outcome: QueryOutcome | Error) => void;
  minLines?: number;
  /** The editor, for a schema panel that inserts names at the cursor. */
  handle?: Ref<SqlEditorHandle>;
  /** Beside the editor and results (a schema panel). */
  aside?: ReactNode;
}

export function SqlWorkbench({
  dialect,
  schema,
  run,
  historyScope,
  label = 'SQL',
  initial,
  placeholder,
  disabled,
  disabledReason,
  toolbarExtra,
  copy,
  errorText = (e) => (e instanceof Error ? e.message : String(e)),
  resetKey,
  onResult,
  minLines,
  handle,
  aside,
}: SqlWorkbenchProps) {
  const ids = useId();
  const errorId = `${ids}-error`;
  const { items, push, clear } = useQueryHistory(historyScope);
  const [text, setText] = useState(() => initial ?? readHistory(historyScope)[0]?.text ?? '');
  const [out, setOut] = useState<RunState>({ state: 'idle' });
  const editor = useRef<SqlEditorHandle>(null);
  useImperativeHandle(handle, () => ({
    focus: () => editor.current?.focus(),
    selectionOrStatement: () => editor.current?.selectionOrStatement() ?? '',
    replace: (t: string) => editor.current?.replace(t),
    insert: (t: string) => editor.current?.insert(t),
  }));
  const abort = useRef<AbortController | null>(null);

  // oxlint-disable-next-line react/exhaustive-deps -- reset on a new target
  useEffect(() => {
    abort.current?.abort();
    abort.current = null;
    setOut({ state: 'idle' });
  }, [resetKey]);
  useEffect(() => () => abort.current?.abort(), []);

  const exec = useCallback(
    async (statement: string) => {
      const q = statement.trim();
      if (!q || disabled) return;
      abort.current?.abort();
      const ac = new AbortController();
      abort.current = ac;
      setOut({ state: 'running', text: q });
      const t0 = performance.now();
      try {
        const result = await run(q, ac.signal);
        if (abort.current !== ac) return;
        const elapsedMs = result.elapsedMs ?? performance.now() - t0;
        setOut({ state: 'done', text: q, result, elapsedMs });
        push({ text: q, at: Date.now(), ok: true, elapsedMs, rows: result.rows.length });
        onResult?.(q, result);
      } catch (e) {
        if (abort.current !== ac) return;
        const raw = errorText(e);
        const message = cleanSqlError(raw);
        setOut({ state: 'error', text: q, message, position: sqlErrorPosition(raw, q) });
        push({ text: q, at: Date.now(), ok: false });
        onResult?.(q, e instanceof Error ? e : new Error(raw));
      } finally {
        if (abort.current === ac) abort.current = null;
      }
    },
    [run, disabled, push, errorText, onResult],
  );

  const cancel = () => {
    abort.current?.abort();
    abort.current = null;
    setOut({ state: 'idle' });
  };

  const examples = useMemo(() => sqlExamples(schema ?? { tables: [] }, dialect), [schema, dialect]);
  const busy = out.state === 'running';
  // The error marks the editor only while it shows the statement that failed.
  const failed = out.state === 'error' ? out : undefined;
  const markError = failed && failed.text === text.trim() ? failed.position : undefined;

  const main = (
    <div className="loams-sql-workbench">
      <div className="loams-sql-frame">
        <SqlToolbar
          busy={busy}
          disabled={disabled || !text.trim()}
          onRun={() => void exec(text)}
          onRunSelection={() => void exec(editor.current?.selectionOrStatement() ?? text)}
          onFormat={dialect === 'gql' ? undefined : () => editor.current?.replace(formatSql(text))}
          onCancel={cancel}
          history={
            <HistoryMenu items={items} onPick={(t) => editor.current?.replace(t)} onClear={clear} />
          }
        >
          {toolbarExtra}
        </SqlToolbar>
        <SqlEditor
          handle={editor}
          value={text}
          onChange={setText}
          onRun={(t) => void exec(t)}
          dialect={dialect}
          schema={schema}
          label={label}
          describedBy={failed ? errorId : undefined}
          invalid={!!failed}
          errorAt={markError}
          placeholder={placeholder ?? 'Write a query, or pick an example below.'}
          minLines={minLines}
        />
      </div>
      {disabled && disabledReason && <p className="loams-sql-disabled">{disabledReason}</p>}
      <div aria-live="polite">
        {busy && (
          <p className="loams-sql-running" role="status">
            Running…
          </p>
        )}
      </div>
      {failed && (
        <QueryErrorView
          id={errorId}
          message={failed.message}
          statement={failed.text}
          position={failed.position}
        />
      )}
      {out.state === 'done' && (
        <QueryResults
          columns={out.result.columns}
          rows={out.result.rows}
          truncated={out.result.truncated}
          rowsAffected={out.result.rowsAffected}
          elapsedMs={out.elapsedMs}
          copy={copy}
        />
      )}
      {out.state === 'idle' && (
        <SqlExamples
          examples={examples}
          onPick={(sql) => {
            editor.current?.replace(sql);
            void exec(sql);
          }}
        />
      )}
    </div>
  );
  if (!aside) return main;
  return (
    <div className="loams-sql-layout">
      {main}
      {aside}
    </div>
  );
}

// ---------- schema panel ----------

/** The tables and their typed columns; a click inserts the name at the editor's cursor. */
export function SchemaPanel({
  schema,
  onInsert,
  title = 'Schema',
  loading,
  error,
}: {
  schema: SqlSchema;
  onInsert?: (name: string) => void;
  title?: string;
  loading?: boolean;
  error?: string;
}) {
  const [filter, setFilter] = useState('');
  const f = filter.trim().toLowerCase();
  const tables = schema.tables.filter(
    (t) =>
      !f ||
      t.name.toLowerCase().includes(f) ||
      t.columns.some((c) => c.name.toLowerCase().includes(f)),
  );
  return (
    <aside className="loams-schema-panel" aria-label={title}>
      <div className="loams-schema-head">
        <h3>{title}</h3>
        {schema.tables.length > 6 && (
          <input
            className="loams-input loams-schema-filter"
            aria-label="Filter tables and columns"
            placeholder="Filter"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
          />
        )}
      </div>
      {loading && <p className="loams-schema-note">Reading the schema…</p>}
      {error && <p className="loams-schema-note">{error}</p>}
      {!loading && !error && schema.tables.length === 0 && (
        <p className="loams-schema-note">No tables yet.</p>
      )}
      <ul className="loams-schema-tables">
        {tables.map((t, i) => (
          <li key={`${t.schema ?? ''}.${t.name}`}>
            <details open={i < 3 || !!f}>
              <summary>
                <span className="loams-schema-table">{t.name}</span>
                <span className="loams-schema-kind">{t.kind ?? t.schema ?? 'table'}</span>
              </summary>
              <ul className="loams-schema-columns">
                {t.columns
                  .filter(
                    (c) =>
                      !f || t.name.toLowerCase().includes(f) || c.name.toLowerCase().includes(f),
                  )
                  .map((c) => (
                    <li key={c.name}>
                      <button
                        type="button"
                        className="loams-schema-column"
                        onClick={() => onInsert?.(c.name)}
                        disabled={!onInsert}
                        title={onInsert ? `Insert ${c.name}` : undefined}
                      >
                        <span className="loams-schema-col">{c.name}</span>
                        {c.type && <span className="loams-schema-type">{c.type}</span>}
                      </button>
                    </li>
                  ))}
              </ul>
            </details>
          </li>
        ))}
      </ul>
    </aside>
  );
}
