// The durable execution page: promises, schedules, tasks and runs of the
// local engine's Resonate server, through the envelope client.

import { type Loaded, useLoad as useSharedLoad } from '@loams/desktop-ui';
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
import { type FormEvent, type ReactNode, useCallback, useEffect, useRef, useState } from 'react';
import { describeCron } from './cron.js';
import {
  type Decoded,
  type DurableApi,
  decodeValue,
  EnvelopeError,
  encodeParam,
  PROMISE_STATES,
  type PromiseRecord,
  type PromiseState,
  parseTag,
  type ScheduleRecord,
  TASK_STATES,
  type TaskRecord,
  type TaskState,
  type Value,
} from './envelope.js';
import { buildRuns, countNodes, type RunNode } from './runs.js';

const PAGE = 50;

export function errorText(e: unknown): string {
  if (e instanceof EnvelopeError) return e.message;
  return e instanceof Error ? e.message : String(e);
}

function useLoad<T>(load: () => Promise<T>, deps: unknown[]): [Loaded<T>, () => void] {
  return useSharedLoad(load, deps, { errorText });
}

function ErrorNotice({ title, message }: { title: string; message: string }) {
  return (
    <Notice tone="danger" title={title}>
      {message}
    </Notice>
  );
}

const Loading = ({ what }: { what: string }) => (
  <p className="text-muted text-sm py-6">Loading {what}…</p>
);

const Mono = ({ children }: { children: ReactNode }) => (
  <span className="font-mono text-xs">{children}</span>
);

const when = (ms: number | undefined) => (ms ? new Date(ms).toLocaleString() : '—');

function promiseTone(s: PromiseState) {
  if (s === 'resolved') return 'done';
  if (s === 'pending') return 'progress';
  if (s === 'rejected_canceled') return 'neutral';
  return 'failed';
}
function taskTone(s: TaskState) {
  if (s === 'fulfilled') return 'done';
  if (s === 'halted') return 'failed';
  if (s === 'suspended') return 'planned';
  return 'progress';
}

export const PromiseState_ = ({ state }: { state: PromiseState }) => (
  <StatusTag status={promiseTone(state)}>{state}</StatusTag>
);

// ---- payloads ----

export function ValueView({ value, label }: { value: Value | undefined; label: string }) {
  const d: Decoded = decodeValue(value);
  const [asText, setAsText] = useState(false);
  let body: ReactNode;
  if (d.kind === 'empty') body = <span className="text-muted text-sm">empty</span>;
  else if (d.kind === 'json')
    body = (
      <pre className="font-mono text-xs overflow-auto max-h-60 m-0 whitespace-pre-wrap">
        {d.text}
      </pre>
    );
  else
    body = (
      <>
        <pre className="font-mono text-xs overflow-auto max-h-60 m-0 break-all whitespace-pre-wrap">
          {asText && d.text !== undefined ? d.text : d.raw}
        </pre>
        {d.text !== undefined && (
          <Button size="sm" variant="quiet" onClick={() => setAsText(!asText)}>
            {asText ? 'Show base64' : 'Show as text'}
          </Button>
        )}
      </>
    );
  return (
    <div className="flex flex-col gap-1" data-value={label}>
      <div className="text-muted text-xs">
        {label}
        {d.kind === 'base64' && !asText ? ' (base64)' : ''}
      </div>
      {body}
    </div>
  );
}

function TagList({ tags }: { tags: Record<string, string> }) {
  const entries = Object.entries(tags);
  if (entries.length === 0) return <span className="text-muted text-sm">no tags</span>;
  return (
    <div className="flex flex-wrap gap-1">
      {entries.map(([k, v]) => (
        <Badge key={k}>{`${k}=${v}`}</Badge>
      ))}
    </div>
  );
}

