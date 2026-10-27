// `/jobs`: Jobs and queues (§26 §6.11 "the console lists queues, counts, jobs, DLQs,
// schedules…"; JB1). Five tabs over one namespace: queues with depth and lease rates,
// jobs with their states and retries (and a job's payload, attempts and logs), cron
// schedules, the dead letters with retry and discard, and per-function usage from the
// `loams_function_*` families. Every call goes through a JobsClient (client.ts).

import { errorMessage, NamespacePicker, PageHead, Tabs, useLoad } from '@loams/desktop-ui';
import {
  Badge,
  Button,
  Card,
  type Column,
  Dialog,
  Empty,
  Field,
  formatBytes,
  formatNumber,
  Notice,
  Primary,
  SampleDataNotice,
  Select,
  Stat,
  Stats,
  StatusTag,
  Table,
} from '@loams/ui';
import { type ReactNode, useEffect, useRef, useState } from 'react';
import {
  type DeadLetter,
  type FunctionUsage,
  JOB_STATES,
  type JobDetail,
  type JobState,
  type JobSummary,
  type JobsClient,
  type QueueSummary,
  type ScheduleSummary,
  USAGE_WINDOWS,
  type UsageWindow,
} from './client.js';
import {
  ageText,
  backlog,
  outcomeLabel,
  outcomeStatus,
  payloadPreview,
  rateText,
  secondsText,
  stateLabel,
  stateStatus,
  timeText,
} from './format.js';

export const JOBS_PACKAGE = 'loams.jobs.v1';
const NAMESPACES_KEY = 'loams.jobs.namespaces';

export interface JobsPageProps {
  client: JobsClient;
  /** True while the client is the fixture mock: the page says the data is a sample. */
  preview?: boolean;
  /** The first namespace shown. */
  namespace?: string;
  /** The first tab shown. */
  initialTab?: 'queues' | 'jobs' | 'schedules' | 'dead-letters' | 'usage';
}

export function JobsPage({ client, preview, namespace, initialTab }: JobsPageProps) {
  const [ns, setNs] = useState(namespace ?? 'default');
  return (
    <div className="lc-page">
      <PageHead
        title="Jobs"
        badge={<Badge>{JOBS_PACKAGE}</Badge>}
        subtitle="Queues, jobs, schedules and dead letters, and what each function used."
        actions={
          <NamespacePicker ns={ns} onOpen={setNs} storageKey={NAMESPACES_KEY} idPrefix="jobs-ns" />
        }
      />
      {preview && (
        <SampleDataNotice>
          This server does not serve {JOBS_PACKAGE} yet. Retry and discard change only the sample.
        </SampleDataNotice>
      )}
      <Tabs
        key={ns}
        initial={initialTab}
        tabs={[
          { id: 'queues', label: 'Queues', content: <QueuesPanel client={client} ns={ns} /> },
          { id: 'jobs', label: 'Jobs', content: <JobsPanel client={client} ns={ns} /> },
          {
            id: 'schedules',
            label: 'Schedules',
            content: <SchedulesPanel client={client} ns={ns} />,
          },
          {
            id: 'dead-letters',
            label: 'Dead letters',
            content: <DeadLettersPanel client={client} ns={ns} />,
          },
          { id: 'usage', label: 'Usage', content: <UsagePanel client={client} ns={ns} /> },
        ]}
      />
    </div>
  );
}

interface PanelProps {
  client: JobsClient;
  ns: string;
}

function Loading({ what }: { what: string }) {
  return (
    <p className="text-muted" role="status">
      Loading {what}…
    </p>
  );
}

function Failed({ what, message, retry }: { what: string; message: string; retry: () => void }) {
  return (
    <div className="flex flex-col items-start gap-2">
      <Notice tone="danger" title={`Could not load ${what}`}>
        {message}
      </Notice>
      <Button onClick={retry}>Try again</Button>
    </div>
  );
}

function When({ iso }: { iso?: string }) {
  if (!iso) return <span className="text-muted">—</span>;
  return <time dateTime={iso}>{timeText(iso)}</time>;
}

function StateTag({ state }: { state: JobState }) {
  return <StatusTag status={stateStatus(state)}>{stateLabel(state)}</StatusTag>;
}

// ---------------------------------------------------------------- Queues

