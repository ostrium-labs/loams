// The Live Deployments page: deploy history with the active deployment and
// rollback, the functions of a deployment with their argument validators,
// worker health (where the API has it), and a tail of function logs.

import {
  Badge,
  Button,
  Card,
  Checkbox,
  Dialog,
  Empty,
  Field,
  Notice,
  SampleDataNotice,
  Select,
  SkeletonList,
  type Status,
  StatusTag,
  Table,
} from '@loams/ui';
import { type ReactNode, useCallback, useEffect, useState } from 'react';
import {
  type Deployment,
  type DeploymentDetail,
  type DeploymentState,
  isSkipped,
  type LiveAdminApi,
  type LogLine,
  validatorText,
  type WorkerHealth,
} from './admin.js';
import { errorText } from './pages.js';

export const PREVIEW_NOTICE = 'Actions here change nothing.';
/** The most log lines kept on screen; older lines drop off the top. */
export const LOG_CAP = 500;

const Mono = ({ children }: { children: ReactNode }) => (
  <span className={/* mono-ok: id */ 'font-mono text-small'}>{children}</span>
);
const shortId = (id: string) => id.slice(-8);
const when = (ms: number) => new Date(ms).toLocaleString();

const STATE_TAG: Record<DeploymentState, Status> = {
  pending: 'planned',
  validating: 'progress',
  backfilling: 'progress',
  active: 'done',
  superseded: 'neutral',
  failed: 'failed',
  degraded: 'failed',
};
const STATE_LABEL: Record<DeploymentState, string> = {
  pending: 'Pending',
  validating: 'Validating',
  backfilling: 'Backfilling',
  active: 'Active',
  superseded: 'Superseded',
  failed: 'Failed',
  degraded: 'Degraded',
};

export function DeploymentStateTag({ state }: { state: DeploymentState }) {
  return <StatusTag status={STATE_TAG[state]}>{STATE_LABEL[state]}</StatusTag>;
}

const isLive = (d: Deployment) => d.state === 'active' || d.state === 'degraded';
const canActivate = (d: Deployment) => d.state === 'superseded';

// ---- worker health ------------------------------------------------------------

export function WorkerHealthCard({ workers }: { workers: WorkerHealth[] }) {
  const degraded = workers.filter((w) => w.degraded).length;
  return (
    <Card title="Worker health" flush>
      {degraded > 0 && (
        <div className="p-3">
          <Notice tone="warn" title="Degraded">
            {degraded === 1 ? 'One worker is' : `${degraded} workers are`} degraded: five or more
            crashes in a minute. Calls on that node fail until the deployment is fixed or rolled
            back.
          </Notice>
        </div>
      )}
      <Table
        caption="Worker health"
        rows={workers}
        rowKey={(w) => `${w.node}/${w.deploymentId}`}
        empty={<Empty title="No workers">No function worker is running.</Empty>}
        columns={[
          { key: 'node', header: 'Node', cell: (w) => <Mono>{w.node}</Mono> },
          {
            key: 'deployment',
            header: 'Deployment',
            cell: (w) => <Mono>{shortId(w.deploymentId)}</Mono>,
          },
          {
            key: 'health',
            header: 'Health',
            cell: (w) =>
              w.degraded ? (
                <StatusTag status="failed">Degraded</StatusTag>
              ) : (
                <StatusTag status="done">Healthy</StatusTag>
              ),
          },
          {
            key: 'crashes',
            header: 'Crashes (1 min)',
            numeric: true,
            cell: (w) => w.crashesLastMinute,
          },
          { key: 'restarts', header: 'Restarts', numeric: true, cell: (w) => w.restarts },
          {
            key: 'last',
            header: 'Last failure',
            cell: (w) =>
              w.lastCrashMs ? (
                <span>
                  {when(w.lastCrashMs)} {w.lastReason && <Mono>{w.lastReason}</Mono>}
                </span>
              ) : (
                <span className="text-muted">None</span>
              ),
          },
        ]}
      />
    </Card>
  );
}

// ---- deployments --------------------------------------------------------------

