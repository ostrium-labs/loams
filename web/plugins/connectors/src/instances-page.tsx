// `/connectors/instances` and `/connectors/instances/:name`: running connector instances (CN2),
// with pause and resume, per-split checkpoints, lag, throughput and errors.

import {
  Badge,
  Button,
  Card,
  Empty,
  Notice,
  SampleDataNotice,
  SkeletonList,
  type Status,
  StatusTag,
} from '@loams/ui';
import { useCallback, useEffect, useRef, useState } from 'react';
import {
  type FlowClient,
  formatLag,
  type InstanceDetail,
  type InstanceState,
  type InstanceSummary,
} from './flow-client.js';
import { PageHead } from './page-head.js';

const STATE_TONE: Record<InstanceState, Status> = {
  running: 'done',
  starting: 'progress',
  paused: 'planned',
  stopped: 'neutral',
  failed: 'failed',
};

export const canPause = (s: InstanceState) => s === 'running' || s === 'starting';
export const canResume = (s: InstanceState) => s === 'paused' || s === 'stopped' || s === 'failed';

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Says so when the pages run on fixture data rather than a FlowService. */
export function FixtureNotice({ flow }: { flow: FlowClient }) {
  if (!flow.fixture) return null;
  return (
    <SampleDataNotice>
      No FlowService is served yet: these instances are fixtures, and anything created, paused or
      resumed here lasts only until the app reloads.
    </SampleDataNotice>
  );
}

/** Pause or resume, with a busy flag and an error; `done` refreshes the caller. */
function usePauseResume(flow: FlowClient, done: () => void) {
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();
  const act = async (name: string, op: 'pause' | 'resume') => {
    setBusy(name);
    setError(undefined);
    try {
      const r = await (op === 'pause' ? flow.pause(name) : flow.resume(name));
      if (!r.ok) setError(r.message);
      done();
    } catch (e) {
      setError(message(e));
    } finally {
      setBusy(undefined);
    }
  };
  return { busy, error, act };
}

function PauseResume({
  i,
  busy,
  act,
}: {
  i: InstanceSummary;
  busy: string | undefined;
  act: (name: string, op: 'pause' | 'resume') => Promise<void>;
}) {
  if (canPause(i.state)) {
    return (
      <Button
        size="sm"
        disabled={busy === i.name}
        aria-label={`Pause ${i.name}`}
        onClick={() => void act(i.name, 'pause')}
      >
        Pause
      </Button>
    );
  }
  if (canResume(i.state)) {
    return (
      <Button
        size="sm"
        variant="primary"
        disabled={busy === i.name}
        aria-label={`Resume ${i.name}`}
        onClick={() => void act(i.name, 'resume')}
      >
        Resume
      </Button>
    );
  }
  return null;
}