function QueuesPanel({ client, ns }: PanelProps) {
  const [loaded, reload] = useLoad(() => client.listQueues(ns), [client, ns], {
    keepOnReload: true,
  });
  if (loaded.state === 'loading') return <Loading what="queues" />;
  if (loaded.state === 'error')
    return <Failed what="queues" message={loaded.message} retry={reload} />;
  const queues = loaded.data;
  const total = (f: (q: QueueSummary) => number) => queues.reduce((s, q) => s + f(q), 0);
  const columns: Column<QueueSummary>[] = [
    {
      key: 'name',
      header: 'Queue',
      cell: (q) => (
        <Primary
          title={q.name}
          detail={[
            q.paused ? 'Paused' : 'Running',
            `${q.shards} shard${q.shards === 1 ? '' : 's'}`,
            q.deadLetterQueue ? `DLQ ${q.deadLetterQueue}` : undefined,
          ]
            .filter(Boolean)
            .join(' · ')}
        />
      ),
    },
    {
      key: 'backlog',
      header: 'Depth',
      numeric: true,
      cell: (q) => formatNumber(backlog(q.counts)),
    },
    {
      key: 'active',
      header: 'Active',
      numeric: true,
      cell: (q) =>
        q.concurrency ? `${q.counts.active} / ${q.concurrency}` : formatNumber(q.counts.active),
    },
    { key: 'failed', header: 'Failed', numeric: true, cell: (q) => formatNumber(q.counts.failed) },
    {
      key: 'dead',
      header: 'Dead-lettered',
      numeric: true,
      cell: (q) => formatNumber(q.counts.dead_lettered),
    },
    { key: 'enq', header: 'Enqueued', numeric: true, cell: (q) => rateText(q.rates.enqueued) },
    { key: 'lease', header: 'Leased', numeric: true, cell: (q) => rateText(q.rates.leased) },
    { key: 'done', header: 'Completed', numeric: true, cell: (q) => rateText(q.rates.completed) },
    {
      key: 'oldest',
      header: 'Oldest waiting',
      numeric: true,
      cell: (q) => ageText(q.oldestWaitingSeconds),
    },
  ];
  return (
    <div className="flex flex-col gap-4">
      <Stats>
        <Stat label="Queues" value={formatNumber(queues.length)} />
        <Stat
          label="Depth"
          value={formatNumber(total((q) => backlog(q.counts)))}
          detail="waiting, delayed, prioritized"
        />
        <Stat label="Active" value={formatNumber(total((q) => q.counts.active))} />
        <Stat label="Lease rate" value={rateText(total((q) => q.rates.leased))} />
      </Stats>
      <Card
        title="Queues"
        flush
        actions={
          <Button size="sm" onClick={reload}>
            Refresh
          </Button>
        }
      >
        <Table
          caption="Queues, their depth and their rates over the last minute"
          columns={columns}
          rows={queues}
          rowKey={(q) => q.name}
          empty={
            <Empty title="No queues in this namespace" seed={26}>
              A queue appears when a Celery or BullMQ app, or the SDK, first enqueues to it.
            </Empty>
          }
        />
      </Card>
    </div>
  );
}

// ---------------------------------------------------------------- Jobs

