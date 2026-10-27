// `/deployments`: three views over one DeployClient.
//   Live      each deployment's build -> upload -> validate -> activate (-> rollback)
//             pipeline as a React Flow graph, updated from the client's stream.
//   Promotion each workspace's path to dev and main, with the env and secret key
//             events (names only) that rode along.
//   Timeline  deploy frequency and failure rate, live, on @loams/charts.

import { LiveChart, Sparkline, useMetricStream } from '@loams/charts';
import {
  Badge,
  Button,
  Card,
  type Column,
  Dialog,
  Empty,
  formatRelative,
  Notice,
  SampleDataNotice,
  SelectionGroup,
  SelectionIndicator,
  Stat,
  Stats,
  StatusTag,
  Table,
} from '@loams/ui';
import {
  type ComponentProps,
  type KeyboardEvent,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useReducer,
  useRef,
  useState,
} from 'react';
import { deploymentGraph, PROMOTION_LABEL, promotionGraph } from './layout.js';
import {
  applyEvent,
  canRollback,
  DEPLOYMENT_STATE_LABEL,
  STAGE_LABEL,
  statusTone,
  summariseSecrets,
} from './model.js';
import { useReducedMotion } from './motion.js';
import { useNow } from './now.js';
import { PipelineCanvas } from './PipelineCanvas.js';
import { PipelineActionsContext } from './StageNode.js';
import { DeployStyles } from './styles.js';
import type {
  DeployClient,
  DeployEvent,
  Deployment,
  LogLine,
  Promotion,
  SecretEvent,
  StageKind,
} from './types.js';

type ChartSpec = ComponentProps<typeof LiveChart>['spec'];

/** The timeline's window: the last hour. */
export const TIMELINE_WINDOW_MS = 60 * 60_000;

export const FREQUENCY_SPEC: ChartSpec = {
  chartType: 'bar',
  title: 'Deploy frequency',
  subtitle: 'Deployments per hour',
  encodings: {
    x: { field: 't', type: 'temporal' },
    y: { field: 'v', type: 'quantitative' },
    color: 'series',
  },
};

export const FAILURE_SPEC: ChartSpec = {
  chartType: 'line',
  title: 'Failure rate',
  subtitle: 'Failed deployments, percent',
  encodings: {
    x: { field: 't', type: 'temporal' },
    y: { field: 'v', type: 'quantitative' },
    color: 'series',
  },
};

// ---- tabs ------------------------------------------------------------------------------------

const TABS = [
  { id: 'live', label: 'Live deployments' },
  { id: 'promotion', label: 'Environment promotion' },
  { id: 'timeline', label: 'Timeline' },
] as const;
type TabId = (typeof TABS)[number]['id'];

export function DeploymentsPage({ client, sample }: { client: DeployClient; sample?: boolean }) {
  const [tab, setTab] = useState<TabId>('live');
  const base = useId();
  const refs = useRef<Record<string, HTMLButtonElement | null>>({});
  const onKey = (e: KeyboardEvent) => {
    const i = TABS.findIndex((t) => t.id === tab);
    const to =
      e.key === 'ArrowRight'
        ? TABS[(i + 1) % TABS.length]
        : e.key === 'ArrowLeft'
          ? TABS[(i - 1 + TABS.length) % TABS.length]
          : e.key === 'Home'
            ? TABS[0]
            : e.key === 'End'
              ? TABS[TABS.length - 1]
              : undefined;
    if (!to) return;
    e.preventDefault();
    setTab(to.id);
    refs.current[to.id]?.focus();
  };
  return (
    <div className="lc-page ld-page">
      <DeployStyles />
      <header className="lc-page-head">
        <h1>Deployments</h1>
        <p>Pipelines, environment promotion and how often deploys land or fail.</p>
      </header>
      {sample && (
        <SampleDataNotice>The pipelines and deploys shown are simulated.</SampleDataNotice>
      )}
      <Summary client={client} />
      <div role="tablist" aria-label="Deployment views" className="ds-tabs" onKeyDown={onKey}>
        <SelectionGroup>
          {TABS.map((t) => (
            <button
              key={t.id}
              ref={(el) => {
                refs.current[t.id] = el;
              }}
              type="button"
              role="tab"
              className="ds-tab"
              id={`${base}-${t.id}`}
              aria-selected={tab === t.id}
              aria-controls={`${base}-panel`}
              tabIndex={tab === t.id ? 0 : -1}
              onClick={() => setTab(t.id)}
            >
              {t.label}
              {tab === t.id && (
                <SelectionIndicator layoutId="deploy-tab" className="ds-tab-indicator" />
              )}
            </button>
          ))}
        </SelectionGroup>
      </div>
      <div role="tabpanel" id={`${base}-panel`} aria-labelledby={`${base}-${tab}`}>
        {tab === 'live' && <LiveDeployments client={client} />}
        {tab === 'promotion' && <Promotions client={client} />}
        {tab === 'timeline' && <Timeline client={client} />}
      </div>
    </div>
  );
}