export function InstancesPage({
  flow,
  navigate,
}: {
  flow: FlowClient;
  navigate: (to: string) => void;
}) {
  const [list, setList] = useState<InstanceSummary[]>();
  const [loadError, setLoadError] = useState<string>();
  const refresh = useCallback(() => {
    flow
      .listInstances()
      .then((l) => {
        setList(l);
        setLoadError(undefined);
      })
      .catch((e) => setLoadError(message(e)));
  }, [flow]);
  useEffect(refresh, [refresh]);
  const { busy, error, act } = usePauseResume(flow, refresh);

  return (
    <div className="lc-page">
      <PageHead
        title="Connector instances"
        subtitle="Sources and sinks running against the Fabric."
        actions={
          <Button size="sm" variant="quiet" onClick={() => navigate('/connectors')}>
            Catalog
          </Button>
        }
      />
      <FixtureNotice flow={flow} />
      {loadError && (
        <Notice tone="danger" title="Could not load instances">
          {loadError}
        </Notice>
      )}
      {error && (
        <Notice tone="danger" title="Action failed">
          {error}
        </Notice>
      )}
      {!list && !loadError && <SkeletonList label="Loading connector instances" rows={4} />}
      {list && list.length === 0 && (
        <Empty
          title="No instances yet"
          actions={
            <Button size="sm" onClick={() => navigate('/connectors')}>
              Browse the catalog
            </Button>
          }
        >
          Pick a connector in the catalog and create one.
        </Empty>
      )}
      {list && list.length > 0 && (
        <div className="loams-table-wrap">
          <table className="loams-table">
            <caption className="sr-only">Connector instances</caption>
            <thead>
              <tr>
                <th scope="col">Name</th>
                <th scope="col">Connector</th>
                <th scope="col">Direction</th>
                <th scope="col">Runtime</th>
                <th scope="col">State</th>
                <th scope="col" className="loams-num">
                  Events/s
                </th>
                <th scope="col" className="loams-num">
                  Lag
                </th>
                <th scope="col">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {list.map((i) => (
                <tr key={i.name}>
                  <th scope="row">
                    <a
                      href={`#/connectors/instances/${encodeURIComponent(i.name)}`}
                      className={/* mono-ok: id */ 'font-mono'}
                      onClick={(e) => {
                        e.preventDefault();
                        navigate(`/connectors/instances/${encodeURIComponent(i.name)}`);
                      }}
                    >
                      {i.name}
                    </a>
                  </th>
                  <td className={/* mono-ok: id */ 'font-mono text-small'}>{i.connector}</td>
                  <td>{i.direction}</td>
                  <td>
                    <Badge>{i.runtime}</Badge>
                  </td>
                  <td>
                    <StatusTag status={STATE_TONE[i.state]}>{i.state}</StatusTag>
                  </td>
                  <td className="loams-num">{i.eventsPerSec.toLocaleString('en')}</td>
                  <td className="loams-num">{formatLag(i.lagSeconds)}</td>
                  <td className="text-right">
                    <PauseResume i={i} busy={busy} act={act} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/** A small accessible bar sparkline of events per second; the numbers are in the table below it. */
function Throughput({ d }: { d: InstanceDetail }) {
  const pts = d.throughput;
  if (pts.length === 0) return <p className="m-0 text-body text-muted">No samples yet.</p>;
  const max = Math.max(1, ...pts.map((p) => p.eventsPerSec));
  const latest = pts[pts.length - 1];
  return (
    <div className="flex flex-col gap-2">
      <svg
        role="img"
        aria-label={`Events per second over the last hour, latest ${latest?.eventsPerSec ?? 0}, peak ${max}`}
        viewBox={`0 0 ${pts.length * 10} 40`}
        className="h-16 w-full"
        preserveAspectRatio="none"
      >
        {pts.map((p, i) => {
          const h = (p.eventsPerSec / max) * 38;
          return (
            <rect
              key={p.at}
              x={i * 10 + 1}
              y={40 - h}
              width={8}
              height={h}
              fill="currentColor"
              opacity={0.6}
            />
          );
        })}
      </svg>
      <p className="m-0 text-small text-muted">
        Latest {latest?.eventsPerSec.toLocaleString('en')} events/s, peak {max.toLocaleString('en')}{' '}
        events/s.
      </p>
    </div>
  );
}

export function InstanceDetailPage({
  flow,
  name,
  navigate,
}: {
  flow: FlowClient;
  name: string;
  navigate: (to: string) => void;
}) {
  // Results are tagged with the name they were loaded for: a page reused for another instance shows
  // nothing of the previous one, and a late answer for a superseded name is dropped.
  const [loaded, setLoaded] = useState<{ name: string; d?: InstanceDetail; error?: string }>();
  const current = useRef(name);
  current.current = name;
  const refresh = useCallback(() => {
    const keep = (patch: { d?: InstanceDetail; error?: string }) => {
      if (current.current !== name) return;
      setLoaded((cur) => ({ name, d: cur?.name === name ? cur.d : undefined, ...patch }));
    };
    flow
      .getInstance(name)
      .then((r) => keep(r.ok ? { d: r.value, error: undefined } : { error: r.message }))
      .catch((e) => keep({ error: message(e) }));
  }, [flow, name]);
  const d = loaded?.name === name ? loaded.d : undefined;
  const loadError = loaded?.name === name ? loaded.error : undefined;
  useEffect(refresh, [refresh]);
  const { busy, error, act } = usePauseResume(flow, refresh);

  const back = (
    <Button size="sm" variant="quiet" onClick={() => navigate('/connectors/instances')}>
      All instances
    </Button>
  );
  if (loadError) {
    return (
      <div className="lc-page">
        <PageHead title="Instance" actions={back} />
        <Notice tone="danger" title={loadError} />
      </div>
    );
  }
  if (!d) {
    return (
      <div className="lc-page">
        <PageHead title="Instance" actions={back} />
        <SkeletonList label="Loading the instance" layout="cards" rows={3} />
      </div>
    );
  }
  return (
    <div className="lc-page">
      <PageHead
        title={d.name}
        subtitle={
          <>
            <span className={/* mono-ok: id */ 'font-mono'}>{d.connector}</span> · {d.direction} ·{' '}
            {d.runtime} <StatusTag status={STATE_TONE[d.state]}>{d.state}</StatusTag>
          </>
        }
        actions={
          <>
            {back}
            <PauseResume i={d} busy={busy} act={act} />
          </>
        }
      />
      <FixtureNotice flow={flow} />
      <div className="flex flex-col gap-4">
        {error && (
          <Notice tone="danger" title="Action failed">
            {error}
          </Notice>
        )}
        {d.lastError && (
          <Notice tone="danger" title={`Last error (${d.lastError.class})`}>
            {d.lastError.message}
            {d.lastError.hint && <> {d.lastError.hint}</>}
          </Notice>
        )}
        <div className="grid gap-4 lg:grid-cols-3">
          <Card title="Lag">
            <p className="m-0 text-title font-semibold tabular-nums">{formatLag(d.lagSeconds)}</p>
          </Card>
          <Card title="Throughput">
            <Throughput d={d} />
          </Card>
          <Card title="Delivery">
            <dl className="m-0 text-body">
              <dt className="text-muted">Mode</dt>
              <dd className="m-0">{d.delivery}</dd>
              <dt className="mt-2 text-muted">Dead-lettered</dt>
              <dd className="m-0 tabular-nums">{d.dlqCount}</dd>
            </dl>
          </Card>
        </div>
        <Card title="Checkpoints">
          {d.checkpoints.length === 0 ? (
            <p className="m-0 text-body text-muted">No checkpoint committed yet.</p>
          ) : (
            <div className="loams-table-wrap">
              <table className="loams-table">
                <caption className="sr-only">Committed position per split</caption>
                <thead>
                  <tr>
                    <th scope="col">Split</th>
                    <th scope="col">Position</th>
                    <th scope="col" className="loams-num">
                      Epoch
                    </th>
                    <th scope="col">Committed</th>
                    <th scope="col" className="loams-num">
                      Lag
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {d.checkpoints.map((c) => (
                    <tr key={c.split}>
                      <th scope="row" className={/* mono-ok: id */ 'font-mono'}>
                        {c.split}
                      </th>
                      <td className={/* mono-ok: id */ 'font-mono text-small'}>{c.position}</td>
                      <td className="loams-num">{c.epoch}</td>
                      <td>
                        <time dateTime={c.committedAt}>
                          {c.committedAt.replace('T', ' ').slice(0, 19)}
                        </time>
                      </td>
                      <td className="loams-num">{formatLag(c.lagSeconds)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Card>
        <Card title="Errors">
          {d.errors.length === 0 ? (
            <p className="m-0 text-body text-muted">No errors.</p>
          ) : (
            <ul className="m-0 flex list-none flex-col gap-2 p-0 text-body">
              {d.errors.map((e) => (
                <li
                  key={`${e.at}-${e.class}`}
                  className="border-b border-rule-soft pb-2 last:border-b-0"
                >
                  <Badge>{e.class}</Badge>{' '}
                  <time dateTime={e.at} className="text-small text-muted">
                    {e.at.replace('T', ' ').slice(0, 19)}
                  </time>
                  {e.split && (
                    <span className={/* mono-ok: id */ 'ml-2 font-mono text-small'}>{e.split}</span>
                  )}
                  <p className="m-0 mt-1">{e.message}</p>
                  {e.hint && <p className="m-0 text-small text-muted">{e.hint}</p>}
                </li>
              ))}
            </ul>
          )}
        </Card>
        <Card title="Config and Fabric binding">
          <dl className="m-0 mb-3 grid grid-cols-[10rem_1fr] gap-2 text-body">
            <dt className="text-muted">Fabric</dt>
            <dd className={/* mono-ok: id */ 'm-0 font-mono'}>
              {d.fabric.kind}{' '}
              {d.fabric.kind === 'topic'
                ? d.fabric.topic
                : d.fabric.kind === 'table'
                  ? d.fabric.table
                  : d.fabric.collection}
            </dd>
            <dt className="text-muted">Secrets</dt>
            <dd className="m-0">
              {d.secrets.length === 0 ? 'none' : d.secrets.map((s) => <Badge key={s}>{s}</Badge>)}
            </dd>
            <dt className="text-muted">Version</dt>
            <dd className="m-0 tabular-nums">{d.version}</dd>
          </dl>
          <section aria-label="Instance config">
            <pre className="m-0 max-h-80 overflow-auto border border-rule bg-surface p-3 font-mono text-small">
              {JSON.stringify(d.config, null, 2)}
            </pre>
          </section>
        </Card>
      </div>
    </div>
  );
}
