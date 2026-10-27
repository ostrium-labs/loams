// `/analytics`: the Analytics (House) page (§49 §18.3, D778). It asks whether the
// server lists `loams.house.v1.HouseService`, then shows the SQL editor, the settings
// panel, the streaming results grid and the server's query history.
//
// Runs are read-only by default (`read_only = true`, `max_rows = 1 000`). With "Allow
// writes" on, a statement the server refuses with `house_read_only` asks for
// confirmation and runs again with `read_only = false`; with it off, the refusal is
// shown. Refusals from the classifier and deny list (INTO OUTFILE, FROM INFILE, host
// table functions, settings off the allowlist) are shown with their ClickHouse code.

import { PageHead } from '@loams/desktop-ui';
import {
  Badge,
  Button,
  Checkbox,
  ComingSoon,
  Dialog,
  Empty,
  formatSql,
  isNotServed,
  Notice,
  QueryResults,
  SampleDataNotice,
  SchemaPanel,
  Select,
  SkeletonList,
  SqlEditor,
  type SqlEditorHandle,
  SqlExamples,
  type SqlFunction,
  type SqlSchema,
  type SqlTable,
  SqlToolbar,
  sqlErrorPosition,
  sqlExamples,
} from '@loams/ui';
import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import {
  type Cell,
  type ColumnHeader,
  type Database,
  type HouseClient,
  HouseError,
  type HouseFailure,
  newQueryId,
  PAGE_MAX_ROWS,
  PAGE_TIMEOUT_MS,
  type Progress,
  SERVER_MAX_ROWS,
  type Summary,
  toFailure,
} from './client.js';
import { FailureView } from './FailureView.js';
import { HistoryPanel } from './HistoryPanel.js';
import { SettingsPanel } from './SettingsPanel.js';
import { type SettingRow, settingsMap } from './settings.js';

export const PACKAGE = 'loams.house.v1';
/** A ClickHouse string literal. */
const chString = (s: string) => `'${s.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`;
const fmt = new Intl.NumberFormat('en');

export interface HousePageProps {
  client: HouseClient;
  /** The client answers from built-in sample data, not a server. */
  sample?: boolean;
  /** The server this page talks to, so the chosen database is remembered per server. */
  serverKey?: string;
}

/** Databases a person does not pick from: ClickHouse's own catalogs. */
const HIDDEN_DATABASES = new Set(['system', 'INFORMATION_SCHEMA', 'information_schema']);

const DATABASE_KEY = 'loams.house.database';

/** The database last chosen on `serverKey`, if storage allows. */
export function rememberedDatabase(serverKey: string): string | undefined {
  try {
    const raw = JSON.parse(globalThis.localStorage?.getItem(DATABASE_KEY) ?? '{}');
    const name = raw && typeof raw === 'object' ? raw[serverKey] : undefined;
    return typeof name === 'string' && name ? name : undefined;
  } catch {
    return undefined;
  }
}

function rememberDatabase(serverKey: string, name: string) {
  try {
    const raw = JSON.parse(globalThis.localStorage?.getItem(DATABASE_KEY) ?? '{}');
    const all = raw && typeof raw === 'object' && !Array.isArray(raw) ? raw : {};
    globalThis.localStorage?.setItem(DATABASE_KEY, JSON.stringify({ ...all, [serverKey]: name }));
  } catch {
    // Storage is a convenience: the page still works on `default`.
  }
}

/**
 * The databases to offer: `ListDatabases`, else (a House that does not serve it, or
 * lists none) the names in `system.databases`, read from `default` like the function list.
 */
async function loadDatabases(
  client: HouseClient,
  signal: AbortSignal,
  onNotServed: () => void,
): Promise<Database[]> {
  try {
    const page = await client.listDatabases();
    if (page.items.length) return page.items;
  } catch (e) {
    // No House at all is the page's "coming soon"; a House without ListDatabases
    // (`unimplemented`, `not_implemented`) still has system.databases.
    if (toFailure(e).reason?.endsWith('not_configured')) {
      onNotServed();
      return [];
    }
  }
  const names: Database[] = [];
  try {
    for await (const f of client.executeQuery(
      {
        sql: 'SELECT name FROM system.databases ORDER BY name',
        database: 'default',
        settings: {},
        params: {},
        format: 'ARROW_IPC',
        maxRows: SERVER_MAX_ROWS,
        timeoutMs: PAGE_TIMEOUT_MS,
        readOnly: true,
      },
      signal,
    )) {
      if (f.kind !== 'batch') continue;
      for (const [name] of f.rows)
        if (typeof name === 'string' && !HIDDEN_DATABASES.has(name))
          names.push({ name, tableCount: 0, comment: '' });
    }
  } catch {
    // No list: the page stays on `default`.
  }
  return names;
}

