// The Transfers page (design §85 §12, DQ1 Task 6): resumable uploads and
// downloads of large files, and remote job runs with a live log, through the
// desktop's transfer sidecar over HTTP/3 (HTTP/2+TLS when UDP is blocked).
import type {
  IpcResult,
  LoamsDesktopApi,
  ServerEntry,
  TransferJob,
  TransferRemoteList,
  TransferSnapshot,
  TransferTarget,
} from '@loams/desktop/contracts';
import {
  Button,
  Card,
  Field,
  Input,
  Notice,
  Select,
  StatusTag,
  Textarea,
  formatBytes,
} from '@loams/ui';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  formatEta,
  formatRate,
  fraction,
  parseParams,
  stateLabel,
  stateTone,
  transportLabel,
} from './format.js';

type Transfers = NonNullable<LoamsDesktopApi['transfers']>;

function useSnapshot(api: Transfers): TransferSnapshot | null {
  const [snap, setSnap] = useState<TransferSnapshot | null>(null);
  useEffect(() => {
    let live = true;
    void api.snapshot().then((s) => live && setSnap(s));
    const off = api.onChange((s) => setSnap(s));
    return () => {
      live = false;
      off();
    };
  }, [api]);
  return snap;
}

function errorText<T>(r: IpcResult<T>): string | null {
  return r.ok ? null : r.message;
}

/** One upload, download or followed run. */
export function JobRow({ job, api }: { job: TransferJob; api: Transfers }) {
  const p = job.progress;
  const busy = job.state === 'running' || job.state === 'queued';
  const finished = job.state === 'done' || job.state === 'failed' || job.state === 'cancelled';
  return (
    <li className="lc-transfer" aria-label={`${job.kind} ${job.name}`}>
      <div className="lc-transfer-head">
        <strong>{job.name}</strong>
        <span className="lc-muted">
          {job.kind === 'run' ? 'Run' : job.kind === 'upload' ? 'Upload' : 'Download'} · {job.ns} ·{' '}
          {job.server}
        </span>
        <StatusTag status={stateTone(job)}>{stateLabel(job)}</StatusTag>
      </div>
      {job.kind !== 'run' && (
        <>
          <progress max={1} value={fraction(p)} aria-label="Progress" />
          <dl className="lc-transfer-stats">
            <div>
              <dt>Done</dt>
              <dd>
                {formatBytes(p?.bytes_done ?? 0)} of {p ? formatBytes(p.bytes_total) : '—'}
              </dd>
            </div>
            <div>
              <dt>Speed</dt>
              <dd>{formatRate(busy ? (p?.rate_bps ?? 0) : 0)}</dd>
            </div>
            <div>
              <dt>Time left</dt>
              <dd>{busy ? formatEta(p?.eta_s) : '—'}</dd>
            </div>
            <div>
              <dt>Transport</dt>
              <dd>{transportLabel(p?.transport)}</dd>
            </div>
            <div>
              <dt>In flight</dt>
              <dd>
                {p?.inflight ?? 0} chunks{p && p.retries > 0 ? `, ${p.retries} retries` : ''}
              </dd>
            </div>
          </dl>
        </>
      )}
      {job.error && (
        <Notice tone="danger" title={stateLabel(job)}>
          {job.error.message}
        </Notice>
      )}
      {job.kind === 'run' && <LogView lines={job.log} offset={job.logOffset ?? 0} />}
      <div className="lc-actions">
        {busy && job.kind !== 'run' && (
          <Button variant="quiet" onClick={() => void api.pause(job.id)}>
            Pause
          </Button>
        )}
        {(job.state === 'paused' || job.state === 'failed') && job.kind !== 'run' && (
          <Button onClick={() => void api.resume(job.id)}>Resume</Button>
        )}
        {!finished && (
          <Button variant="quiet" onClick={() => void api.cancel(job.id)}>
            Cancel
          </Button>
        )}
        {finished && (
          <Button variant="quiet" onClick={() => void api.dismiss(job.id)}>
            Dismiss
          </Button>
        )}
      </div>
    </li>
  );
}