function TagInput({
  tags,
  onChange,
}: {
  tags: [string, string][];
  onChange: (t: [string, string][]) => void;
}) {
  const [draft, setDraft] = useState('');
  const [bad, setBad] = useState(false);
  const add = () => {
    const t = parseTag(draft);
    if (!t) return setBad(true);
    setBad(false);
    onChange([...tags.filter(([k]) => k !== t[0]), t]);
    setDraft('');
  };
  return (
    <div className="flex flex-col gap-2">
      <div className="flex gap-2 items-start">
        <Input
          aria-label="Tag (key=value)"
          placeholder="key=value"
          value={draft}
          aria-invalid={bad || undefined}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              add();
            }
          }}
        />
        <Button size="sm" onClick={add}>
          Add tag
        </Button>
      </div>
      {bad && <span className="text-danger text-xs">A tag looks like key=value.</span>}
      {tags.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {tags.map(([k, v]) => (
            <Button
              key={k}
              size="sm"
              variant="quiet"
              aria-label={`Remove tag ${k}`}
              onClick={() => onChange(tags.filter(([x]) => x !== k))}
            >
              {`${k}=${v} ×`}
            </Button>
          ))}
        </div>
      )}
    </div>
  );
}

// ---- promises ----

export function PromiseDetail({
  api,
  promise,
  onClose,
  onChanged,
}: {
  api: DurableApi;
  promise: PromiseRecord;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [current, setCurrent] = useState(promise);
  const cancel = async () => {
    setBusy(true);
    setError(undefined);
    try {
      setCurrent(await api.cancelPromise(current.id));
      setConfirming(false);
      onChanged();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open
      onClose={onClose}
      title={<span className="font-mono text-sm break-all">{current.id}</span>}
      footer={
        confirming ? (
          <>
            <span className="text-sm mr-auto">
              Cancel this promise? It will be rejected as canceled.
            </span>
            <Button variant="quiet" onClick={() => setConfirming(false)}>
              Keep it
            </Button>
            <Button variant="danger" disabled={busy} onClick={cancel}>
              Confirm cancel
            </Button>
          </>
        ) : (
          <>
            <Button variant="quiet" onClick={onClose}>
              Close
            </Button>
            {current.state === 'pending' && (
              <Button variant="danger" onClick={() => setConfirming(true)}>
                Cancel promise
              </Button>
            )}
          </>
        )
      }
    >
      <div className="flex flex-col gap-4">
        <dl className="grid grid-cols-2 gap-2 m-0 text-sm">
          <div>
            <dt className="text-muted text-xs">State</dt>
            <dd className="m-0">
              <PromiseState_ state={current.state} />
            </dd>
          </div>
          <div>
            <dt className="text-muted text-xs">Created</dt>
            <dd className="m-0">{when(current.createdAt)}</dd>
          </div>
          <div>
            <dt className="text-muted text-xs">Times out</dt>
            <dd className="m-0">{when(current.timeoutAt)}</dd>
          </div>
          <div>
            <dt className="text-muted text-xs">Settled</dt>
            <dd className="m-0">{when(current.settledAt)}</dd>
          </div>
        </dl>
        <TagList tags={current.tags} />
        <ValueView label="param" value={current.param} />
        <ValueView label="value" value={current.value} />
        {error && <ErrorNotice title="Not canceled" message={error} />}
      </div>
    </Dialog>
  );
}

function NewPromise({
  api,
  onClose,
  onCreated,
}: {
  api: DurableApi;
  onClose: () => void;
  onCreated: () => void;
}) {
  const [id, setId] = useState('');
  const [timeout, setTimeoutSecs] = useState('3600');
  const [param, setParam] = useState('');
  const [tags, setTags] = useState<[string, string][]>([]);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setError(undefined);
    const secs = Number(timeout);
    if (!Number.isFinite(secs) || secs <= 0)
      return setError('The timeout is a number of seconds above zero.');
    let value: Value;
    try {
      value = encodeParam(param);
    } catch {
      return setError('The param is not valid JSON.');
    }
    setBusy(true);
    try {
      await api.createPromise({
        id: id.trim(),
        timeoutAt: Date.now() + Math.round(secs * 1000),
        param: value,
        tags: Object.fromEntries(tags),
      });
      onCreated();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open
      onClose={onClose}
      title="New promise"
      footer={
        <>
          <Button variant="quiet" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            form="durable-new-promise"
            disabled={busy || !id.trim()}
          >
            Create promise
          </Button>
        </>
      }
    >
      <form id="durable-new-promise" className="flex flex-col gap-3" onSubmit={submit}>
        <Field label="Id">
          {(p) => <Input {...p} value={id} onChange={(e) => setId(e.target.value)} />}
        </Field>
        <Field
          label="Timeout (seconds)"
          hint="The promise is rejected as timed out after this long."
        >
          {(p) => (
            <Input
              {...p}
              inputMode="numeric"
              value={timeout}
              onChange={(e) => setTimeoutSecs(e.target.value)}
            />
          )}
        </Field>
        <Field label="Param (JSON)" hint="Optional.">
          {(p) => (
            <Textarea
              {...p}
              rows={4}
              className="font-mono"
              value={param}
              onChange={(e) => setParam(e.target.value)}
            />
          )}
        </Field>
        <div>
          <div className="text-sm mb-1">Tags</div>
          <TagInput tags={tags} onChange={setTags} />
        </div>
        {error && <ErrorNotice title="Could not create the promise" message={error} />}
      </form>
    </Dialog>
  );
}

export function PromisesTab({ api }: { api: DurableApi }) {
  const [state, setState] = useState<PromiseState | ''>('');
  const [tags, setTags] = useState<[string, string][]>([]);
  const [rows, setRows] = useState<PromiseRecord[]>([]);
  const [cursor, setCursor] = useState<string>();
  const [status, setStatus] = useState<'loading' | 'ready' | 'error'>('loading');
  const [error, setError] = useState<string>();
  const [selected, setSelected] = useState<PromiseRecord>();
  const [creating, setCreating] = useState(false);
  const [tick, setTick] = useState(0);
  const query = useRef({ state, tags });
  query.current = { state, tags };

  const fetchPage = useCallback(
    async (after?: string) => {
      const q = query.current;
      const page = await api.searchPromises({
        ...(q.state ? { state: q.state } : {}),
        ...(q.tags.length ? { tags: Object.fromEntries(q.tags) } : {}),
        limit: PAGE,
        ...(after ? { cursor: after } : {}),
      });
      return page;
    },
    [api],
  );

  // biome-ignore lint/correctness/useExhaustiveDependencies: filters and reload trigger a fresh first page
  useEffect(() => {
    let live = true;
    setStatus('loading');
    fetchPage()
      .then((p) => {
        if (!live) return;
        setRows(p.items);
        setCursor(p.cursor);
        setStatus('ready');
      })
      .catch((e) => {
        if (!live) return;
        setError(errorText(e));
        setStatus('error');
      });
    return () => {
      live = false;
    };
  }, [fetchPage, state, tags, tick]);

  const more = async () => {
    try {
      const p = await fetchPage(cursor);
      setRows((r) => [...r, ...p.items]);
      setCursor(p.cursor);
    } catch (e) {
      setError(errorText(e));
      setStatus('error');
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-end gap-4">
        <Field label="State">
          {(p) => (
            <Select
              {...p}
              value={state}
              onChange={(e) => setState(e.target.value as PromiseState | '')}
            >
              <option value="">All states</option>
              {PROMISE_STATES.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </Select>
          )}
        </Field>
        <div className="flex-1 min-w-64">
          <TagInput tags={tags} onChange={setTags} />
        </div>
        <Button variant="primary" onClick={() => setCreating(true)}>
          New promise
        </Button>
      </div>
      {status === 'loading' && <Loading what="promises" />}
      {status === 'error' && <ErrorNotice title="Could not load promises" message={error ?? ''} />}
      {status === 'ready' && (
        <>
          <Table
            caption="Promises"
            rows={rows}
            rowKey={(p) => p.id}
            onRowClick={setSelected}
            empty={
              <Empty title="No promises">
                {state || tags.length
                  ? 'Nothing matches these filters.'
                  : 'Create one, or run a Resonate worker against this engine.'}
              </Empty>
            }
            columns={[
              {
                key: 'id',
                header: 'Id',
                cell: (p) => (
                  <button
                    type="button"
                    className="bg-transparent border-0 p-0 text-left cursor-pointer text-ink"
                    onClick={() => setSelected(p)}
                  >
                    <Mono>{p.id}</Mono>
                  </button>
                ),
              },
              { key: 'state', header: 'State', cell: (p) => <PromiseState_ state={p.state} /> },
              { key: 'created', header: 'Created', cell: (p) => when(p.createdAt) },
              { key: 'timeout', header: 'Times out', cell: (p) => when(p.timeoutAt) },
            ]}
          />
          {cursor && (
            <div>
              <Button onClick={more}>Load more</Button>
            </div>
          )}
        </>
      )}
      {selected && (
        <PromiseDetail
          key={selected.id}
          api={api}
          promise={selected}
          onClose={() => setSelected(undefined)}
          onChanged={() => setTick((t) => t + 1)}
        />
      )}
      {creating && (
        <NewPromise
          api={api}
          onClose={() => setCreating(false)}
          onCreated={() => {
            setCreating(false);
            setTick((t) => t + 1);
          }}
        />
      )}
    </div>
  );
}

// ---- schedules ----

function NewSchedule({
  api,
  onClose,
  onCreated,
}: {
  api: DurableApi;
  onClose: () => void;
  onCreated: () => void;
}) {
  const [id, setId] = useState('');
  const [cron, setCron] = useState('0 * * * *');
  const [promiseId, setPromiseId] = useState('{{.id}}-{{.timestamp}}');
  const [timeout, setTimeoutSecs] = useState('3600');
  const [target, setTarget] = useState('poll://any@default');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const preview = describeCron(cron);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setError(undefined);
    const secs = Number(timeout);
    if (!Number.isFinite(secs) || secs <= 0)
      return setError('The timeout is a number of seconds above zero.');
    setBusy(true);
    try {
      await api.createSchedule({
        id: id.trim(),
        cron: cron.trim(),
        promiseId: promiseId.trim(),
        promiseTimeout: Math.round(secs * 1000),
        promiseParam: { headers: {}, data: '' },
        promiseTags: { 'resonate:target': target.trim() },
      });
      onCreated();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open
      onClose={onClose}
      title="New schedule"
      footer={
        <>
          <Button variant="quiet" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            form="durable-new-schedule"
            disabled={busy || !id.trim() || !cron.trim() || !target.trim()}
          >
            Create schedule
          </Button>
        </>
      }
    >
      <form id="durable-new-schedule" className="flex flex-col gap-3" onSubmit={submit}>
        <Field label="Id">
          {(p) => <Input {...p} value={id} onChange={(e) => setId(e.target.value)} />}
        </Field>
        <Field
          label="Cron"
          hint={
            preview
              ? `Runs: ${preview}`
              : 'Five fields: minute hour day-of-month month day-of-week.'
          }
        >
          {(p) => (
            <Input
              {...p}
              className="font-mono"
              value={cron}
              onChange={(e) => setCron(e.target.value)}
            />
          )}
        </Field>
        <Field
          label="Promise id template"
          hint="{{.id}} is the schedule id, {{.timestamp}} the run time."
        >
          {(p) => (
            <Input
              {...p}
              className="font-mono"
              value={promiseId}
              onChange={(e) => setPromiseId(e.target.value)}
            />
          )}
        </Field>
        <Field
          label="Target"
          hint="Where the created promises are routed (the resonate:target tag). The server requires it."
        >
          {(p) => (
            <Input
              {...p}
              className="font-mono"
              value={target}
              onChange={(e) => setTarget(e.target.value)}
            />
          )}
        </Field>
        <Field label="Promise timeout (seconds)">
          {(p) => (
            <Input
              {...p}
              inputMode="numeric"
              value={timeout}
              onChange={(e) => setTimeoutSecs(e.target.value)}
            />
          )}
        </Field>
        {error && <ErrorNotice title="Could not create the schedule" message={error} />}
      </form>
    </Dialog>
  );
}

export function SchedulesTab({ api }: { api: DurableApi }) {
  const [rows, reload] = useLoad(() => api.searchSchedules({ limit: 200 }), [api]);
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState<ScheduleRecord>();
  const [error, setError] = useState<string>();
  const remove = async () => {
    if (!deleting) return;
    try {
      await api.deleteSchedule(deleting.id);
      setDeleting(undefined);
      reload();
    } catch (e) {
      setError(errorText(e));
    }
  };
  return (
    <div className="flex flex-col gap-4">
      <div>
        <Button variant="primary" onClick={() => setCreating(true)}>
          New schedule
        </Button>
      </div>
      {error && <ErrorNotice title="Not deleted" message={error} />}
      {rows.state === 'loading' && <Loading what="schedules" />}
      {rows.state === 'error' && (
        <ErrorNotice title="Could not load schedules" message={rows.message} />
      )}
      {rows.state === 'ready' && (
        <Table
          caption="Schedules"
          rows={rows.data.items}
          rowKey={(s) => s.id}
          empty={
            <Empty title="No schedules">A schedule creates a promise on a cron timetable.</Empty>
          }
          columns={[
            { key: 'id', header: 'Id', cell: (s) => <Mono>{s.id}</Mono> },
            {
              key: 'cron',
              header: 'Cron',
              cell: (s) => (
                <span className="flex flex-col">
                  <Mono>{s.cron}</Mono>
                  <span className="text-muted text-xs">{describeCron(s.cron) ?? ''}</span>
                </span>
              ),
            },
            { key: 'tmpl', header: 'Promise id', cell: (s) => <Mono>{s.promiseId}</Mono> },
            { key: 'next', header: 'Next run', cell: (s) => when(s.nextRunAt) },
            { key: 'last', header: 'Last run', cell: (s) => when(s.lastRunAt) },
            {
              key: 'x',
              header: '',
              cell: (s) => (
                <Button
                  size="sm"
                  variant="quiet"
                  aria-label={`Delete schedule ${s.id}`}
                  onClick={() => setDeleting(s)}
                >
                  Delete
                </Button>
              ),
            },
          ]}
        />
      )}
      {creating && (
        <NewSchedule
          api={api}
          onClose={() => setCreating(false)}
          onCreated={() => {
            setCreating(false);
            reload();
          }}
        />
      )}
      {deleting && (
        <Dialog
          open
          onClose={() => setDeleting(undefined)}
          title="Delete schedule"
          footer={
            <>
              <Button variant="quiet" onClick={() => setDeleting(undefined)}>
                Keep it
              </Button>
              <Button variant="danger" onClick={remove}>
                Delete schedule
              </Button>
            </>
          }
        >
          <p>
            Delete <Mono>{deleting.id}</Mono>? It stops creating promises; the ones it already
            created stay.
          </p>
        </Dialog>
      )}
    </div>
  );
}

// ---- tasks ----

export function TasksTab({ api }: { api: DurableApi }) {
  const [state, setState] = useState<TaskState | ''>('');
  const [rows] = useLoad(
    () => api.searchTasks({ ...(state ? { state } : {}), limit: 200 }),
    [api, state],
  );
  const [selected, setSelected] = useState<TaskRecord>();
  return (
    <div className="flex flex-col gap-4">
      <div className="max-w-xs">
        <Field label="State">
          {(p) => (
            <Select
              {...p}
              value={state}
              onChange={(e) => setState(e.target.value as TaskState | '')}
            >
              <option value="">All states</option>
              {TASK_STATES.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </Select>
          )}
        </Field>
      </div>
      {rows.state === 'loading' && <Loading what="tasks" />}
      {rows.state === 'error' && (
        <ErrorNotice title="Could not load tasks" message={rows.message} />
      )}
      {rows.state === 'ready' && (
        <Table
          caption="Tasks"
          rows={rows.data.items}
          rowKey={(t) => t.id}
          onRowClick={setSelected}
          empty={<Empty title="No tasks">Tasks appear when a promise is routed to a worker.</Empty>}
          columns={[
            {
              key: 'id',
              header: 'Id',
              cell: (t) => (
                <button
                  type="button"
                  className="bg-transparent border-0 p-0 text-left cursor-pointer text-ink"
                  onClick={() => setSelected(t)}
                >
                  <Mono>{t.id}</Mono>
                </button>
              ),
            },
            {
              key: 'state',
              header: 'State',
              cell: (t) => <StatusTag status={taskTone(t.state)}>{t.state}</StatusTag>,
            },
            { key: 'version', header: 'Version', numeric: true, cell: (t) => t.version },
            { key: 'pid', header: 'Worker', cell: (t) => (t.pid ? <Mono>{t.pid}</Mono> : '—') },
          ]}
        />
      )}
      {selected && (
        <Dialog
          open
          onClose={() => setSelected(undefined)}
          title={<span className="font-mono text-sm break-all">{selected.id}</span>}
          footer={
            <Button variant="quiet" onClick={() => setSelected(undefined)}>
              Close
            </Button>
          }
        >
          <dl className="grid grid-cols-2 gap-2 m-0 text-sm">
            <div>
              <dt className="text-muted text-xs">State</dt>
              <dd className="m-0">
                <StatusTag status={taskTone(selected.state)}>{selected.state}</StatusTag>
              </dd>
            </div>
            <div>
              <dt className="text-muted text-xs">Version</dt>
              <dd className="m-0">{selected.version}</dd>
            </div>
            <div>
              <dt className="text-muted text-xs">Worker</dt>
              <dd className="m-0">{selected.pid ?? '—'}</dd>
            </div>
            <div>
              <dt className="text-muted text-xs">TTL</dt>
              <dd className="m-0">{selected.ttl === undefined ? '—' : `${selected.ttl} ms`}</dd>
            </div>
            <div className="col-span-2">
              <dt className="text-muted text-xs">Resumes</dt>
              <dd className="m-0 font-mono text-xs break-all">
                {JSON.stringify(selected.resumes)}
              </dd>
            </div>
          </dl>
        </Dialog>
      )}
    </div>
  );
}

// ---- runs ----

function RunRow({ node, depth }: { node: RunNode; depth: number }) {
  const [open, setOpen] = useState(depth === 0);
  const p = node.promise;
  return (
    <li className="list-none">
      <div className="flex items-center gap-2 py-1" style={{ paddingLeft: `${depth * 1.25}rem` }}>
        {node.children.length > 0 ? (
          <Button
            size="sm"
            variant="quiet"
            aria-expanded={open}
            aria-label={`${open ? 'Collapse' : 'Expand'} ${p.id}`}
            onClick={() => setOpen(!open)}
          >
            {open ? '▾' : '▸'}
          </Button>
        ) : (
          <span className="inline-block w-8" />
        )}
        <Mono>{p.id}</Mono>
        <PromiseState_ state={p.state} />
        <span className="text-muted text-xs">{when(p.createdAt)}</span>
      </div>
      {open && node.children.length > 0 && (
        <ul className="m-0 p-0">
          {node.children.map((c) => (
            <RunRow key={c.promise.id} node={c} depth={depth + 1} />
          ))}
        </ul>
      )}
    </li>
  );
}

export function RunsTab({ api }: { api: DurableApi }) {
  const [runs, reload] = useLoad(
    async () => buildRuns((await api.searchPromises({ limit: 200 })).items),
    [api],
  );
  return (
    <div className="flex flex-col gap-4">
      <div>
        <Button onClick={reload}>Refresh</Button>
      </div>
      {runs.state === 'loading' && <Loading what="runs" />}
      {runs.state === 'error' && <ErrorNotice title="Could not load runs" message={runs.message} />}
      {runs.state === 'ready' &&
        (runs.data.length === 0 ? (
          <Empty title="No runs">
            A run is a workflow promise and the promises it spawns, linked by the resonate:origin
            and resonate:parent tags.
          </Empty>
        ) : (
          <Card title={`${runs.data.length} run${runs.data.length === 1 ? '' : 's'}`}>
            <ul className="m-0 p-0">
              {runs.data.map((r) => (
                <RunRow key={r.promise.id} node={r} depth={0} />
              ))}
            </ul>
            <p className="text-muted text-xs m-0 mt-2">
              {runs.data.reduce((n, r) => n + countNodes(r), 0)} promises in the latest 200.
            </p>
          </Card>
        ))}
    </div>
  );
}

// ---- page ----

const TABS = [
  { id: 'promises', label: 'Promises' },
  { id: 'schedules', label: 'Schedules' },
  { id: 'tasks', label: 'Tasks' },
  { id: 'runs', label: 'Runs' },
] as const;
type TabId = (typeof TABS)[number]['id'];

export function DurablePage({ api }: { api: DurableApi }) {
  const [tab, setTab] = useState<TabId>('promises');
  return (
    <div className="flex flex-col gap-6 p-6 max-w-6xl">
      <header>
        <h1 className="m-0">Durable execution</h1>
        <p className="text-muted m-0">
          Promises, schedules and tasks of this engine's Resonate server, and the runs they form.
        </p>
      </header>
      <div role="tablist" aria-label="Durable views" className="flex gap-4 border-b border-rule">
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
      {tab === 'promises' && <PromisesTab api={api} />}
      {tab === 'schedules' && <SchedulesTab api={api} />}
      {tab === 'tasks' && <TasksTab api={api} />}
      {tab === 'runs' && <RunsTab api={api} />}
    </div>
  );
}