export function DeploymentsTable({
  deployments,
  selected,
  onSelect,
  onActivate,
}: {
  deployments: Deployment[];
  selected?: string;
  onSelect: (id: string) => void;
  onActivate: (d: Deployment) => void;
}) {
  const active = deployments.find(isLive);
  return (
    <Table
      caption="Deploy history"
      rows={deployments}
      rowKey={(d) => d.id}
      empty={
        <Empty title="No deployments">
          Deploy functions with the Live CLI and they appear here.
        </Empty>
      }
      columns={[
        {
          key: 'id',
          header: 'Deployment',
          cell: (d) => (
            <button
              type="button"
              aria-pressed={d.id === selected}
              aria-label={`Show functions of deployment ${d.id}`}
              className={
                d.id === selected
                  ? 'bg-transparent border-0 p-0 text-left cursor-pointer text-accent-fg font-medium'
                  : 'bg-transparent border-0 p-0 text-left cursor-pointer text-ink'
              }
              onClick={() => onSelect(d.id)}
            >
              <Mono>{shortId(d.id)}</Mono>
            </button>
          ),
        },
        {
          key: 'state',
          header: 'State',
          cell: (d) => (
            <span className="flex flex-col gap-1">
              <DeploymentStateTag state={d.state} />
              {d.failureReason && <span className="text-danger text-small">{d.failureReason}</span>}
            </span>
          ),
        },
        { key: 'message', header: 'Message', cell: (d) => d.message || '—' },
        { key: 'by', header: 'By', cell: (d) => d.createdBy },
        { key: 'created', header: 'Created', cell: (d) => when(d.createdAtMs) },
        {
          key: 'bundle',
          header: 'Bundle',
          cell: (d) => (
            <span title={`sha256 ${d.bundleSha256}`}>
              <Mono>{d.bundleSha256.slice(0, 12)}</Mono>
            </span>
          ),
        },
        {
          key: 'actions',
          header: <span className="sr-only">Actions</span>,
          cell: (d) =>
            canActivate(d) ? (
              <Button
                size="sm"
                onClick={() => onActivate(d)}
                aria-label={`${active && active.createdAtMs > d.createdAtMs ? 'Roll back to' : 'Activate'} ${d.id}`}
              >
                {active && active.createdAtMs > d.createdAtMs ? 'Roll back' : 'Activate'}
              </Button>
            ) : null,
        },
      ]}
    />
  );
}

// ---- functions ----------------------------------------------------------------

export function FunctionsCard({
  detail,
  onTail,
}: {
  detail: DeploymentDetail;
  onTail: (fn: string) => void;
}) {
  const { deployment, functions } = detail;
  return (
    <Card title={`Functions in ${shortId(deployment.id)}`} flush>
      <Table
        caption={`Functions in deployment ${deployment.id}`}
        rows={functions}
        rowKey={(f) => f.path}
        empty={<Empty title="No functions">This deployment exports no functions.</Empty>}
        columns={[
          { key: 'path', header: 'Function', cell: (f) => <Mono>{f.path}</Mono> },
          { key: 'kind', header: 'Kind', cell: (f) => <Badge>{f.kind}</Badge> },
          {
            key: 'visibility',
            header: 'Visibility',
            cell: (f) =>
              f.visibility === 'internal' ? (
                <Badge title="Callable only from other functions">internal</Badge>
              ) : (
                <Badge>public</Badge>
              ),
          },
          {
            key: 'args',
            header: 'Arguments validator',
            cell: (f) =>
              f.args ? (
                <code className="font-mono text-small whitespace-pre-wrap">
                  {validatorText(f.args)}
                </code>
              ) : (
                <span className="text-muted">Not validated</span>
              ),
          },
          {
            key: 'logs',
            header: <span className="sr-only">Logs</span>,
            cell: (f) => (
              <Button
                size="sm"
                variant="quiet"
                onClick={() => onTail(f.path)}
                aria-label={`Tail logs of ${f.path}`}
              >
                Logs
              </Button>
            ),
          },
        ]}
      />
    </Card>
  );
}

// ---- logs ---------------------------------------------------------------------

const LEVEL_CLASS: Record<LogLine['level'], string> = {
  log: 'text-ink',
  debug: 'text-muted',
  info: 'text-ink',
  warn: 'text-danger',
  error: 'text-danger',
};

type KeyedLine = LogLine & { key: number };

