// The Overview's cards. Each is honest about its state: "Not running" when the
// local engine or stack is down, "Not reachable" with the reason when a read
// fails, numbers only when a read succeeded. Each links to its page.

import type {
  EngineState,
  FactoryAppInfo,
  FactoryHealth,
  ServerEntry,
  StackState,
} from '@loams/desktop/contracts';
import { Card, Stat, StatusTag } from '@loams/ui';
import type { ReactNode } from 'react';
import type { ConnectorCounts, DataSummary, DurableSummary, Load, StreamsSummary } from './load.js';

type Tone = 'done' | 'progress' | 'planned' | 'failed';

export function OverviewCard({
  id,
  title,
  href,
  tag,
  children,
}: {
  id: string;
  title: string;
  href: string;
  tag?: { tone: Tone; label: string };
  children: ReactNode;
}) {
  return (
    <div data-card={id}>
      <Card title={title} actions={tag && <StatusTag status={tag.tone}>{tag.label}</StatusTag>}>
        <div className="flex flex-col gap-3">
          {children}
          <a
            className="text-sm text-accent no-underline"
            href={`#${href}`}
            aria-label={`Open ${title}`}
          >
            Open {title}
          </a>
        </div>
      </Card>
    </div>
  );
}

/** `Stats` collapses to one column in a card this narrow; two columns read better. */
const StatGrid = ({ children }: { children: ReactNode }) => (
  <div className="grid grid-cols-2 gap-2">{children}</div>
);

const Muted = ({ children }: { children: ReactNode }) => (
  <p className="text-sm text-muted m-0">{children}</p>
);

/** Where a card reads from: `down` when the active server is the local engine and it is not ready. */
export interface Where {
  down: boolean;
  /** Undefined until the first engine state or server list arrives. */
  known: boolean;
}

/** The card body for a loader that may be down, loading or failed; `ok` renders the data. */
function Gate<T>({
  where,
  load,
  what,
  ok,
}: {
  where: Where;
  load: Load<T>;
  what: string;
  ok: (data: T) => ReactNode;
}): { tag: { tone: Tone; label: string }; body: ReactNode } {
  if (where.known && where.down) {
    return {
      tag: { tone: 'planned', label: 'Not running' },
      body: <Muted>The local engine is not running, so {what} cannot be read.</Muted>,
    };
  }
  if (!where.known || load.state === 'loading') {
    return { tag: { tone: 'progress', label: 'Loading' }, body: <Muted>Reading {what}…</Muted> };
  }
  if (load.state === 'error') {
    return {
      tag: { tone: 'failed', label: 'Not reachable' },
      body: (
        <Muted>
          Could not read {what}: {load.message}
        </Muted>
      ),
    };
  }
  return { tag: { tone: 'done', label: 'OK' }, body: ok(load.data) };
}

export function EngineCard({
  engine,
  active,
}: {
  engine: EngineState | undefined;
  active: ServerEntry | undefined;
}) {
  if (active && active.kind !== 'local') {
    return (
      <OverviewCard
        id="engine"
        title="Engine"
        href="/settings/servers"
        tag={{ tone: 'done', label: 'Connected' }}
      >
        <Muted>Connected to {active.name}.</Muted>
        <code className="font-mono text-xs">{active.url}</code>
      </OverviewCard>
    );
  }
  if (!engine) {
    return (
      <OverviewCard
        id="engine"
        title="Engine"
        href="/settings/servers"
        tag={{ tone: 'progress', label: 'Loading' }}
      >
        <Muted>Reading engine state…</Muted>
      </OverviewCard>
    );
  }
  const tag: Record<EngineState['phase'], { tone: Tone; label: string }> = {
    stopped: { tone: 'planned', label: 'Not running' },
    starting: { tone: 'progress', label: 'Starting' },
    ready: { tone: 'done', label: 'Ready' },
    failed: { tone: 'failed', label: 'Failed' },
  };
  return (
    <OverviewCard id="engine" title="Engine" href="/settings/servers" tag={tag[engine.phase]}>
      {engine.phase === 'stopped' && <Muted>The local engine is not running.</Muted>}
      {engine.phase === 'starting' && <Muted>Starting (attempt {engine.attempt})…</Muted>}
      {engine.phase === 'failed' && <Muted>{engine.reason}</Muted>}
      {engine.phase === 'ready' && (
        <dl className="m-0 grid grid-cols-[6rem_1fr] gap-y-1 text-xs">
          {(
            [
              ['HTTP', engine.url],
              ['Elasticsearch', engine.esUrl],
              ['Flight', engine.flightUrl],
              ['Durable', engine.durableUrl],
              ...(engine.liveUrl ? [['Live', engine.liveUrl]] : []),
            ] as [string, string][]
          ).map(([k, v]) => (
            <div key={k} className="contents">
              <dt className="text-muted">{k}</dt>
              <dd className="m-0 font-mono truncate" title={v}>
                {v}
              </dd>
            </div>
          ))}
        </dl>
      )}
    </OverviewCard>
  );
}

