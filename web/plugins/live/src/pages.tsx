// The Loams Live page: tables, documents, a live Watch query and mutations.
// When the engine runs without Live it shows the TiKV stack card instead.

import { Code, ConnectError } from '@connectrpc/connect';
import type { EngineState } from '@loams/desktop/contracts';
import type { LoamsDesktopApi } from '@loams/platform-electron';
import {
  Badge,
  Button,
  Card,
  Dialog,
  Empty,
  Field,
  Input,
  Notice,
  Select,
  StatusTag,
  Table,
  Textarea,
} from '@loams/ui';
import { type ReactNode, useCallback, useEffect, useRef, useState } from 'react';
import type { Doc, LiveApi, QueryArgs, TableInfo } from './client.js';
import { type Json, parseJson } from './value.js';

const PAGE = 50;
export const DEPLOY_TIP = 'Not yet available (R1 Task 13)';

export function errorText(e: unknown): string {
  return e instanceof ConnectError ? e.rawMessage : e instanceof Error ? e.message : String(e);
}

/**
 * Live is not running in the local engine: the protocol proxy answers 503 with
 * `{code: 'live_not_running', message}` (protocol/route.ts). Connect does not know that
 * code, so the error arrives as Unavailable with the proxy's message; the message is
 * what tells it from any other Unavailable (an engine or network fault).
 */
export const LIVE_NOT_RUNNING = 'Loams Live is not running in the local engine.';
/** The proxy's 503 while the local engine restarts (protocol/route.ts): transient, so poll. */
export const ENGINE_NOT_READY = 'the local engine is not ready yet';
const isStarting = (e: unknown) =>
  e instanceof ConnectError && e.code === Code.Unavailable && e.rawMessage === ENGINE_NOT_READY;
const isNotRunning = (e: unknown) =>
  e instanceof ConnectError && e.code === Code.Unavailable && e.rawMessage === LIVE_NOT_RUNNING;
const isUnserved = (e: unknown) => e instanceof ConnectError && e.code === Code.Unimplemented;

const Mono = ({ children }: { children: ReactNode }) => (
  <span className="font-mono text-xs">{children}</span>
);

function Loading({ what }: { what: string }) {
  return (
    <p className="text-muted" role="status">
      Loading {what}...
    </p>
  );
}

function Err({ title, message }: { title: string; message: string }) {
  return (
    <Notice tone="danger" title={title}>
      {message}
    </Notice>
  );
}

const show = (v: Json) => (typeof v === 'string' ? v : JSON.stringify(v));

function DocTable({ rows, caption }: { rows: Doc[]; caption: string }) {
  return (
    <Table
      caption={caption}
      rows={rows}
      rowKey={(d) => String(d._id)}
      empty={<Empty title="No documents">This table has no documents in this range.</Empty>}
      columns={[
        { key: 'id', header: '_id', cell: (d) => <Mono>{String(d._id)}</Mono> },
        {
          key: 'created',
          header: 'Created',
          cell: (d) =>
            typeof d._creationTime === 'number' ? new Date(d._creationTime).toLocaleString() : '',
        },
        {
          key: 'fields',
          header: 'Fields',
          cell: (d) => {
            const { _id, _creationTime, ...rest } = d;
            return <Mono>{show(rest)}</Mono>;
          },
        },
      ]}
    />
  );
}

// ---- the TiKV stack card -------------------------------------------------

type Stacks = LoamsDesktopApi['stacks'];
type StackState = Awaited<ReturnType<Stacks['state']>>;