export function LogsCard({
  api,
  functions,
  fn,
  onFn,
}: {
  api: LiveAdminApi;
  functions: string[];
  fn: string;
  onFn: (fn: string) => void;
}) {
  const [follow, setFollow] = useState(true);
  const [lines, setLines] = useState<KeyedLine[]>([]);
  const [error, setError] = useState<string>();
  const [received, setReceived] = useState(0);
  const [skipped, setSkipped] = useState(0);
  const dropped = Math.max(0, received - lines.length);

  useEffect(() => {
    const ctl = new AbortController();
    setLines([]);
    setReceived(0);
    setSkipped(0);
    setError(undefined);
    // A line's key is its arrival order in this tail: one request can log the
    // same text twice in a millisecond, so no field of the line is unique.
    let seq = 0;
    (async () => {
      try {
        for await (const l of api.tailLogs({ follow, function: fn || undefined }, ctl.signal)) {
          if (ctl.signal.aborted) return;
          if (isSkipped(l)) {
            setSkipped((n) => n + l.skipped);
            continue;
          }
          const row = { ...l, key: seq++ };
          setReceived((n) => n + 1);
          setLines((prev) => (prev.length < LOG_CAP ? [...prev, row] : [...prev.slice(1), row]));
        }
      } catch (e) {
        if (!ctl.signal.aborted) setError(errorText(e));
      }
    })();
    return () => ctl.abort();
  }, [api, fn, follow]);

  return (
    <Card title="Function logs">
      <div className="flex flex-col gap-3">
        <div className="flex items-end gap-4 flex-wrap">
          <Field label="Function">
            {(p) => (
              <Select {...p} value={fn} onChange={(e) => onFn(e.target.value)}>
                <option value="">All functions</option>
                {functions.map((f) => (
                  <option key={f} value={f}>
                    {f}
                  </option>
                ))}
                {fn && !functions.includes(fn) && <option value={fn}>{fn}</option>}
              </Select>
            )}
          </Field>
          <Checkbox
            label="Follow new lines"
            checked={follow}
            onChange={(e) => setFollow(e.target.checked)}
          />
          <Button
            size="sm"
            variant="quiet"
            onClick={() => {
              setLines([]);
              setReceived(0);
              setSkipped(0);
            }}
          >
            Clear
          </Button>
        </div>
        {error && (
          <Notice tone="danger" title="Could not tail logs">
            {error}
          </Notice>
        )}
        {skipped > 0 && (
          <p className="m-0 text-muted text-small">
            {skipped} {skipped === 1 ? 'line was' : 'lines were'} overwritten in the engine's log
            buffer before this view read {skipped === 1 ? 'it' : 'them'}.
          </p>
        )}
        {dropped > 0 && (
          <p className="m-0 text-muted text-small">
            {dropped} older {dropped === 1 ? 'line' : 'lines'} dropped from view.
          </p>
        )}
        <div
          role="log"
          aria-live="polite"
          aria-label="Function log lines"
          // oxlint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- the scrolled log must be reachable by keyboard
          tabIndex={0}
          className={
            /* mono-ok: code */ 'font-mono text-small border border-rule rounded p-2 overflow-auto max-h-80'
          }
        >
          {lines.length === 0 ? (
            <p className="m-0 text-muted">
              {follow ? 'Waiting for log lines...' : 'No log lines.'}
            </p>
          ) : (
            <ol className="m-0 p-0 list-none flex flex-col gap-1">
              {lines.map((l) => (
                <li key={l.key} className={LEVEL_CLASS[l.level]}>
                  <span className="text-muted">{new Date(l.ts).toLocaleTimeString()}</span>{' '}
                  <span className="uppercase">{l.level}</span> <span>{l.function}</span>{' '}
                  <span className="text-muted">{l.requestId}</span>{' '}
                  {l.deployment && (
                    <span className="text-muted">{`deployment ${shortId(l.deployment)}`} </span>
                  )}
                  {l.line}
                  {l.truncated && <span className="text-muted"> (truncated)</span>}
                </li>
              ))}
            </ol>
          )}
        </div>
      </div>
    </Card>
  );
}

// ---- the page -----------------------------------------------------------------

type Loaded =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; deployments: Deployment[]; workers?: WorkerHealth[] };