export function DataCard({ where, load }: { where: Where; load: Load<DataSummary> }) {
  const g = Gate({
    where,
    load,
    what: 'the collections',
    ok: (d) => (
      <StatGrid>
        <Stat label="Collections" value={d.collections} detail={`namespace ${d.namespace}`} />
      </StatGrid>
    ),
  });
  return (
    <OverviewCard id="data" title="Data" href="/data" tag={g.tag}>
      {g.body}
    </OverviewCard>
  );
}

export function DurableCard({ where, load }: { where: Where; load: Load<DurableSummary> }) {
  const g = Gate({
    where,
    load,
    what: 'durable promises',
    ok: (d) => (
      <StatGrid>
        <Stat label="Pending promises" value={d.more ? `${d.pending}+` : d.pending} />
      </StatGrid>
    ),
  });
  return (
    <OverviewCard id="durable" title="Durable" href="/durable" tag={g.tag}>
      {g.body}
    </OverviewCard>
  );
}

export function StreamsCard({ where, load }: { where: Where; load: Load<StreamsSummary> }) {
  const g = Gate({
    where,
    load,
    what: 'streams and links',
    ok: (d) => (
      <>
        <StatGrid>
          <Stat label="Streams" value={d.streams} />
          <Stat label="Links" value={d.links} />
          <div className="col-span-2 grid">
            <Stat
              label="Max link lag"
              value={d.maxLag ? d.maxLag.records.toLocaleString() : '0'}
              detail={d.maxLag && d.maxLag.records > 0 ? d.maxLag.link : 'records'}
            />
          </div>
        </StatGrid>
        {d.unregistered > 0 && (
          <Muted>
            {d.unregistered} {d.unregistered === 1 ? 'link is' : 'links are'} unregistered.
          </Muted>
        )}
      </>
    ),
  });
  return (
    <OverviewCard id="streams" title="Streams" href="/streams" tag={g.tag}>
      {g.body}
    </OverviewCard>
  );
}

export function ConnectorsCard({ load }: { load: Load<ConnectorCounts> }) {
  let tag: { tone: Tone; label: string } = { tone: 'progress', label: 'Loading' };
  let body: ReactNode = <Muted>Reading the catalog…</Muted>;
  if (load.state === 'error') {
    tag = { tone: 'failed', label: 'Unavailable' };
    body = <Muted>Could not read the connector catalog: {load.message}</Muted>;
  } else if (load.state === 'ok') {
    tag = { tone: 'done', label: `${load.data.total} in catalog` };
    body = (
      <StatGrid>
        <Stat label="Preview" value={load.data.preview} />
        <Stat label="Planned" value={load.data.planned} />
      </StatGrid>
    );
  }
  return (
    <OverviewCard id="connectors" title="Connectors" href="/connectors" tag={tag}>
      {body}
    </OverviewCard>
  );
}

const HEALTH: Record<FactoryHealth, { tone: Tone; label: string }> = {
  ok: { tone: 'done', label: 'Connected' },
  unconfigured: { tone: 'planned', label: 'Not configured' },
  auth_failed: { tone: 'failed', label: 'Sign-in failed' },
  unreachable: { tone: 'failed', label: 'Unreachable' },
};