export function StackCard({ stacks }: { stacks: Stacks }) {
  const [state, setState] = useState<StackState>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    let live = true;
    void stacks.state('tikv').then((s) => live && setState(s));
    const off = stacks.onState((id, s) => id === 'tikv' && live && setState(s));
    return () => {
      live = false;
      off();
    };
  }, [stacks]);
  const act = async (go: () => Promise<{ ok: boolean; message?: string }>) => {
    setError(undefined);
    const r = await go();
    if (!r.ok) setError(r.message ?? 'failed');
  };
  const phase = state?.phase ?? 'stopped';
  return (
    <Card title="TiKV stack">
      <div className="flex flex-col gap-3">
        <p className="m-0 text-muted">
          Live stores documents in TiKV. Starting the stack runs TiKV and PD in containers and
          restarts the local engine with Live on.
        </p>
        <div className="flex items-center gap-3">
          <Badge>{phase}</Badge>
          {phase === 'unavailable' && (
            <span className="text-muted">Install podman or docker to run the stack.</span>
          )}
          {state?.phase === 'error' && <span className="text-danger">{state.message}</span>}
          {phase === 'running' ? (
            <Button onClick={() => act(() => stacks.stop('tikv'))}>Stop</Button>
          ) : (
            <Button
              variant="primary"
              disabled={phase === 'starting' || phase === 'unavailable'}
              onClick={() => act(() => stacks.start('tikv'))}
            >
              {phase === 'starting' ? 'Starting...' : 'Start'}
            </Button>
          )}
        </div>
        {error && <Err title="Could not change the stack" message={error} />}
      </div>
    </Card>
  );
}

// ---- query controls ---------------------------------------------------------

interface Sel {
  table: string;
  index: string;
  eq: string;
}

function parseEq(text: string): { eq?: Json[]; error?: string } {
  if (!text.trim()) return {};
  try {
    const v = parseJson(text);
    return Array.isArray(v) ? { eq: v } : { error: 'The filter is a JSON array, like ["alice"].' };
  } catch {
    return { error: 'The filter is not valid JSON.' };
  }
}

function Controls({
  tables,
  sel,
  onChange,
  children,
}: {
  tables: TableInfo[];
  sel: Sel;
  onChange: (s: Sel) => void;
  children?: ReactNode;
}) {
  const table = tables.find((t) => t.name === sel.table);
  const eq = parseEq(sel.eq);
  return (
    <div className="flex flex-wrap items-end gap-4">
      <Field label="Table">
        {(p) => (
          <Select
            {...p}
            value={sel.table}
            onChange={(e) => onChange({ table: e.target.value, index: '', eq: '' })}
          >
            {tables.map((t) => (
              <option key={t.name} value={t.name}>
                {t.name}
              </option>
            ))}
          </Select>
        )}
      </Field>
      <Field label="Index">
        {(p) => (
          <Select
            {...p}
            value={sel.index}
            onChange={(e) => onChange({ ...sel, index: e.target.value })}
          >
            <option value="">by_creation_time</option>
            {(table?.indexes ?? [])
              .filter((i) => i.name !== 'by_creation_time')
              .map((i) => (
                <option key={i.name} value={i.name}>
                  {i.name} ({i.fields.join(', ')})
                </option>
              ))}
          </Select>
        )}
      </Field>
      <Field label="Index filter (JSON array)" error={eq.error}>
        {(p) => (
          <Input
            {...p}
            className="font-mono"
            placeholder='["alice"]'
            value={sel.eq}
            onChange={(e) => onChange({ ...sel, eq: e.target.value })}
          />
        )}
      </Field>
      {children}
    </div>
  );
}

function queryArgs(sel: Sel, limit?: number): QueryArgs | undefined {
  if (!sel.table) return undefined;
  const { eq, error } = parseEq(sel.eq);
  if (error) return undefined;
  return {
    table: sel.table,
    ...(sel.index ? { index: sel.index } : {}),
    ...(eq ? { eq } : {}),
    ...(limit ? { limit } : {}),
  };
}

// ---- documents ------------------------------------------------------------------