function JobsPanel({ client, ns }: PanelProps) {
  const [queue, setQueue] = useState('');
  const [state, setState] = useState<JobState | ''>('');
  const [selected, setSelected] = useState<{ queue: string; id: string } | null>(null);
  const [more, setMore] = useState<{
    items: JobSummary[];
    next?: string;
    busy: boolean;
    error?: string;
  }>({
    items: [],
    busy: false,
  });
  // Each first-page load starts a new generation; a response from an older one (a filter
  // changed or a reload ran while it was in flight) must not touch the pagination state.
  const generation = useRef(0);
  const [queues] = useLoad(() => client.listQueues(ns), [client, ns]);
  const [loaded, reload] = useLoad(
    async () => {
      const gen = ++generation.current;
      const page = await client.listJobs(ns, {
        queue: queue || undefined,
        state: state || undefined,
      });
      if (gen === generation.current) setMore({ items: [], next: page.nextPageToken, busy: false });
      return page;
    },
    [client, ns, queue, state],
    { keepOnReload: true },
  );

  const loadMore = async () => {
    if (!more.next) return;
    const gen = generation.current;
    setMore((m) => ({ ...m, busy: true, error: undefined }));
    try {
      const page = await client.listJobs(ns, {
        queue: queue || undefined,
        state: state || undefined,
        pageToken: more.next,
      });
      if (gen !== generation.current) return;
      setMore((m) => ({
        items: [...m.items, ...page.items],
        next: page.nextPageToken,
        busy: false,
      }));
    } catch (e) {
      if (gen !== generation.current) return;
      setMore((m) => ({ ...m, busy: false, error: errorMessage(e) }));
    }
  };

  const columns: Column<JobSummary>[] = [
    {
      key: 'job',
      header: 'Job',
      cell: (j) => (
        <button
          type="button"
          className="loams-link cursor-pointer border-0 bg-transparent p-0 text-left font-sans"
          aria-label={`Open job ${j.name} ${j.id}`}
          aria-pressed={selected?.id === j.id && selected.queue === j.queue}
          onClick={() => setSelected({ queue: j.queue, id: j.id })}
        >
          <Primary title={j.name} detail={<code>{j.id}</code>} />
        </button>
      ),
    },
    { key: 'queue', header: 'Queue', cell: (j) => j.queue },
    { key: 'state', header: 'State', cell: (j) => <StateTag state={j.state} /> },
    {
      key: 'attempts',
      header: 'Attempts',
      numeric: true,
      cell: (j) => (
        <span title={j.stalled ? `${j.stalled} stalled` : undefined}>
          {j.attempt} / {j.maxAttempts}
          {j.stalled > 0 ? ` (+${j.stalled} stalled)` : ''}
        </span>
      ),
    },
    {
      key: 'error',
      header: 'Last error',
      cell: (j) => (j.error ? <code>{j.error.code}</code> : <span className="text-muted">—</span>),
    },
    { key: 'updated', header: 'Updated', cell: (j) => <When iso={j.updatedAt} /> },
  ];

  const queueNames = queues.state === 'ready' ? queues.data.map((q) => q.name) : [];
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-end gap-4">
        <Field label="Queue">
          {(p) => (
            <Select {...p} value={queue} onChange={(e) => setQueue(e.target.value)}>
              <option value="">All queues</option>
              {queueNames.map((q) => (
                <option key={q} value={q}>
                  {q}
                </option>
              ))}
            </Select>
          )}
        </Field>
        <Field label="State">
          {(p) => (
            <Select
              {...p}
              value={state}
              onChange={(e) => setState(e.target.value as JobState | '')}
            >
              <option value="">All states</option>
              {JOB_STATES.map((s) => (
                <option key={s} value={s}>
                  {stateLabel(s)}
                </option>
              ))}
            </Select>
          )}
        </Field>
        <Button size="sm" onClick={reload}>
          Refresh
        </Button>
      </div>
      {loaded.state === 'loading' && <Loading what="jobs" />}
      {loaded.state === 'error' && <Failed what="jobs" message={loaded.message} retry={reload} />}
      {loaded.state === 'ready' && (
        <Card title="Jobs" flush>
          <Table
            caption="Jobs, newest first"
            columns={columns}
            rows={[...loaded.data.items, ...more.items]}
            rowKey={(j) => `${j.queue}/${j.id}`}
            onRowClick={(j) => setSelected({ queue: j.queue, id: j.id })}
            empty={
              <Empty title="No jobs match" seed={27}>
                {queue || state
                  ? 'Try another queue or state.'
                  : 'Nothing has been enqueued in this namespace.'}
              </Empty>
            }
          />
          {more.error && (
            <Notice tone="danger" title="Could not load more jobs">
              {more.error}
            </Notice>
          )}
          {more.next && (
            <div className="p-3">
              <Button size="sm" onClick={loadMore} disabled={more.busy}>
                {more.busy ? 'Loading…' : 'Load more'}
              </Button>
            </div>
          )}
        </Card>
      )}
      {selected && (
        <JobDetailView
          key={`${selected.queue}/${selected.id}`}
          client={client}
          ns={ns}
          queue={selected.queue}
          id={selected.id}
          onClose={() => setSelected(null)}
        />
      )}
    </div>
  );
}