export function FactoryCard({ load }: { load: Load<FactoryAppInfo[]> }) {
  let tag: { tone: Tone; label: string } | undefined = { tone: 'progress', label: 'Loading' };
  let body: ReactNode = <Muted>Reading the apps…</Muted>;
  if (load.state === 'error') {
    tag = { tone: 'failed', label: 'Unavailable' };
    body = <Muted>Could not read the factory apps: {load.message}</Muted>;
  } else if (load.state === 'ok') {
    const on = load.data.filter((a) => a.health === 'ok').length;
    tag = { tone: on > 0 ? 'done' : 'planned', label: `${on} of ${load.data.length} connected` };
    body = (
      <ul className="m-0 p-0 list-none flex flex-col gap-1">
        {load.data.map((a) => (
          <li key={a.id} className="flex items-center justify-between gap-2 text-sm">
            <span>{a.label}</span>
            <StatusTag status={HEALTH[a.health].tone}>{HEALTH[a.health].label}</StatusTag>
          </li>
        ))}
      </ul>
    );
  }
  return (
    <OverviewCard id="factory" title="Software Factory" href="/factory" tag={tag}>
      {body}
    </OverviewCard>
  );
}

const STACK_TAG: Record<StackState['phase'], { tone: Tone; label: string }> = {
  unavailable: { tone: 'planned', label: 'Unavailable' },
  stopped: { tone: 'planned', label: 'Not running' },
  starting: { tone: 'progress', label: 'Starting' },
  running: { tone: 'done', label: 'Running' },
  error: { tone: 'failed', label: 'Error' },
};

const LOCAL_ONLY = { tone: 'planned', label: 'Local only' } as const;
const LocalOnly = () => (
  <Muted>Local only: available when This computer is the active server.</Muted>
);

export function StackCard({
  id,
  title,
  href,
  state,
  local = true,
}: {
  id: string;
  title: string;
  href: string;
  state: StackState | undefined;
  /** False when the active server is not this computer: the local stack is not what the page shows. */
  local?: boolean;
}) {
  if (!local) {
    return (
      <OverviewCard id={id} title={title} href={href} tag={LOCAL_ONLY}>
        <LocalOnly />
      </OverviewCard>
    );
  }
  if (!state) {
    return (
      <OverviewCard id={id} title={title} href={href} tag={{ tone: 'progress', label: 'Loading' }}>
        <Muted>Reading stack state…</Muted>
      </OverviewCard>
    );
  }
  return (
    <OverviewCard id={id} title={title} href={href} tag={STACK_TAG[state.phase]}>
      {state.phase === 'unavailable' && (
        <Muted>No container runtime found. Install Docker or Podman to run this stack.</Muted>
      )}
      {state.phase === 'stopped' && <Muted>The stack is not running.</Muted>}
      {state.phase === 'starting' && <Muted>Starting…</Muted>}
      {state.phase === 'error' && <Muted>{state.message}</Muted>}
      {state.phase === 'running' && (
        <StatGrid>
          <Stat label="Services" value={state.services.length} />
        </StatGrid>
      )}
    </OverviewCard>
  );
}

export function LiveCard({
  engine,
  local = true,
}: {
  engine: EngineState | undefined;
  local?: boolean;
}) {
  if (!local) {
    return (
      <OverviewCard id="live" title="Live" href="/live" tag={LOCAL_ONLY}>
        <LocalOnly />
      </OverviewCard>
    );
  }
  if (!engine) {
    return (
      <OverviewCard
        id="live"
        title="Live"
        href="/live"
        tag={{ tone: 'progress', label: 'Loading' }}
      >
        <Muted>Reading engine state…</Muted>
      </OverviewCard>
    );
  }
  const running = engine.phase === 'ready' && !!engine.liveUrl;
  return (
    <OverviewCard
      id="live"
      title="Live"
      href="/live"
      tag={running ? { tone: 'done', label: 'Running' } : { tone: 'planned', label: 'Not running' }}
    >
      {running && engine.phase === 'ready' ? (
        <code className="font-mono text-xs truncate">{engine.liveUrl}</code>
      ) : (
        <Muted>
          {engine.phase === 'ready'
            ? 'This engine was started without Live.'
            : 'The local engine is not running.'}
        </Muted>
      )}
    </OverviewCard>
  );
}