export function DocumentsTab({ api, tables, sel, onSel, onGone }: TabProps) {
  const [limit, setLimit] = useState(PAGE);
  const [rows, setRows] = useState<Doc[]>();
  const [error, setError] = useState<string>();
  const [tick, setTick] = useState(0);
  const args = queryArgs(sel, limit);
  const key = JSON.stringify(args);
  // biome-ignore lint/correctness/useExhaustiveDependencies: `key` is the args, `tick` the refresh
  useEffect(() => {
    if (!args) return;
    const ctl = new AbortController();
    setError(undefined);
    api
      .query(args, ctl.signal)
      .then(setRows)
      .catch((e) => {
        if (ctl.signal.aborted) return;
        if (isNotRunning(e)) onGone();
        else setError(errorText(e));
      });
    return () => ctl.abort();
  }, [api, key, tick, onGone]);
  return (
    <div className="flex flex-col gap-4">
      <Controls
        tables={tables}
        sel={sel}
        onChange={(s) => {
          setLimit(PAGE);
          setRows(undefined);
          onSel(s);
        }}
      >
        <Button onClick={() => setTick((t) => t + 1)}>Refresh</Button>
      </Controls>
      {error && <Err title="Could not read documents" message={error} />}
      {!error && !rows && args && <Loading what="documents" />}
      {rows && (
        <>
          <DocTable rows={rows} caption="Documents" />
          {rows.length >= limit && (
            <div>
              <Button onClick={() => setLimit((n) => n + PAGE)}>Load more</Button>
            </div>
          )}
        </>
      )}
    </div>
  );
}

// ---- live query ---------------------------------------------------------------