export function JobDetailView({
  client,
  ns,
  queue,
  id,
  onClose,
}: PanelProps & { queue: string; id: string; onClose: () => void }) {
  const [loaded, reload] = useLoad(() => client.getJob(ns, queue, id), [client, ns, queue, id], {
    keepOnReload: true,
  });
  const headingRef = useRef<HTMLHeadingElement>(null);
  const ready = loaded.state === 'ready';
  // Move focus to the detail when it opens, so keyboard and screen-reader users land on it.
  useEffect(() => {
    if (ready) headingRef.current?.focus();
  }, [ready]);

  let body: ReactNode;
  if (loaded.state === 'loading') body = <Loading what="the job" />;
  else if (loaded.state === 'error')
    body = <Failed what="the job" message={loaded.message} retry={reload} />;
  else body = <JobBody job={loaded.data} />;

  return (
    <section className="loams-card" aria-labelledby={`job-${id}-title`}>
      <div className="loams-card-head">
        <h2 id={`job-${id}-title`} ref={headingRef} tabIndex={-1}>
          {loaded.state === 'ready' ? loaded.data.name : 'Job'} <code>{id}</code>
        </h2>
        <div className="flex gap-2">
          <Button size="sm" onClick={reload}>
            Refresh
          </Button>
          <Button size="sm" variant="quiet" onClick={onClose} aria-label="Close job detail">
            Close
          </Button>
        </div>
      </div>
      <div className="loams-card-body flex flex-col gap-4 p-4">{body}</div>
    </section>
  );
}

/** A labelled region that scrolls its content and is reachable by keyboard. */
function ScrollRegion({
  label,
  className,
  children,
}: {
  label: string;
  className: string;
  children: ReactNode;
}) {
  return (
    // oxlint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- a scrollable region must be keyboard reachable
    <section aria-label={label} tabIndex={0} className={`${className} overflow-auto`}>
      {children}
    </section>
  );
}