export function DeploymentsPage({
  api,
  navigate,
}: {
  api: LiveAdminApi;
  navigate?: (to: string) => void;
}) {
  const [loaded, setLoaded] = useState<Loaded>({ state: 'loading' });
  const [tick, setTick] = useState(0);
  const [selected, setSelected] = useState<string>();
  const [detail, setDetail] = useState<DeploymentDetail>();
  const [detailError, setDetailError] = useState<string>();
  const [logFn, setLogFn] = useState('');
  const [pending, setPending] = useState<Deployment>();
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string>();
  const [done, setDone] = useState<string>();

  const reload = useCallback(() => setTick((n) => n + 1), []);

  // oxlint-disable-next-line react/exhaustive-deps -- `tick` is the reload trigger
  useEffect(() => {
    const ctl = new AbortController();
    Promise.all([api.listDeployments(ctl.signal), api.workerHealth?.(ctl.signal)])
      .then(([deployments, workers]) => {
        if (ctl.signal.aborted) return;
        setLoaded({ state: 'ready', deployments, workers });
        setSelected((s) =>
          s && deployments.some((d) => d.id === s)
            ? s
            : (deployments.find(isLive)?.id ?? deployments[0]?.id),
        );
      })
      .catch((e) => {
        if (!ctl.signal.aborted) setLoaded({ state: 'error', message: errorText(e) });
      });
    return () => ctl.abort();
  }, [api, tick]);

  // oxlint-disable-next-line react/exhaustive-deps -- `tick` reloads the detail after an activation
  useEffect(() => {
    if (!selected) {
      setDetail(undefined);
      return;
    }
    const ctl = new AbortController();
    setDetailError(undefined);
    api
      .getDeployment(selected, ctl.signal)
      .then((d) => !ctl.signal.aborted && setDetail(d))
      .catch((e) => !ctl.signal.aborted && setDetailError(errorText(e)));
    return () => ctl.abort();
  }, [api, selected, tick]);

  const active = loaded.state === 'ready' ? loaded.deployments.find(isLive) : undefined;
  const rollingBack = !!(pending && active && active.createdAtMs > pending.createdAtMs);

  const confirm = async () => {
    if (!pending) return;
    setBusy(true);
    setActionError(undefined);
    try {
      const d = await api.activateDeployment(pending.id, active?.id);
      setDone(`${rollingBack ? 'Rolled back to' : 'Activated'} ${shortId(d.id)}.`);
      setPending(undefined);
      setSelected(d.id);
      reload();
    } catch (e) {
      setActionError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  // A detail fetched for an earlier selection is not shown while the new one loads.
  const shown = detail && detail.deployment.id === selected ? detail : undefined;
  const functionPaths = shown?.functions.map((f) => f.path) ?? [];

  return (
    <div className="lc-page">
      <header className="lc-page-head flex flex-wrap items-start justify-between gap-4">
        <div>
          <h1 className="m-0">Live deployments</h1>
          <p className="text-muted m-0">
            Deploy history, the functions each deployment serves and function logs.
          </p>
        </div>
        <div className="flex gap-2">
          {navigate && <Button onClick={() => navigate('/live')}>Back to Live</Button>}
          <Button onClick={reload}>Refresh</Button>
        </div>
      </header>
      {api.mocked && <SampleDataNotice>{PREVIEW_NOTICE}</SampleDataNotice>}
      {done && (
        <div role="status">
          <Notice tone="success" title="Done">
            {done}
          </Notice>
        </div>
      )}
      {loaded.state === 'loading' && <SkeletonList label="Loading deployments" rows={4} />}
      {loaded.state === 'error' && (
        <div className="flex flex-col gap-3">
          <Notice tone="danger" title="Could not load deployments">
            {loaded.message}
          </Notice>
          <div>
            <Button onClick={reload}>Retry</Button>
          </div>
        </div>
      )}
      {loaded.state === 'ready' && (
        <>
          <section aria-label="Active deployment" className="flex items-center gap-3">
            <span className="text-muted">Active:</span>
            {active ? (
              <>
                <Mono>{active.id}</Mono>
                <DeploymentStateTag state={active.state} />
                <span>{active.message}</span>
              </>
            ) : (
              <span className="text-muted">No deployment is active.</span>
            )}
          </section>
          {loaded.workers && <WorkerHealthCard workers={loaded.workers} />}
          <Card title="Deploy history" flush>
            <DeploymentsTable
              deployments={loaded.deployments}
              selected={selected}
              onSelect={setSelected}
              onActivate={(d) => {
                setActionError(undefined);
                setDone(undefined);
                setPending(d);
              }}
            />
          </Card>
          {detailError && (
            <Notice tone="danger" title="Could not load functions">
              {detailError}
            </Notice>
          )}
          {shown && <FunctionsCard detail={shown} onTail={setLogFn} />}
          <LogsCard api={api} functions={functionPaths} fn={logFn} onFn={setLogFn} />
        </>
      )}
      <Dialog
        open={!!pending}
        onClose={() => !busy && setPending(undefined)}
        title={rollingBack ? 'Confirm rollback' : 'Confirm activation'}
        footer={
          <>
            <Button variant="quiet" onClick={() => setPending(undefined)} disabled={busy}>
              Cancel
            </Button>
            <Button variant="primary" disabled={busy} onClick={confirm}>
              {busy ? 'Working...' : rollingBack ? 'Roll back' : 'Activate'}
            </Button>
          </>
        }
      >
        {pending && (
          <div className="flex flex-col gap-3">
            <p className="m-0">
              {rollingBack ? 'Roll back' : 'Switch'} from{' '}
              {active ? <Mono>{shortId(active.id)}</Mono> : 'no deployment'} to{' '}
              <Mono>{shortId(pending.id)}</Mono> ({pending.message || 'no message'}). New calls use
              its functions and schema, and every live query reruns once.
            </p>
            {actionError && (
              <Notice tone="danger" title="Activation failed">
                {actionError}
              </Notice>
            )}
          </div>
        )}
      </Dialog>
    </div>
  );
}