export function WatchTab({ api, tables, sel, onSel, onGone }: TabProps) {
  const [rows, setRows] = useState<Doc[]>([]);
  const [transitions, setTransitions] = useState(0);
  const [live, setLive] = useState(false);
  const [error, setError] = useState<string>();
  const [ended, setEnded] = useState(false);
  const ctl = useRef<AbortController | undefined>(undefined);

  const stop = useCallback(() => {
    ctl.current?.abort();
    ctl.current = undefined;
    setLive(false);
  }, []);
  // The stream is closed when the tab (or the page) goes away.
  useEffect(() => () => ctl.current?.abort(), []);

  const start = async () => {
    const args = queryArgs(sel);
    if (!args) return;
    stop();
    const c = new AbortController();
    ctl.current = c;
    setError(undefined);
    setEnded(false);
    setTransitions(0);
    setRows([]);
    setLive(true);
    try {
      for await (const ev of api.watch(args, c.signal)) {
        if (!ev.changed) continue;
        setTransitions((n) => n + 1);
        if (ev.error) setError(ev.error);
        else setRows(ev.rows ?? []);
      }
      // The server closed the stream (an engine restart, a dropped connection).
      if (!c.signal.aborted) setEnded(true);
    } catch (e) {
      if (!c.signal.aborted) {
        if (isNotRunning(e)) onGone();
        else setError(errorText(e));
      }
    } finally {
      if (ctl.current === c) {
        ctl.current = undefined;
        setLive(false);
      }
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <Controls
        tables={tables}
        sel={sel}
        onChange={(s) => {
          stop();
          setRows([]);
          setTransitions(0);
          setError(undefined);
          setEnded(false);
          onSel(s);
        }}
      >
        {live ? (
          <Button onClick={stop}>Unwatch</Button>
        ) : (
          <Button variant="primary" disabled={!queryArgs(sel)} onClick={start}>
            Watch
          </Button>
        )}
      </Controls>
      <div className="flex items-center gap-3">
        {live && (
          <span className="live-pill" role="status">
            <StatusTag status="progress">live</StatusTag>
          </span>
        )}
        <span className="text-muted">
          Transitions: <Mono>{transitions}</Mono>
        </span>
      </div>
      {error && <Err title="The watch reported an error" message={error} />}
      {ended && !live && (
        <Notice tone="info" title="The watch ended">
          The server closed the stream. Press Watch to start it again.
        </Notice>
      )}
      {(live || transitions > 0) && <DocTable rows={rows} caption="Live documents" />}
      {!live && transitions === 0 && !error && (
        <Empty title="Not watching">
          Pick a table and press Watch. Rows update here as documents change.
        </Empty>
      )}
    </div>
  );
}

// ---- mutations --------------------------------------------------------------------

interface Pending {
  title: string;
  fn: string;
  args: Json;
}

function parseObject(text: string): { value?: { [k: string]: Json }; error?: string } {
  try {
    const v = parseJson(text);
    if (v && typeof v === 'object' && !Array.isArray(v)) return { value: v };
    return { error: 'The fields are a JSON object.' };
  } catch {
    return { error: 'The fields are not valid JSON.' };
  }
}

export function MutateTab({ api, tables, sel, onGone }: TabProps) {
  const [insertTable, setInsertTable] = useState(sel.table || tables[0]?.name || '');
  const [insertFields, setInsertFields] = useState('{\n  "name": "example"\n}');
  const [patchId, setPatchId] = useState('');
  const [patchFields, setPatchFields] = useState('{}');
  const [deleteId, setDeleteId] = useState('');
  const [pending, setPending] = useState<Pending>();
  const [formError, setFormError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [done, setDone] = useState<string>();
  // One key per confirmation: a retry of the same dialog reuses it, so a
  // mutation that did commit is not applied twice.
  const key = useRef('');

  const ask = (p: Pending) => {
    key.current = crypto.randomUUID();
    setError(undefined);
    setDone(undefined);
    setPending(p);
  };
  const askWith = (title: string, fn: string, build: () => { [k: string]: Json } | string) => {
    const a = build();
    if (typeof a === 'string') return setFormError(a);
    setFormError(undefined);
    ask({ title, fn, args: a });
  };
  const confirm = async () => {
    if (!pending) return;
    setBusy(true);
    setError(undefined);
    try {
      const r = await api.mutate(pending.fn, pending.args, key.current);
      setDone(
        `${pending.title}: committed at ${r.commitTs}${r.result ? ` (${show(r.result)})` : ''}`,
      );
      setPending(undefined);
    } catch (e) {
      if (isNotRunning(e)) {
        setPending(undefined);
        onGone();
      } else setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-4">
      {done && (
        <Notice tone="info" title="Done">
          {done}
        </Notice>
      )}
      {formError && <Err title="Check the form" message={formError} />}
      <Card title="Insert a document">
        <div className="flex flex-col gap-3">
          <Field label="Table">
            {(p) => (
              <Select {...p} value={insertTable} onChange={(e) => setInsertTable(e.target.value)}>
                {tables.map((t) => (
                  <option key={t.name} value={t.name}>
                    {t.name}
                  </option>
                ))}
              </Select>
            )}
          </Field>
          <Field label="Fields (JSON)">
            {(p) => (
              <Textarea
                {...p}
                rows={5}
                className="font-mono"
                value={insertFields}
                onChange={(e) => setInsertFields(e.target.value)}
              />
            )}
          </Field>
          <div>
            <Button
              variant="primary"
              disabled={!insertTable}
              onClick={() =>
                askWith('Insert', '_system:insert', () => {
                  const f = parseObject(insertFields);
                  return f.value ? { table: insertTable, fields: f.value } : (f.error ?? '');
                })
              }
            >
              Insert
            </Button>
          </div>
        </div>
      </Card>
      <Card title="Patch a document">
        <div className="flex flex-col gap-3">
          <Field label="Document id">
            {(p) => (
              <Input
                {...p}
                className="font-mono"
                value={patchId}
                onChange={(e) => setPatchId(e.target.value)}
              />
            )}
          </Field>
          <Field label="Fields to set (JSON)" hint="Other fields stay as they are.">
            {(p) => (
              <Textarea
                {...p}
                rows={3}
                className="font-mono"
                value={patchFields}
                onChange={(e) => setPatchFields(e.target.value)}
              />
            )}
          </Field>
          <div>
            <Button
              disabled={!patchId.trim()}
              onClick={() =>
                askWith('Patch', '_system:patch', () => {
                  const f = parseObject(patchFields);
                  return f.value ? { id: patchId.trim(), fields: f.value } : (f.error ?? '');
                })
              }
            >
              Patch
            </Button>
          </div>
        </div>
      </Card>
      <Card title="Delete a document">
        <div className="flex items-end gap-4">
          <Field label="Document id">
            {(p) => (
              <Input
                {...p}
                className="font-mono"
                value={deleteId}
                onChange={(e) => setDeleteId(e.target.value)}
              />
            )}
          </Field>
          <Button
            variant="danger"
            disabled={!deleteId.trim()}
            onClick={() => askWith('Delete', '_system:delete', () => ({ id: deleteId.trim() }))}
          >
            Delete
          </Button>
        </div>
      </Card>
      <Dialog
        open={!!pending}
        onClose={() => !busy && setPending(undefined)}
        title={pending ? `Confirm: ${pending.title.toLowerCase()}` : 'Confirm'}
        footer={
          <>
            <Button variant="quiet" onClick={() => setPending(undefined)} disabled={busy}>
              Cancel
            </Button>
            <Button
              variant={pending?.fn === '_system:delete' ? 'danger' : 'primary'}
              disabled={busy}
              onClick={confirm}
            >
              Confirm
            </Button>
          </>
        }
      >
        {pending && (
          <div className="flex flex-col gap-3">
            <p className="m-0">
              Run <Mono>{pending.fn}</Mono> with these arguments. It commits at once.
            </p>
            <pre className="font-mono text-xs m-0 overflow-auto">
              {JSON.stringify(pending.args, null, 2)}
            </pre>
            <p className="m-0 text-muted">
              Idempotency key <Mono>{key.current}</Mono>
            </p>
            {error && <Err title="The mutation failed" message={error} />}
          </div>
        )}
      </Dialog>
    </div>
  );
}

// ---- the engine's Live notice ---------------------------------------------------------

/** Why Live runs on local data although TiKV was chosen (ruling T23-8), from the engine state. */
function useLiveNotice(
  engine?: Partial<Pick<LoamsDesktopApi['engine'], 'state' | 'onState'>>,
): string | undefined {
  const [notice, setNotice] = useState<string>();
  useEffect(() => {
    if (!engine?.state || !engine.onState) return;
    let live = true;
    const take = (s: EngineState) => {
      if (live) setNotice(s.phase === 'ready' ? s.liveNotice : undefined);
    };
    engine
      .state()
      .then(take)
      .catch(() => undefined);
    const off = engine.onState(take);
    return () => {
      live = false;
      off();
    };
  }, [engine]);
  return notice;
}

// ---- the page -----------------------------------------------------------------------

export interface TabProps {
  api: LiveApi;
  tables: TableInfo[];
  sel: Sel;
  onSel: (s: Sel) => void;
  /** Live went away under the view: go back to the needs-TiKV state. */
  onGone: () => void;
}

const TABS = [
  { id: 'documents', label: 'Documents' },
  { id: 'watch', label: 'Live query' },
  { id: 'mutate', label: 'Mutate' },
] as const;
type TabId = (typeof TABS)[number]['id'];

type Loaded =
  | { state: 'loading' }
  | { state: 'needs-tikv' }
  | { state: 'unserved' }
  | { state: 'error'; message: string }
  | { state: 'ready'; tables: TableInfo[] };

export function LivePage({
  api,
  desktop,
  retryMs = 2000,
}: {
  api: LiveApi;
  /** `engine`, when given, carries the Live notice (ruling T23-8). */
  desktop: Pick<LoamsDesktopApi, 'stacks'> & {
    engine?: Partial<Pick<LoamsDesktopApi['engine'], 'state' | 'onState'>>;
  };
  retryMs?: number;
}) {
  const [tab, setTab] = useState<TabId>('documents');
  const [sel, setSel] = useState<Sel>({ table: '', index: '', eq: '' });
  const [loaded, setLoaded] = useState<Loaded>({ state: 'loading' });
  const [tick, setTick] = useState(0);
  const gone = useCallback(() => setLoaded({ state: 'needs-tikv' }), []);
  const notice = useLiveNotice(desktop.engine);

  // biome-ignore lint/correctness/useExhaustiveDependencies: `tick` is the reload trigger
  useEffect(() => {
    const ctl = new AbortController();
    let retry: ReturnType<typeof setTimeout> | undefined;
    api
      .tables(ctl.signal)
      .then((tables) => {
        setLoaded({ state: 'ready', tables });
        setSel((s) =>
          tables.some((t) => t.name === s.table)
            ? s
            : { table: tables[0]?.name ?? '', index: '', eq: '' },
        );
      })
      .catch((e) => {
        if (ctl.signal.aborted) return;
        if (isNotRunning(e)) setLoaded({ state: 'needs-tikv' });
        else if (isStarting(e)) {
          setLoaded({ state: 'loading' });
          retry = setTimeout(() => setTick((n) => n + 1), retryMs);
        } else if (isUnserved(e)) setLoaded({ state: 'unserved' });
        else setLoaded({ state: 'error', message: errorText(e) });
      });
    return () => {
      ctl.abort();
      clearTimeout(retry);
    };
  }, [api, tick, retryMs]);

  // While Live is off, look again: the stack takes a while to start and the
  // engine restarts with Live once it is up.
  useEffect(() => {
    if (loaded.state !== 'needs-tikv') return;
    const t = setInterval(() => setTick((n) => n + 1), retryMs);
    return () => clearInterval(t);
  }, [loaded.state, retryMs]);

  return (
    <div className="flex flex-col gap-6 p-6 max-w-6xl">
      <header className="flex items-start justify-between gap-4">
        <div>
          <h1 className="m-0">Live</h1>
          <p className="text-muted m-0">
            Tables and documents that update in place: query, watch and mutate this engine's Loams
            Live.
          </p>
        </div>
        <span title={DEPLOY_TIP}>
          <Button disabled aria-label="Deploy" title={DEPLOY_TIP}>
            Deploy
          </Button>
        </span>
      </header>
      {notice && (
        <Notice tone="warn" title="Live on TiKV">
          {notice}
        </Notice>
      )}
      {loaded.state === 'loading' && <Loading what="tables" />}
      {loaded.state === 'needs-tikv' && (
        <div className="flex flex-col gap-4">
          <Notice tone="info" title="Live needs the TiKV stack">
            This engine is running without Live. Start the TiKV stack and Loams restarts the engine
            with Live on.
          </Notice>
          <StackCard stacks={desktop.stacks} />
        </div>
      )}
      {loaded.state === 'unserved' && (
        <Empty title="This server does not serve Live.">
          Switch to a server that does, such as This computer.
        </Empty>
      )}
      {loaded.state === 'error' && (
        <div className="flex flex-col gap-3">
          <Err title="Could not load tables" message={loaded.message} />
          <div>
            <Button onClick={() => setTick((n) => n + 1)}>Retry</Button>
          </div>
        </div>
      )}
      {loaded.state === 'ready' && (
        <>
          <Card title="Tables" flush>
            <Table
              caption="Tables"
              rows={loaded.tables}
              rowKey={(t) => t.name}
              onRowClick={(t) => setSel({ table: t.name, index: '', eq: '' })}
              columns={[
                {
                  key: 'name',
                  header: 'Name',
                  cell: (t) => (
                    <button
                      type="button"
                      className={
                        t.name === sel.table
                          ? 'bg-transparent border-0 p-0 text-left cursor-pointer text-accent font-medium'
                          : 'bg-transparent border-0 p-0 text-left cursor-pointer text-ink'
                      }
                      onClick={() => setSel({ table: t.name, index: '', eq: '' })}
                    >
                      <Mono>{t.name}</Mono>
                    </button>
                  ),
                },
                { key: 'id', header: 'Id', cell: (t) => <Mono>{t.id}</Mono> },
                {
                  key: 'indexes',
                  header: 'Indexes',
                  cell: (t) => t.indexes.map((i) => `${i.name}(${i.fields.join(',')})`).join('  '),
                },
              ]}
            />
          </Card>
          <div role="tablist" aria-label="Live views" className="flex gap-4 border-b border-rule">
            {TABS.map((t) => (
              <button
                key={t.id}
                type="button"
                role="tab"
                aria-selected={t.id === tab}
                className={
                  t.id === tab
                    ? 'bg-transparent border-0 border-b-2 border-accent text-ink px-1 py-2 cursor-pointer font-medium'
                    : 'bg-transparent border-0 border-b-2 border-transparent text-muted px-1 py-2 cursor-pointer'
                }
                onClick={() => setTab(t.id)}
              >
                {t.label}
              </button>
            ))}
          </div>
          {tab === 'documents' && (
            <DocumentsTab api={api} tables={loaded.tables} sel={sel} onSel={setSel} onGone={gone} />
          )}
          {tab === 'watch' && (
            <WatchTab api={api} tables={loaded.tables} sel={sel} onSel={setSel} onGone={gone} />
          )}
          {tab === 'mutate' && (
            <MutateTab api={api} tables={loaded.tables} sel={sel} onSel={setSel} onGone={gone} />
          )}
        </>
      )}
    </div>
  );
}