function JobBody({ job }: { job: JobDetail }) {
  const preview = payloadPreview(job.payload);
  const attemptCols: Column<JobDetail['attempts'][number]>[] = [
    { key: 'n', header: '#', numeric: true, cell: (a) => a.n },
    {
      key: 'outcome',
      header: 'Outcome',
      cell: (a) => (
        <StatusTag status={outcomeStatus(a.outcome)}>{outcomeLabel(a.outcome)}</StatusTag>
      ),
    },
    { key: 'worker', header: 'Worker', cell: (a) => <code>{a.worker}</code> },
    { key: 'started', header: 'Started', cell: (a) => <When iso={a.startedAt} /> },
    {
      key: 'took',
      header: 'Took',
      numeric: true,
      cell: (a) =>
        a.endedAt ? secondsText((Date.parse(a.endedAt) - Date.parse(a.startedAt)) / 1000) : '—',
    },
    {
      key: 'error',
      header: 'Error',
      cell: (a) =>
        a.error ? `${a.error.code}: ${a.error.message}` : <span className="text-muted">—</span>,
    },
  ];
  return (
    <>
      <dl className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-1 text-body">
        <dt className="text-muted">State</dt>
        <dd>
          <StateTag state={job.state} />
        </dd>
        <dt className="text-muted">Queue</dt>
        <dd>{job.queue}</dd>
        <dt className="text-muted">Mode</dt>
        <dd>{job.mode === 'durable' ? 'Durable (replays from its last checkpoint)' : 'Queue'}</dd>
        <dt className="text-muted">Attempts</dt>
        <dd>
          {job.attempt} of {job.maxAttempts}
          {job.stalled > 0 && `, stalled ${job.stalled}×`}
        </dd>
        {job.priority !== undefined && (
          <>
            <dt className="text-muted">Priority</dt>
            <dd>{job.priority}</dd>
          </>
        )}
        {job.jobKey && (
          <>
            <dt className="text-muted">Job key</dt>
            <dd>
              <code>{job.jobKey}</code>
            </dd>
          </>
        )}
        <dt className="text-muted">Created</dt>
        <dd>
          <When iso={job.createdAt} />
        </dd>
        <dt className="text-muted">Updated</dt>
        <dd>
          <When iso={job.updatedAt} />
        </dd>
        {job.progress !== undefined && (
          <>
            <dt className="text-muted">Progress</dt>
            <dd>
              <progress max={100} value={job.progress} aria-label="Job progress">
                {job.progress}%
              </progress>{' '}
              {job.progress}%
            </dd>
          </>
        )}
      </dl>
      {job.error && (
        <Notice tone="warn" title={`Last error: ${job.error.code}`}>
          {job.error.message}
        </Notice>
      )}

      <section aria-labelledby={`payload-${job.id}`} className="flex flex-col gap-2">
        <h3 id={`payload-${job.id}`}>
          Payload <Badge>{job.payload.contentType}</Badge>{' '}
          <span className="text-muted">{formatBytes(job.payload.sizeBytes)}</span>
        </h3>
        {preview.kind === 'text' && (
          <>
            <ScrollRegion label="Payload preview" className="max-h-80">
              <pre className="loams-pre">{preview.text}</pre>
            </ScrollRegion>
            {preview.truncated && (
              <p className="text-muted text-body">Preview cut at 4,096 characters.</p>
            )}
          </>
        )}
        {preview.kind === 'external' && (
          <p className="text-muted text-body">
            Stored in the object store as <code>{preview.blobRef}</code>; it is not previewed here.
          </p>
        )}
        {preview.kind === 'binary' && (
          <p className="text-muted text-body">Binary payload; no preview.</p>
        )}
        {Object.keys(job.headers).length > 0 && (
          <details>
            <summary>Headers ({Object.keys(job.headers).length})</summary>
            <dl className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-1 text-body">
              {Object.entries(job.headers).map(([k, v]) => (
                <div key={k} className="contents">
                  <dt>
                    <code>{k}</code>
                  </dt>
                  <dd>
                    <code>{v}</code>
                  </dd>
                </div>
              ))}
            </dl>
          </details>
        )}
      </section>

      <section aria-labelledby={`attempts-${job.id}`} className="flex flex-col gap-2">
        <h3 id={`attempts-${job.id}`}>Attempts</h3>
        <Table
          caption="Attempts, first to last"
          columns={attemptCols}
          rows={job.attempts}
          rowKey={(a) => String(a.n)}
          empty={<p className="text-muted text-body">Not leased yet.</p>}
        />
      </section>

      <section aria-labelledby={`logs-${job.id}`} className="flex flex-col gap-2">
        <h3 id={`logs-${job.id}`}>Logs</h3>
        {job.logs.length === 0 ? (
          <p className="text-muted text-body">No log lines.</p>
        ) : (
          <ScrollRegion label="Job logs" className="max-h-64">
            <ol className={/* mono-ok: code */ 'loams-pre font-mono text-body'}>
              {job.logs.map((l, i) => (
                // oxlint-disable-next-line react/no-array-index-key -- log lines are append-only and may repeat
                <li key={i}>
                  <time dateTime={l.at} className="text-muted">
                    {timeText(l.at)}
                  </time>{' '}
                  {l.text}
                </li>
              ))}
            </ol>
          </ScrollRegion>
        )}
      </section>
    </>
  );
}

// ---------------------------------------------------------------- Schedules

function SchedulesPanel({ client, ns }: PanelProps) {
  const [loaded, reload] = useLoad(() => client.listSchedules(ns), [client, ns], {
    keepOnReload: true,
  });
  if (loaded.state === 'loading') return <Loading what="schedules" />;
  if (loaded.state === 'error')
    return <Failed what="schedules" message={loaded.message} retry={reload} />;
  const columns: Column<ScheduleSummary>[] = [
    {
      key: 'id',
      header: 'Schedule',
      cell: (s) => <Primary title={s.id} detail={`${s.queue} → ${s.task}`} />,
    },
    {
      key: 'cron',
      header: 'Cron',
      cell: (s) => (
        <span>
          <code>{s.cron}</code> <span className="text-muted">{s.timezone}</span>
        </span>
      ),
    },
    {
      key: 'state',
      header: 'State',
      cell: (s) => (
        <StatusTag status={s.paused ? 'neutral' : 'progress'}>
          {s.paused ? 'Paused' : 'Active'}
        </StatusTag>
      ),
    },
    {
      key: 'next',
      header: 'Next run',
      cell: (s) => <When iso={s.paused ? undefined : s.nextRunAt} />,
    },
    { key: 'last', header: 'Last run', cell: (s) => <When iso={s.lastRunAt} /> },
  ];
  return (
    <Card
      title="Schedules"
      flush
      actions={
        <Button size="sm" onClick={reload}>
          Refresh
        </Button>
      }
    >
      <Table
        caption="Cron schedules"
        columns={columns}
        rows={loaded.data}
        rowKey={(s) => s.id}
        empty={
          <Empty title="No schedules" seed={28}>
            Celery beat on Loams, BullMQ job schedulers and the SDK's schedule call create them.
          </Empty>
        }
      />
    </Card>
  );
}