/**
 * Inside the workspace only the House's own word counts (house_not_configured): one
 * method a server leaves unimplemented (ListDatabases, say) is not the whole page.
 */
const houseNotServed = (f: HouseFailure) => isNotServed({ reason: f.reason });

type Detected = 'loading' | 'available' | 'absent' | 'not-served' | { error: string };

export function HousePage({ client, sample, serverKey = '' }: HousePageProps) {
  const [detected, setDetected] = useState<Detected>('loading');
  const [tick, setTick] = useState(0);
  const notServed = useCallback(() => setDetected('not-served'), []);

  // oxlint-disable-next-line react/exhaustive-deps -- tick retries on purpose
  useEffect(() => {
    let live = true;
    setDetected('loading');
    client.available().then(
      (ok) => live && setDetected(ok ? 'available' : 'absent'),
      (e) => {
        if (!live) return;
        const f = toFailure(e);
        setDetected(isNotServed(f) ? 'not-served' : { error: f.message });
      },
    );
    return () => {
      live = false;
    };
  }, [client, tick]);

  const head = <PageHead title="Analytics" badge={<Badge>{PACKAGE}</Badge>} />;
  if (detected === 'loading') {
    return (
      <div className="lc-page">
        {head}
        <SkeletonList label="Asking the server about Loams House" layout="cards" rows={3} />
      </div>
    );
  }
  if (typeof detected === 'object') {
    return (
      <div className="lc-page">
        {head}
        <Notice tone="danger" title="Could not reach the server">
          {detected.error}
        </Notice>
        <div>
          <Button onClick={() => setTick((t) => t + 1)}>Try again</Button>
        </div>
      </div>
    );
  }
  if (detected === 'absent') {
    return (
      <div className="lc-page">
        {head}
        <Empty title="Analytics is not configured on this server" seed={33}>
          Loams House answers ClickHouse SQL over {PACKAGE}. This server does not list it. Configure
          a House endpoint on the server, or switch to one that has it.
        </Empty>
      </div>
    );
  }
  if (detected === 'not-served') {
    // The engine lists House but answers house_not_configured (a preview without it):
    // a feature to come, so no error and no sample data.
    return (
      <div className="lc-page">
        {head}
        <ComingSoon feature="Analytics" seed={33} />
      </div>
    );
  }
  return (
    <Workspace client={client} sample={sample} serverKey={serverKey} onNotServed={notServed} />
  );
}

interface RunState {
  columns: ColumnHeader[];
  progress?: Progress;
  summary?: Summary;
  queryId: string;
  sql: string;
  maxRows: number;
}

