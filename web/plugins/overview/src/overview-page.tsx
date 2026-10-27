import type { Transport } from '@connectrpc/connect';
import type { LoamsDesktopApi, ServerEntry, StackId, StackState } from '@loams/desktop/contracts';
import { Slot } from '@loams/slots';
import { Button, StaggerItem, StaggerList } from '@loams/ui';
import { useEffect, useMemo, useState } from 'react';
import {
  ConnectorsCard,
  DataCard,
  DurableCard,
  EngineCard,
  FactoryCard,
  LiveCard,
  StackCard,
  StreamsCard,
  type Where,
} from './cards.js';
import { LiveDashboard } from './live/live-dashboard.js';
import { mockOverviewMetrics } from './live/mock.js';
import type { OverviewMetricsClient } from './live/types.js';
import { loadConnectors, loadData, loadDurable, loadStreams, type Net, useLoad } from './load.js';

function useEngine(desktop: LoamsDesktopApi) {
  const [engine, setEngine] = useState<Awaited<ReturnType<LoamsDesktopApi['engine']['state']>>>();
  useEffect(() => {
    let live = true;
    const off = desktop.engine.onState((s) => live && setEngine(s));
    desktop.engine
      .state()
      .then((s) => live && setEngine((c) => c ?? s))
      .catch(() => undefined);
    return () => {
      live = false;
      off();
    };
  }, [desktop]);
  return engine;
}

function useActiveServer(desktop: LoamsDesktopApi): { active?: ServerEntry; known: boolean } {
  const [v, setV] = useState<{ active?: ServerEntry; known: boolean }>({ known: false });
  useEffect(() => {
    let live = true;
    desktop.servers
      .list()
      .then(({ servers, activeId }) => {
        if (live) setV({ active: servers.find((s) => s.id === activeId), known: true });
      })
      .catch(() => live && setV({ known: true }));
    return () => {
      live = false;
    };
  }, [desktop]);
  return v;
}

function useStack(desktop: LoamsDesktopApi, id: StackId, tick: number): StackState | undefined {
  const [s, setS] = useState<StackState>();
  // oxlint-disable-next-line react/exhaustive-deps -- `tick` re-reads on Refresh
  useEffect(() => {
    let live = true;
    const off = desktop.stacks.onState((sid, st) => live && sid === id && setS(st));
    desktop.stacks
      .state(id)
      .then((st) => live && setS(st))
      .catch(
        (e) =>
          live && setS({ phase: 'error', message: e instanceof Error ? e.message : String(e) }),
      );
    return () => {
      live = false;
      off();
    };
  }, [desktop, id, tick]);
  return s;
}

/** `/`: the grid of status cards, then any `environment.overview.card` entries other plugins add. */
export function OverviewPage({
  desktop,
  transport,
  net,
  metrics,
}: {
  desktop: LoamsDesktopApi;
  transport: Transport;
  net: Net;
  /** Live metrics, deployments, alerts and topology. Fixtures until a server API exists. */
  metrics?: OverviewMetricsClient;
}) {
  const [tick, setTick] = useState(0);
  const liveClient = useMemo(() => metrics ?? mockOverviewMetrics(), [metrics]);
  const engine = useEngine(desktop);
  const { active, known: serverKnown } = useActiveServer(desktop);
  const where: Where = {
    known: serverKnown && (active?.kind !== 'local' || engine !== undefined),
    down: active?.kind === 'local' && engine?.phase !== 'ready',
  };
  // Until the server list arrives, assume local so the cards read Loading, not "Local only".
  const isLocal = !serverKnown || active?.kind === 'local';
  // Re-read when the engine changes phase, the user hits Refresh, or the server changes.
  const key = `${engine?.phase}:${active?.id}:${tick}`;
  const live = where.known && !where.down;
  const data = useLoad(() => loadData(transport), key, live, active?.id);
  // Durable execution: the local engine's listener here, a remote server's main port elsewhere.
  const durable = useLoad(() => loadDurable(net), key, live, active?.id);
  const streams = useLoad(() => loadStreams(net), key, live, active?.id);
  const connectors = useLoad(() => loadConnectors(desktop), tick);
  const factory = useLoad(() => desktop.factory.list(), tick);
  const postgres = useStack(desktop, 'postgres', tick);

  return (
    <div className="lc-page">
      <header className="lc-page-head flex items-start justify-between gap-4">
        <div>
          <h1>Overview</h1>
          <p>{active ? `${active.name}: ` : ''}what is running, and what it holds.</p>
        </div>
        <Button size="sm" onClick={() => setTick((t) => t + 1)}>
          Refresh
        </Button>
      </header>
      <LiveDashboard client={liveClient} />
      <h2>Status</h2>
      <StaggerList as="div" className="lc-cards">
        <StaggerItem as="div" index={0}>
          <EngineCard engine={engine} active={active} />
        </StaggerItem>
        <StaggerItem as="div" index={1}>
          <DataCard where={where} load={data} />
        </StaggerItem>
        <StaggerItem as="div" index={2}>
          <StackCard
            id="postgres"
            title="Postgres"
            href="/postgres"
            state={postgres}
            local={isLocal}
          />
        </StaggerItem>
        <StaggerItem as="div" index={3}>
          <LiveCard engine={engine} local={isLocal} />
        </StaggerItem>
        <StaggerItem as="div" index={4}>
          <DurableCard where={where} load={durable} />
        </StaggerItem>
        <StaggerItem as="div" index={5}>
          <StreamsCard where={where} load={streams} />
        </StaggerItem>
        <StaggerItem as="div" index={6}>
          <ConnectorsCard load={connectors} />
        </StaggerItem>
        <StaggerItem as="div" index={7}>
          <FactoryCard load={factory} />
        </StaggerItem>
        <Slot name="environment.overview.card" props={{}} />
      </StaggerList>
    </div>
  );
}