/** A run's latest log lines; it follows the end unless the reader scrolled up. */
export function LogView({ lines, offset }: { lines: string[]; offset: number }) {
  const ref = useRef<HTMLPreElement>(null);
  const pinned = useRef(true);
  useEffect(() => {
    const el = ref.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [lines]);
  return (
    <div>
      <pre
        ref={ref}
        className="lc-transfer-log"
        aria-label="Run log"
        onScroll={(e) => {
          const el = e.currentTarget;
          pinned.current = el.scrollTop + el.clientHeight >= el.scrollHeight - 8;
        }}
      >
        {lines.join('\n')}
      </pre>
      <p className="lc-muted">{formatBytes(offset)} received; a reconnect continues from there.</p>
    </div>
  );
}

function KeyForm({
  api,
  server,
  onSaved,
}: {
  api: Transfers;
  server: ServerEntry;
  onSaved: () => void;
}) {
  const [token, setToken] = useState('');
  const [pin, setPin] = useState('');
  const [mode, setMode] = useState<'auto' | 'h3' | 'h2'>('auto');
  const [error, setError] = useState<string | null>(null);
  return (
    <form
      className="lc-transfer-key"
      onSubmit={(e) => {
        e.preventDefault();
        void api.saveKey(server.id, token, { mode, ...(pin ? { pin } : {}) }).then((r) => {
          setError(errorText(r));
          if (r.ok) {
            setToken('');
            onSaved();
          }
        });
      }}
    >
      <p>
        Transfers to <strong>{server.name}</strong> need an API key with access to the namespace. It
        is stored in the system keychain.
      </p>
      <Field label="API key" error={error}>
        {(p) => (
          <Input
            {...p}
            type="password"
            autoComplete="off"
            value={token}
            onChange={(e) => setToken(e.target.value)}
          />
        )}
      </Field>
      <Field
        label="Certificate SHA-256 (dev servers only)"
        hint="Leave empty to use the system's trusted roots."
      >
        {(p) => <Input {...p} value={pin} onChange={(e) => setPin(e.target.value.trim())} />}
      </Field>
      <Field label="Transport">
        {(p) => (
          <Select {...p} value={mode} onChange={(e) => setMode(e.target.value as typeof mode)}>
            <option value="auto">HTTP/3, falling back to HTTP/2</option>
            <option value="h3">HTTP/3 only</option>
            <option value="h2">HTTP/2 only</option>
          </Select>
        )}
      </Field>
      <Button type="submit" disabled={token.length === 0}>
        Save key
      </Button>
    </form>
  );
}

function RemoteSection({ api, target }: { api: Transfers; target: TransferTarget }) {
  const [list, setList] = useState<TransferRemoteList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [job, setJob] = useState('ci');
  const [params, setParams] = useState('');
  const refresh = useCallback(() => {
    void api.remoteList(target).then((r) => {
      setError(errorText(r));
      if (r.ok) setList(r.value);
    });
  }, [api, target]);
  useEffect(refresh, [refresh]);
  const parsed = parseParams(params);
  return (
    <>
      <Card
        title="On the server"
        actions={
          <Button variant="quiet" onClick={refresh}>
            Refresh
          </Button>
        }
      >
        {error && (
          <Notice tone="danger" title="Could not list the server's transfers">
            {error}
          </Notice>
        )}
        {list && list.transfers.length === 0 && (
          <p className="lc-muted">No transfers in this namespace yet.</p>
        )}
        {list && list.transfers.length > 0 && (
          <ul className="lc-transfer-remote">
            {list.transfers.map((t) => (
              <li key={t.tid}>
                <code>{t.tid}</code>
                <span>{t.state}</span>
                <span>{formatBytes(t.committed_bytes)}</span>
                {t.state === 'complete' && (
                  <Button variant="quiet" onClick={() => void api.download(target, t.tid, t.tid)}>
                    Download…
                  </Button>
                )}
              </li>
            ))}
          </ul>
        )}
      </Card>
      <Card title="Runs">
        <p className="lc-muted">
          A runner on the server runs the jobs its operator configured; the log streams here and a
          reconnect continues where it stopped.
        </p>
        <form
          className="lc-transfer-run"
          onSubmit={(e) => {
            e.preventDefault();
            if (parsed.error) return;
            void api.startRun(target, job, parsed.params).then((r) => {
              setError(errorText(r));
              refresh();
            });
          }}
        >
          <Field label="Job">
            {(p) => <Input {...p} value={job} onChange={(e) => setJob(e.target.value)} />}
          </Field>
          <Field label="Params (NAME=value per line)" error={parsed.error}>
            {(p) => (
              <Textarea
                {...p}
                rows={2}
                value={params}
                onChange={(e) => setParams(e.target.value)}
              />
            )}
          </Field>
          <Button type="submit" disabled={!job || Boolean(parsed.error)}>
            Start run
          </Button>
        </form>
        {list && list.runs.length > 0 && (
          <ul className="lc-transfer-remote">
            {list.runs.map((r) => (
              <li key={r.rid}>
                <code>{r.rid}</code>
                <span>{r.job}</span>
                <span>{r.state}</span>
                <Button variant="quiet" onClick={() => void api.followRun(target, r.rid, r.job)}>
                  Follow log
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Card>
    </>
  );
}

/** The page. */
export function TransfersPage({ desktop }: { desktop: LoamsDesktopApi }) {
  const api = desktop.transfers;
  const [servers, setServers] = useState<ServerEntry[]>([]);
  const [serverId, setServerId] = useState('');
  const [ns, setNs] = useState('default');
  const [hasKey, setHasKey] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    void desktop.servers.list().then(({ servers: all, activeId }) => {
      const usable = all.filter((s) => s.kind !== 'demo');
      setServers(usable);
      setServerId(usable.find((s) => s.id === activeId)?.id ?? usable[0]?.id ?? '');
    });
  }, [desktop]);
  useEffect(() => {
    if (!api || !serverId) return;
    void api.hasKey(serverId).then(setHasKey);
  }, [api, serverId]);
  if (!api) {
    return (
      <Notice tone="info" title="Transfers need a newer desktop build">
        This build of Loams Desktop has no transfer sidecar.
      </Notice>
    );
  }
  const server = servers.find((s) => s.id === serverId);
  const target: TransferTarget = { serverId, ns };
  return (
    <div className="lc-page lc-transfers">
      <header className="lc-page-head">
        <h1>Transfers</h1>
        <StatusTag status="progress">Preview</StatusTag>
      </header>
      <p className="lc-muted">
        Large files move in checkpointed chunks: a crash, a sleep or a lost network resumes from the
        last committed chunk, and every file is checked end to end.
      </p>
      <Card title="Where">
        <div className="lc-transfer-target">
          <Field label="Server">
            {(p) => (
              <Select {...p} value={serverId} onChange={(e) => setServerId(e.target.value)}>
                {servers.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
              </Select>
            )}
          </Field>
          <Field label="Namespace">
            {(p) => <Input {...p} value={ns} onChange={(e) => setNs(e.target.value.trim())} />}
          </Field>
        </div>
        {server && hasKey === false && (
          <KeyForm api={api} server={server} onSaved={() => setHasKey(true)} />
        )}
        {server && hasKey && (
          <div className="lc-actions">
            <Button onClick={() => void api.upload(target).then((r) => setError(errorText(r)))}>
              Upload a file…
            </Button>
            <Button variant="quiet" onClick={() => setHasKey(false)}>
              Change key
            </Button>
          </div>
        )}
        {error && (
          <Notice tone="danger" title="Could not start">
            {error}
          </Notice>
        )}
      </Card>
      <Jobs api={api} />
      {server && hasKey && <RemoteSection api={api} target={target} />}
    </div>
  );
}

function Jobs({ api }: { api: Transfers }) {
  const snap = useSnapshot(api);
  if (!snap) return <p className="lc-muted">Reading transfers…</p>;
  return (
    <Card title="On this computer">
      {snap.agent.phase === 'failed' && (
        <Notice tone="danger" title="The transfer sidecar stopped">
          {snap.agent.reason}
        </Notice>
      )}
      {snap.jobs.length === 0 ? (
        <p className="lc-muted">Nothing in progress.</p>
      ) : (
        <ul className="lc-transfer-list">
          {snap.jobs.map((j) => (
            <JobRow key={j.id} job={j} api={api} />
          ))}
        </ul>
      )}
    </Card>
  );
}