// ---------------------------------------------------------------- Dead letters

function DeadLettersPanel({ client, ns }: PanelProps) {
  const [loaded, reload] = useLoad(() => client.listDeadLetters(ns), [client, ns], {
    keepOnReload: true,
  });
  const [busy, setBusy] = useState<string | null>(null);
  const [status, setStatus] = useState<{ tone: 'success' | 'danger'; text: string } | null>(null);
  const [confirm, setConfirm] = useState<DeadLetter | null>(null);
  const statusRef = useRef<HTMLDivElement>(null);

  const act = async (d: DeadLetter, action: 'retry' | 'discard') => {
    setBusy(d.id);
    setStatus(null);
    try {
      if (action === 'retry') await client.retryDeadLetter(ns, d.queue, d.id);
      else await client.discardDeadLetter(ns, d.queue, d.id);
      setStatus({
        tone: 'success',
        text:
          action === 'retry'
            ? `Sent ${d.name} (${d.id}) back to ${d.originalQueue}.`
            : `Discarded ${d.name} (${d.id}).`,
      });
      reload();
    } catch (e) {
      setStatus({ tone: 'danger', text: `Could not ${action} ${d.id}: ${errorMessage(e)}` });
    } finally {
      setBusy(null);
      // The row is gone (or failed): put focus somewhere that says what happened.
      queueMicrotask(() => statusRef.current?.focus());
    }
  };

  const columns: Column<DeadLetter>[] = [
    {
      key: 'job',
      header: 'Job',
      cell: (d) => <Primary title={d.name} detail={<code>{d.id}</code>} />,
    },
    {
      key: 'from',
      header: 'Failed on',
      cell: (d) => <Primary title={d.originalQueue} detail={`in ${d.queue}`} />,
    },
    { key: 'attempts', header: 'Attempts', numeric: true, cell: (d) => d.attempts },
    { key: 'at', header: 'Failed', cell: (d) => <When iso={d.failedAt} /> },
    {
      key: 'error',
      header: 'Error',
      cell: (d) => (
        <span>
          <code>{d.error.code}</code> {d.error.message}
        </span>
      ),
    },
    {
      key: 'actions',
      header: <span className="sr-only">Actions</span>,
      cell: (d) => (
        <div className="flex gap-2">
          <Button
            size="sm"
            onClick={() => act(d, 'retry')}
            disabled={busy !== null}
            aria-label={`Retry ${d.name} ${d.id}`}
          >
            Retry
          </Button>
          <Button
            size="sm"
            variant="danger"
            onClick={() => setConfirm(d)}
            disabled={busy !== null}
            aria-label={`Discard ${d.name} ${d.id}`}
          >
            Discard
          </Button>
        </div>
      ),
    },
  ];

  return (
    <div className="flex flex-col gap-4">
      <div ref={statusRef} tabIndex={-1} aria-live="polite">
        {status && (
          <Notice tone={status.tone} title={status.tone === 'success' ? 'Done' : 'Failed'}>
            {status.text}
          </Notice>
        )}
      </div>
      {loaded.state === 'loading' && <Loading what="dead letters" />}
      {loaded.state === 'error' && (
        <Failed what="dead letters" message={loaded.message} retry={reload} />
      )}
      {loaded.state === 'ready' && (
        <Card
          title="Dead letters"
          flush
          actions={
            <Button size="sm" onClick={reload}>
              Refresh
            </Button>
          }
        >
          <Table
            caption="Jobs in dead-letter queues"
            columns={columns}
            rows={loaded.data}
            rowKey={(d) => `${d.queue}/${d.id}`}
            empty={
              <Empty title="No dead letters" seed={29}>
                A job lands here when it fails for good on a queue that names a dead-letter queue.
              </Empty>
            }
          />
        </Card>
      )}
      <Dialog
        open={confirm !== null}
        onClose={() => setConfirm(null)}
        title="Discard this job?"
        footer={
          <>
            <Button variant="quiet" onClick={() => setConfirm(null)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              onClick={() => {
                const d = confirm;
                setConfirm(null);
                if (d) void act(d, 'discard');
              }}
            >
              Discard
            </Button>
          </>
        }
      >
        {confirm && (
          <p>
            {confirm.name} <code>{confirm.id}</code> and its error history are removed from{' '}
            {confirm.queue}. This cannot be undone.
          </p>
        )}
      </Dialog>
    </div>
  );
}

// ---------------------------------------------------------------- Usage

const WINDOW_LABEL: Record<UsageWindow, string> = {
  '1h': 'Last hour',
  '24h': 'Last 24 hours',
  '7d': 'Last 7 days',
};

function UsagePanel({ client, ns }: PanelProps) {
  const [span, setSpan] = useState<UsageWindow>('24h');
  const [loaded, reload] = useLoad(() => client.functionUsage(ns, span), [client, ns, span], {
    keepOnReload: true,
  });
  const picker = (
    <Field label="Window">
      {(p) => (
        <Select {...p} value={span} onChange={(e) => setSpan(e.target.value as UsageWindow)}>
          {USAGE_WINDOWS.map((w) => (
            <option key={w} value={w}>
              {WINDOW_LABEL[w]}
            </option>
          ))}
        </Select>
      )}
    </Field>
  );
  if (loaded.state === 'loading')
    return (
      <div className="flex flex-col gap-4">
        {picker}
        <Loading what="usage" />
      </div>
    );
  if (loaded.state === 'error')
    return (
      <div className="flex flex-col gap-4">
        {picker}
        <Failed what="usage" message={loaded.message} retry={reload} />
      </div>
    );
  const rows = [...loaded.data].sort((a, b) => b.invocations - a.invocations);
  const sum = (f: (u: FunctionUsage) => number) => rows.reduce((s, u) => s + f(u), 0);
  const most = Math.max(1, ...rows.map((u) => u.invocations));
  const columns: Column<FunctionUsage>[] = [
    {
      key: 'fn',
      header: 'Function',
      cell: (u) => <Primary title={u.function} detail={`${u.tier} · ${u.runner}`} />,
    },
    {
      key: 'inv',
      header: 'Invocations',
      numeric: true,
      cell: (u) => (
        <span className="inline-flex items-center justify-end gap-2">
          <span
            aria-hidden="true"
            className="inline-block h-2 bg-accent"
            style={{ width: `${Math.max(2, Math.round((u.invocations / most) * 80))}px` }}
          />
          {formatNumber(u.invocations)}
        </span>
      ),
    },
    { key: 'wall', header: 'Wall time', numeric: true, cell: (u) => secondsText(u.wallSeconds) },
    { key: 'cpu', header: 'CPU time', numeric: true, cell: (u) => secondsText(u.cpuSeconds) },
    {
      key: 'avg',
      header: 'Mean wall',
      numeric: true,
      cell: (u) => (u.invocations ? secondsText(u.wallSeconds / u.invocations) : '—'),
    },
    { key: 'mem', header: 'Peak memory', numeric: true, cell: (u) => formatBytes(u.residentBytes) },
    {
      key: 'err',
      header: 'Errors',
      numeric: true,
      cell: (u) =>
        `${formatNumber(u.errors)}${u.invocations ? ` (${((u.errors / u.invocations) * 100).toFixed(1)}%)` : ''}`,
    },
  ];
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-end gap-4">
        {picker}
        <Button size="sm" onClick={reload}>
          Refresh
        </Button>
      </div>
      <Stats>
        <Stat
          label="Invocations"
          value={formatNumber(sum((u) => u.invocations))}
          detail={WINDOW_LABEL[span]}
        />
        <Stat label="Wall time" value={secondsText(sum((u) => u.wallSeconds))} />
        <Stat label="CPU time" value={secondsText(sum((u) => u.cpuSeconds))} />
        <Stat label="Errors" value={formatNumber(sum((u) => u.errors))} />
      </Stats>
      <Card title="Per function" flush>
        <Table
          caption={`Usage per function, ${WINDOW_LABEL[span].toLowerCase()}`}
          columns={columns}
          rows={rows}
          rowKey={(u) => `${u.function}/${u.tier}/${u.runner}`}
          empty={
            <Empty title="No function ran in this window" seed={30}>
              Functions report invocations, wall and CPU time here once they run.
            </Empty>
          }
        />
      </Card>
    </div>
  );
}