// ---- summary ---------------------------------------------------------------------------------

function Summary({ client }: { client: DeployClient }) {
  const freq = useCallback(() => client.metricStream('frequency'), [client]);
  const fail = useCallback(() => client.metricStream('failure_rate'), [client]);
  const freqPoints = useMetricStream(freq, 15 * 60_000);
  const failPoints = useMetricStream(fail, 15 * 60_000);
  const lastFreq = freqPoints.at(-1)?.v;
  const lastFail = failPoints.at(-1)?.v;
  return (
    <Stats>
      <Stat
        label="Deploys per hour"
        value={lastFreq === undefined ? '…' : lastFreq}
        detail={
          <Sparkline
            points={freqPoints}
            ariaLabel="Deploys per hour, last 15 minutes"
            height={28}
          />
        }
      />
      <Stat
        label="Failure rate"
        value={lastFail === undefined ? '…' : `${lastFail}%`}
        detail={
          <Sparkline points={failPoints} ariaLabel="Failure rate, last 15 minutes" height={28} />
        }
      />
    </Stats>
  );
}

// ---- live deployments ------------------------------------------------------------------------

function useStream<T, S>(
  open: (signal: AbortSignal) => AsyncIterable<T>,
  reduce: (s: S, v: T) => S,
  initial: S,
): { state: S; error?: string; loaded: boolean } {
  const [state, dispatch] = useReducer(reduce, initial);
  const [error, setError] = useState<string>();
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    const ac = new AbortController();
    (async () => {
      try {
        for await (const v of open(ac.signal)) {
          if (ac.signal.aborted) return;
          dispatch(v);
          setLoaded(true);
          setError(undefined);
        }
      } catch (e) {
        if (!ac.signal.aborted) setError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => ac.abort();
  }, [open]);
  return { state, error, loaded };
}

function LiveDeployments({ client }: { client: DeployClient }) {
  const open = useCallback((s: AbortSignal) => client.watchDeployments(s), [client]);
  const {
    state: deployments,
    error,
    loaded,
  } = useStream<DeployEvent, Deployment[]>(open, applyEvent, []);
  const reduced = useReducedMotion();
  const now = useNow(deployments.some((d) => d.state === 'running' || d.state === 'rolling_back'));
  const [logs, setLogs] = useState<{ deployment: Deployment; stage: StageKind }>();
  const [confirm, setConfirm] = useState<Deployment>();
  const [rollbackError, setRollbackError] = useState<string>();

  const actions = useMemo(
    () => ({
      openLogs: (id: string, stage: StageKind) => {
        const deployment = deployments.find((d) => d.id === id);
        if (deployment) setLogs({ deployment, stage });
      },
    }),
    [deployments],
  );

  const [rollingBack, setRollingBack] = useState(false);

  const rollback = async (d: Deployment) => {
    // One request at a time: a second click would race the first and report a false failure.
    if (rollingBack) return;
    setRollingBack(true);
    setRollbackError(undefined);
    try {
      await client.rollback(d.id);
      setConfirm(undefined);
    } catch (e) {
      setRollbackError(e instanceof Error ? e.message : String(e));
    } finally {
      setRollingBack(false);
    }
  };

  if (error) {
    return (
      <Notice tone="danger" title="Deployments are not streaming">
        {error}
      </Notice>
    );
  }
  if (!loaded)
    return (
      <p className="lc-muted" role="status">
        Loading deployments…
      </p>
    );
  if (deployments.length === 0) {
    return <Empty title="No deployments yet">A push to a tracked branch starts one.</Empty>;
  }

  return (
    <PipelineActionsContext.Provider value={actions}>
      <div className="ld-list">
        {deployments.map((d) => {
          const graph = deploymentGraph(d, now, reduced);
          return (
            <Card
              key={d.id}
              title={`${d.service} ${d.version}`}
              actions={
                <>
                  <StatusTag status={statusTone(d.state)}>
                    {DEPLOYMENT_STATE_LABEL[d.state]}
                  </StatusTag>{' '}
                  {canRollback(d) && (
                    <Button variant="secondary" size="sm" onClick={() => setConfirm(d)}>
                      Roll back to {d.previousVersion}
                    </Button>
                  )}
                </>
              }
            >
              <p className="ld-meta">
                <Badge>{d.environment}</Badge>
                <Badge title="Commit">{d.commit}</Badge>
                <span>by {d.author}</span>
                <span>{formatRelative(new Date(d.createdAt).toISOString(), now)}</span>
              </p>
              <PipelineCanvas
                label={`${d.service} ${d.version} pipeline`}
                nodes={graph.nodes}
                edges={graph.edges}
                height={graph.nodes.some((n) => n.id === 'rollback') ? 300 : 170}
              />
            </Card>
          );
        })}
      </div>
      <Dialog
        open={!!confirm}
        onClose={() => {
          setConfirm(undefined);
          setRollbackError(undefined);
        }}
        title={confirm ? `Roll back ${confirm.service}?` : 'Roll back'}
        footer={
          confirm && (
            <>
              <Button variant="quiet" onClick={() => setConfirm(undefined)}>
                Cancel
              </Button>
              <Button
                variant="danger"
                disabled={rollingBack}
                aria-busy={rollingBack}
                onClick={() => void rollback(confirm)}
              >
                {rollingBack ? 'Rolling back…' : 'Roll back'}
              </Button>
            </>
          )
        }
      >
        {confirm && (
          <p>
            {confirm.environment} goes back from {confirm.version} to {confirm.previousVersion}.
            Traffic shifts to the previous version as soon as the rollback stage starts.
          </p>
        )}
        {rollbackError && (
          <Notice tone="danger" title="Not rolled back">
            {rollbackError}
          </Notice>
        )}
      </Dialog>
      <LogsDialog client={client} target={logs} onClose={() => setLogs(undefined)} />
    </PipelineActionsContext.Provider>
  );
}

function LogsDialog({
  client,
  target,
  onClose,
}: {
  client: DeployClient;
  target?: { deployment: Deployment; stage: StageKind };
  onClose: () => void;
}) {
  const [lines, setLines] = useState<LogLine[]>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    if (!target) return;
    let live = true;
    setLines(undefined);
    setError(undefined);
    client.stageLogs(target.deployment.id, target.stage).then(
      (l) => live && setLines(l),
      (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [client, target]);
  return (
    <Dialog
      open={!!target}
      onClose={onClose}
      title={
        target
          ? `${STAGE_LABEL[target.stage]} logs: ${target.deployment.service} ${target.deployment.version}`
          : 'Logs'
      }
    >
      {error && (
        <Notice tone="danger" title="Logs did not load">
          {error}
        </Notice>
      )}
      {!error && !lines && <p role="status">Loading logs…</p>}
      {lines && lines.length === 0 && <p>This stage has not written anything yet.</p>}
      {lines && lines.length > 0 && (
        <ol className="ld-logs" aria-label="Log lines">
          {lines.map((l, i) => (
            // Lines are append-only and may repeat; position is their identity.
            // oxlint-disable-next-line react/no-array-index-key -- see above
            <li key={i} data-level={l.level}>
              <time dateTime={new Date(l.at).toISOString()}>
                {new Date(l.at).toLocaleTimeString()}
              </time>{' '}
              {l.level !== 'info' && <strong>{l.level} </strong>}
              {l.text}
            </li>
          ))}
        </ol>
      )}
    </Dialog>
  );
}

// ---- promotion -------------------------------------------------------------------------------

const keepLatest = (_: Promotion[], next: Promotion[]) => next;

const SECRET_COLUMNS: Column<SecretEvent>[] = [
  { key: 'key', header: 'Key', cell: (e) => <code>{e.key}</code> },
  { key: 'kind', header: 'Change', cell: (e) => e.kind },
  { key: 'env', header: 'Env', cell: (e) => <Badge>{e.env}</Badge> },
  { key: 'step', header: 'At step', cell: (e) => PROMOTION_LABEL[e.step] },
  {
    key: 'at',
    header: 'When',
    cell: (e) => formatRelative(new Date(e.at).toISOString()),
  },
];

function Promotions({ client }: { client: DeployClient }) {
  const open = useCallback((s: AbortSignal) => client.watchPromotions(s), [client]);
  const {
    state: promotions,
    error,
    loaded,
  } = useStream<Promotion[], Promotion[]>(open, keepLatest, []);
  const reduced = useReducedMotion();
  if (error) {
    return (
      <Notice tone="danger" title="Promotions are not streaming">
        {error}
      </Notice>
    );
  }
  if (!loaded)
    return (
      <p className="lc-muted" role="status">
        Loading promotions…
      </p>
    );
  if (promotions.length === 0) {
    return <Empty title="Nothing in flight">A workspace shows here once it asks for review.</Empty>;
  }
  return (
    <div className="ld-list">
      {promotions.map((p) => {
        const graph = promotionGraph(p, reduced);
        const running = p.steps.find((s) => s.state === 'running');
        const summary = summariseSecrets(p.secretEvents);
        return (
          <Card
            key={p.id}
            title={p.title}
            actions={
              <StatusTag status={running ? 'progress' : 'done'}>
                {running ? PROMOTION_LABEL[running.kind] : 'Promoted to main'}
              </StatusTag>
            }
          >
            <p className="ld-meta">
              <Badge>{p.workspace}</Badge>
              {summary && <span>Env and secrets: {summary}</span>}
            </p>
            <PipelineCanvas
              label={`${p.title} promotion`}
              nodes={graph.nodes}
              edges={graph.edges}
              height={230}
            />
            {p.secretEvents.length > 0 && (
              <Table
                caption={`Env and secret changes for ${p.title} (key names only)`}
                columns={SECRET_COLUMNS}
                rows={p.secretEvents}
                rowKey={(e) => e.id}
              />
            )}
          </Card>
        );
      })}
    </div>
  );
}

// ---- timeline --------------------------------------------------------------------------------

function Timeline({ client }: { client: DeployClient }) {
  const [history, setHistory] = useState<{
    frequency: Record<string, unknown>[];
    failure: Record<string, unknown>[];
  }>();
  const [error, setError] = useState<string>();
  useEffect(() => {
    let live = true;
    Promise.all([
      client.metricHistory('frequency', TIMELINE_WINDOW_MS),
      client.metricHistory('failure_rate', TIMELINE_WINDOW_MS),
    ]).then(
      ([frequency, failure]) => live && setHistory({ frequency, failure }),
      (e: unknown) => live && setError(e instanceof Error ? e.message : String(e)),
    );
    return () => {
      live = false;
    };
  }, [client]);
  const freqStream = useMemo(() => client.metricStream('frequency'), [client]);
  const failStream = useMemo(() => client.metricStream('failure_rate'), [client]);

  if (error) {
    return (
      <Notice tone="danger" title="The timeline did not load">
        {error}
      </Notice>
    );
  }
  if (!history)
    return (
      <p className="lc-muted" role="status">
        Loading the timeline…
      </p>
    );
  return (
    <div className="ld-charts">
      <Card title="Deploy frequency">
        <LiveChart
          spec={FREQUENCY_SPEC}
          data={history.frequency}
          stream={freqStream}
          windowMs={TIMELINE_WINDOW_MS}
          height={260}
          ariaLabel="Deployments per hour over the last hour"
        />
      </Card>
      <Card title="Failure rate">
        <LiveChart
          spec={FAILURE_SPEC}
          data={history.failure}
          stream={failStream}
          windowMs={TIMELINE_WINDOW_MS}
          height={260}
          ariaLabel="Percent of deployments that failed over the last hour"
        />
      </Card>
    </div>
  );
}