function Workspace({
  client,
  sample,
  serverKey,
  onNotServed,
}: {
  client: HouseClient;
  sample?: boolean;
  serverKey: string;
  /** The House answered house_not_configured: the page shows "coming soon" instead. */
  onNotServed: () => void;
}) {
  const ids = useId();
  const failureId = `${ids}-failure`;
  const [databases, setDatabases] = useState<Database[]>([]);
  const [database, setDatabase] = useState(() => rememberedDatabase(serverKey) ?? 'default');
  const chooseDatabase = (name: string) => {
    setDatabase(name);
    rememberDatabase(serverKey, name);
  };
  // No query to start with: the examples below the editor are built from the tables this
  // House has, so the first run never names a table that does not exist.
  const [sql, setSql] = useState('');
  const [failedSql, setFailedSql] = useState<string>();
  const [tables, setTables] = useState<SqlTable[]>([]);
  const [schemaError, setSchemaError] = useState<string>();
  const [schemaLoading, setSchemaLoading] = useState(true);
  const editor = useRef<SqlEditorHandle>(null);
  const [settings, setSettings] = useState<SettingRow[]>([]);
  const [allowWrites, setAllowWrites] = useState(false);
  const [busy, setBusy] = useState(false);
  const [run, setRun] = useState<RunState>();
  const rowsRef = useRef<Cell[][]>([]);
  const [, setVersion] = useState(0);
  const [failure, setFailure] = useState<HouseFailure>();
  const [notice, setNotice] = useState<string>();
  const [plan, setPlan] = useState<string>();
  const [confirm, setConfirm] = useState<{ sql: string }>();
  const [historyTick, setHistoryTick] = useState(0);
  const [functions, setFunctions] = useState<SqlFunction[]>([]);
  const abort = useRef<AbortController | undefined>(undefined);
  const queryId = useRef<string | undefined>(undefined);

  useEffect(() => () => abort.current?.abort(), []);

  useEffect(() => {
    let live = true;
    const ac = new AbortController();
    void loadDatabases(client, ac.signal, () => live && onNotServed()).then((items) => {
      if (!live || !items.length) return;
      setDatabases(items);
      // The remembered database while the server still has it, else `default`, else the first.
      setDatabase((current) =>
        items.some((d) => d.name === current)
          ? current
          : items.some((d) => d.name === 'default')
            ? 'default'
            : (items[0]?.name ?? 'default'),
      );
    });
    // The function list, fetched once through ExecuteQuery (§49 §18.3). The House runs a
    // statement only in a database it serves (it refuses `system`), so the qualified
    // system table is read from `default`.
    (async () => {
      const names: SqlFunction[] = [];
      try {
        for await (const f of client.executeQuery(
          {
            sql: 'SELECT name, is_aggregate FROM system.functions ORDER BY name',
            database: 'default',
            settings: {},
            params: {},
            format: 'ARROW_IPC',
            maxRows: SERVER_MAX_ROWS,
            timeoutMs: PAGE_TIMEOUT_MS,
            readOnly: true,
          },
          ac.signal,
        )) {
          if (f.kind === 'batch')
            for (const r of f.rows)
              if (typeof r[0] === 'string')
                names.push({
                  name: r[0],
                  kind: r[1] === 1 || r[1] === true ? 'aggregate' : 'scalar',
                });
        }
      } catch {
        // completions are a convenience: keywords alone still work
      }
      if (live) setFunctions(names);
    })();
    return () => {
      live = false;
      // Leaving the page stops the function-list stream rather than reading it out.
      ac.abort();
    };
  }, [client, onNotServed]);

  // The selected database's tables and typed columns, for completion, the schema panel and
  // the examples (system.columns, read-only, through the same ExecuteQuery).
  useEffect(() => {
    let live = true;
    const ac = new AbortController();
    setSchemaLoading(true);
    setSchemaError(undefined);
    (async () => {
      const byTable = new Map<string, SqlTable>();
      try {
        for await (const f of client.executeQuery(
          {
            sql: `SELECT table, name, type FROM system.columns WHERE database = ${chString(database)} ORDER BY table, position`,
            database,
            settings: {},
            params: {},
            format: 'ARROW_IPC',
            maxRows: SERVER_MAX_ROWS,
            timeoutMs: PAGE_TIMEOUT_MS,
            readOnly: true,
          },
          ac.signal,
        )) {
          if (f.kind !== 'batch') continue;
          for (const [table, name, type] of f.rows) {
            if (typeof table !== 'string' || typeof name !== 'string') continue;
            let t = byTable.get(table);
            if (!t) {
              t = { name: table, schema: database, kind: 'table', columns: [] };
              byTable.set(table, t);
            }
            t.columns.push({ name, type: typeof type === 'string' ? type : undefined });
          }
        }
        if (live) setTables([...byTable.values()]);
      } catch (e) {
        if (live) setSchemaError(`Could not read the tables: ${toFailure(e).message}`);
      } finally {
        if (live) setSchemaLoading(false);
      }
    })();
    return () => {
      live = false;
      ac.abort();
    };
  }, [client, database]);

  const schema = useMemo<SqlSchema>(
    () => ({ tables, functions, defaultSchema: database }),
    [tables, functions, database],
  );
  const examples = useMemo(() => sqlExamples(schema, 'clickhouse'), [schema]);

  const execute = useCallback(
    async (text: string, readOnly: boolean, maxRows = PAGE_MAX_ROWS) => {
      const s = settingsMap(settings);
      if (!s.ok) {
        setFailure({
          code: 'invalid_argument',
          message: s.problem,
          reason: 'house_access_denied',
          metadata: {},
        });
        return;
      }
      abort.current?.abort();
      const c = new AbortController();
      abort.current = c;
      const id = newQueryId();
      queryId.current = id;
      rowsRef.current = [];
      setVersion((v) => v + 1);
      setBusy(true);
      setFailure(undefined);
      setFailedSql(undefined);
      setNotice(undefined);
      setPlan(undefined);
      setRun({ columns: [], queryId: id, sql: text, maxRows });
      try {
        for await (const f of client.executeQuery(
          {
            sql: text,
            database,
            settings: s.settings,
            params: {},
            format: 'ARROW_IPC',
            maxRows,
            timeoutMs: PAGE_TIMEOUT_MS,
            queryId: id,
            readOnly,
          },
          c.signal,
        )) {
          if (c.signal.aborted) break;
          if (f.kind === 'header') setRun((r) => r && { ...r, columns: f.columns });
          else if (f.kind === 'batch') {
            rowsRef.current.push(...f.rows);
            setVersion((v) => v + 1);
          } else if (f.kind === 'progress') setRun((r) => r && { ...r, progress: f.progress });
          else setRun((r) => r && { ...r, summary: f.summary });
        }
        if (c.signal.aborted && abort.current === c)
          setNotice('Cancelled. The House stops the statement.');
      } catch (e) {
        const f = toFailure(e);
        // A run that a newer one replaced reports nothing.
        if (abort.current !== c) return;
        if (houseNotServed(f)) {
          onNotServed();
          return;
        }
        if (f.reason === 'house_cancelled' || c.signal.aborted) {
          setNotice('Cancelled. The House stops the statement.');
        } else if (
          e instanceof HouseError &&
          f.reason === 'house_read_only' &&
          readOnly &&
          allowWrites
        ) {
          // The server classified it as a write: ask, then run it writable.
          setRun(undefined);
          setConfirm({ sql: text });
        } else {
          setRun(undefined);
          setFailure(f);
          setFailedSql(text);
        }
      } finally {
        if (abort.current === c) {
          setBusy(false);
          abort.current = undefined;
        }
        setHistoryTick((t) => t + 1);
      }
    },
    [client, database, settings, allowWrites, onNotServed],
  );

  const cancel = () => {
    const id = queryId.current;
    abort.current?.abort();
    if (id) void client.cancelQuery(id).catch(() => undefined);
  };

  const explain = async () => {
    setFailure(undefined);
    setNotice(undefined);
    try {
      const res = await client.explainQuery({ sql, database, kind: 'PLAN' });
      setRun(undefined);
      setPlan(res.text);
    } catch (e) {
      const f = toFailure(e);
      if (houseNotServed(f)) onNotServed();
      else setFailure(f);
    }
  };

  const rows = rowsRef.current;
  const summary = run?.summary;
  const progress = run?.progress;
  const pct =
    progress && progress.totalRowsToRead > 0
      ? Math.min(100, Math.round((progress.readRows / progress.totalRowsToRead) * 100))
      : undefined;

  return (
    <div className="lc-page">
      <PageHead
        title="Analytics"
        badge={<Badge>{PACKAGE}</Badge>}
        subtitle="ClickHouse SQL over Loams House: lake and realtime tables, read-only by default."
        actions={
          databases.length > 0 && (
            <label className="flex items-center gap-2 text-body" htmlFor={`${ids}-db`}>
              <span className="text-muted">Database</span>
              <Select
                id={`${ids}-db`}
                className="loams-input-compact"
                value={database}
                onChange={(e) => chooseDatabase(e.target.value)}
              >
                {databases.some((d) => d.name === database) ? null : (
                  <option value={database}>{database}</option>
                )}
                {databases.map((d) => (
                  <option key={d.name} value={d.name}>
                    {d.name}
                  </option>
                ))}
              </Select>
            </label>
          )
        }
      />
      {sample && (
        <SampleDataNotice>
          The server does not serve {PACKAGE} yet. Nothing you run here reaches a server.
        </SampleDataNotice>
      )}
      <div className="grid items-start gap-6 xl:grid-cols-[minmax(0,1fr)_300px]">
        <div className="flex min-w-0 flex-col gap-4">
          <section aria-label="Editor" className="loams-sql-frame">
            <SqlToolbar
              busy={busy}
              disabled={!sql.trim()}
              onRun={() => void execute(sql, true)}
              onRunSelection={() =>
                void execute(editor.current?.selectionOrStatement() || sql, true)
              }
              onFormat={() => editor.current?.replace(formatSql(sql))}
              onCancel={cancel}
            >
              <Button size="sm" onClick={() => void explain()} disabled={busy || !sql.trim()}>
                Explain
              </Button>
              <Checkbox
                label="Allow writes"
                checked={allowWrites}
                onChange={(e) => setAllowWrites(e.target.checked)}
              />
            </SqlToolbar>
            <SqlEditor
              handle={editor}
              label="SQL"
              dialect="clickhouse"
              schema={schema}
              value={sql}
              onChange={setSql}
              onRun={(text) => {
                if (!busy && text.trim()) void execute(text, true);
              }}
              describedBy={failure ? failureId : undefined}
              invalid={!!failure}
              errorAt={
                failure && failedSql !== undefined && failedSql === sql
                  ? sqlErrorPosition(failure.message, failedSql)
                  : undefined
              }
              placeholder="SELECT … FROM a table of this House. Ctrl+Space completes."
            />
          </section>

          {failure && <FailureView id={failureId} failure={failure} writesAllowed={allowWrites} />}
          {notice && <Notice title={notice} />}

          {busy && (
            <div className="flex items-center gap-3 text-body text-muted" role="status">
              <span>Running… {progress ? `${fmt.format(progress.readRows)} rows read` : ''}</span>
              {pct !== undefined && (
                <progress
                  max={100}
                  value={pct}
                  aria-label="Rows read of the estimate"
                  className="w-40"
                />
              )}
            </div>
          )}

          {plan !== undefined && (
            <section aria-label="Plan" className="flex flex-col gap-2">
              <h3 className="m-0 text-emphasis">Plan</h3>
              <pre className="m-0 overflow-auto rounded-md bg-surface p-3 font-mono text-small">
                {plan}
              </pre>
            </section>
          )}

          {!run && !failure && !busy && plan === undefined && (
            <SqlExamples
              examples={examples}
              onPick={(text) => {
                editor.current?.replace(text);
                void execute(text, true);
              }}
            />
          )}

          {run && (run.columns.length > 0 || summary) && (
            <div className="flex flex-col gap-3">
              {summary?.truncated && (
                <div
                  className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-solid border-rule bg-accent-soft px-3 py-2"
                  role="status"
                  data-testid="truncated-banner"
                >
                  <span>
                    Showing the first {fmt.format(rows.length)} rows. The result has more.
                  </span>
                  {run.maxRows < SERVER_MAX_ROWS && (
                    <Button
                      size="sm"
                      disabled={busy}
                      onClick={() => void execute(run.sql, true, SERVER_MAX_ROWS)}
                    >
                      Stream up to {fmt.format(SERVER_MAX_ROWS)} rows
                    </Button>
                  )}
                </div>
              )}
              <QueryResults
                columns={run.columns.map((c) => ({ name: c.name, type: c.clickhouseType }))}
                rows={rows}
                elapsedMs={summary?.elapsedMs}
                extra={
                  summary
                    ? `${fmt.format(summary.readRows)} rows read${
                        summary.writtenRows > 0
                          ? ` · ${fmt.format(summary.writtenRows)} written`
                          : ''
                      }`
                    : undefined
                }
              />
            </div>
          )}
        </div>

        <aside className="flex flex-col gap-6" aria-label="Schema, settings and history">
          <SchemaPanel
            title={`Tables in ${database}`}
            schema={schema}
            loading={schemaLoading}
            error={schemaError}
            onInsert={(name) => editor.current?.insert(name)}
          />
          <SettingsPanel rows={settings} onChange={setSettings} />
          <HistoryPanel client={client} refresh={historyTick} onOpen={setSql} />
        </aside>
      </div>

      <Dialog
        open={!!confirm}
        onClose={() => setConfirm(undefined)}
        title="Run a statement that changes data?"
        footer={
          <>
            <Button onClick={() => setConfirm(undefined)}>Cancel</Button>
            <Button
              variant="danger"
              onClick={() => {
                const text = confirm?.sql;
                setConfirm(undefined);
                if (text) void execute(text, false);
              }}
            >
              Run
            </Button>
          </>
        }
      >
        <p className="m-0 text-body">
          The House says this statement writes. It runs once more, with writes allowed, on{' '}
          <span className={/* mono-ok: id */ 'font-mono'}>{database}</span>.
        </p>
        <pre className="m-0 mt-2 max-h-48 overflow-auto rounded-md bg-surface p-2 font-mono text-small">
          {confirm?.sql}
        </pre>
      </Dialog>
    </div>
  );
}
