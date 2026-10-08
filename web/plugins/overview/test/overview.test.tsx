import { createRouterTransport } from '@connectrpc/connect';
import type { EngineState, FactoryAppInfo, StackState } from '@loams/desktop/contracts';
import { collection } from '@loams/proto';
import { SlotProvider, SlotRegistry } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import {
  ConnectorsCard,
  DataCard,
  DurableCard,
  EngineCard,
  FactoryCard,
  LiveCard,
  StackCard,
  StreamsCard,
} from '../src/cards.js';
import { loadDurable, loadStreams, type Net, useLoad } from '../src/load.js';
import { OverviewPage } from '../src/overview-page.js';
import { fakeDesktop } from './fake.js';

afterEach(cleanup);

const ready: EngineState = {
  phase: 'ready',
  url: 'http://127.0.0.1:8080',
  esUrl: 'http://127.0.0.1:9200',
  flightUrl: 'grpc://127.0.0.1:50051',
  durableUrl: 'http://127.0.0.1:8081',
  liveUrl: 'http://127.0.0.1:8082',
  pid: 1,
};
const up = { known: true, down: false };
const down = { known: true, down: true };
const card = (id: string) => document.querySelector(`[data-card="${id}"]`) as HTMLElement;

describe('overview cards', () => {
  it('overview_cards_each_state', () => {
    const local = { id: 'local', name: 'This computer', kind: 'local' as const, url: 'x' };
    const demo = { id: 'demo', name: 'Demo', kind: 'demo' as const, url: 'http://d' };

    // Engine: every phase, and a remote server.
    for (const [engine, text] of [
      [{ phase: 'stopped' }, 'Not running'],
      [{ phase: 'starting', attempt: 2 }, 'Starting'],
      [{ phase: 'failed', reason: 'port busy', logPath: '/l' }, 'Failed'],
      [ready, 'Ready'],
    ] as [EngineState, string][]) {
      const { unmount } = render(<EngineCard engine={engine} active={local} />);
      expect(within(card('engine')).getByText(text)).toBeTruthy();
      unmount();
    }
    render(<EngineCard engine={{ phase: 'stopped' }} active={demo} />);
    expect(screen.getByText('Connected to Demo.')).toBeTruthy();
    cleanup();

    // Data, Durable and Streams: not running, loading, error, ok.
    render(<DataCard where={down} load={{ state: 'loading' }} />);
    expect(screen.getByText('Not running')).toBeTruthy();
    expect(screen.getByText(/local engine is not running/)).toBeTruthy();
    cleanup();
    render(<DataCard where={up} load={{ state: 'error', message: 'refused' }} />);
    expect(screen.getByText('Not reachable')).toBeTruthy();
    expect(screen.getByText(/refused/)).toBeTruthy();
    cleanup();
    render(
      <>
        <DataCard
          where={up}
          load={{ state: 'ok', data: { namespace: 'default', collections: 7 } }}
        />
        <DurableCard where={up} load={{ state: 'ok', data: { pending: 100, more: true } }} />
        <StreamsCard
          where={up}
          load={{
            state: 'ok',
            data: {
              streams: 3,
              links: 2,
              maxLag: { records: 1234, link: 'orders-to-pg' },
              unregistered: 1,
            },
          }}
        />
      </>,
    );
    expect(within(card('data')).getByText('7')).toBeTruthy();
    expect(within(card('durable')).getByText('100+')).toBeTruthy();
    expect(within(card('streams')).getByText('1,234')).toBeTruthy();
    expect(within(card('streams')).getByText('orders-to-pg')).toBeTruthy();
    expect(within(card('streams')).getByText('1 link is unregistered.')).toBeTruthy();
    cleanup();

    // Connectors and Factory.
    render(
      <ConnectorsCard load={{ state: 'ok', data: { total: 120, preview: 14, planned: 90 } }} />,
    );
    expect(within(card('connectors')).getByText('14')).toBeTruthy();
    expect(within(card('connectors')).getByText('90')).toBeTruthy();
    cleanup();
    const apps = [
      { id: 'forgejo', label: 'Forgejo', health: 'ok' },
      { id: 'zulip', label: 'Zulip', health: 'unconfigured' },
      { id: 'plane', label: 'Plane', health: 'auth_failed' },
      { id: 'matomo', label: 'Matomo', health: 'unreachable' },
    ] as FactoryAppInfo[];
    render(<FactoryCard load={{ state: 'ok', data: apps }} />);
    for (const t of [
      'Connected',
      'Not configured',
      'Sign-in failed',
      'Unreachable',
      '1 of 4 connected',
    ]) {
      expect(within(card('factory')).getByText(t)).toBeTruthy();
    }
    cleanup();

    // Stacks and Live.
    const stacks: [StackState, string][] = [
      [{ phase: 'unavailable', reason: 'no_container_runtime' }, 'Unavailable'],
      [{ phase: 'stopped' }, 'Not running'],
      [{ phase: 'starting' }, 'Starting'],
      [{ phase: 'running', services: [{ name: 'pg', state: 'up', ports: [] }] }, 'Running'],
      [{ phase: 'error', message: 'boom' }, 'Error'],
    ];
    for (const [state, text] of stacks) {
      const { unmount } = render(
        <StackCard id="postgres" title="Postgres" href="/postgres" state={state} />,
      );
      expect(within(card('postgres')).getByText(text)).toBeTruthy();
      unmount();
    }
    render(<LiveCard engine={ready} />);
    expect(screen.getByText('Running')).toBeTruthy();
    cleanup();
    render(<LiveCard engine={{ ...ready, liveUrl: undefined }} />);
    expect(screen.getByText('Not running')).toBeTruthy();
    expect(screen.getByText('This engine was started without Live.')).toBeTruthy();
  });

  it('every_card_links_to_its_page', () => {
    render(<StackCard id="wesql" title="WeSQL" href="/wesql" state={{ phase: 'stopped' }} />);
    expect(screen.getByRole('link', { name: 'Open WeSQL' }).getAttribute('href')).toBe('#/wesql');
  });
});

describe('overview loaders', () => {
  const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200 });

  it('streams_hide_internal_and_find_max_lag', async () => {
    const net: Net = {
      baseUrl: 'http://x',
      fetch: async (url) => {
        const u = String(url);
        if (u.endsWith('/streams')) {
          return json({ streams: [{ name: 'orders' }, { name: '_collection.a' }] });
        }
        if (u.endsWith('/links')) {
          return json({
            links: [
              { name: 'a', status: 'running' },
              { name: 'b', status: 'unregistered' },
              { name: '_collection.a.v', status: 'running' },
            ],
          });
        }
        if (u.endsWith('/links/a')) return json({ lag: [{ partition: 0, records: 5 }] });
        if (u.endsWith('/links/b')) {
          return json({
            lag: [
              { partition: 0, records: 2 },
              { partition: 1, records: 90 },
            ],
          });
        }
        return new Response('no', { status: 404 });
      },
    };
    expect(await loadStreams(net)).toEqual({
      streams: 1,
      links: 2,
      maxLag: { records: 90, link: 'b' },
      unregistered: 1,
    });
  });

  it('durable_counts_pending_through_the_envelope', async () => {
    let body: { kind: string; data: { state: string } } | undefined;
    const net: Net = {
      baseUrl: 'http://x',
      fetch: async (_u, init) => {
        body = JSON.parse(String(init?.body));
        return json({
          kind: 'promise.search',
          head: { corrId: 'c', status: 200, version: 'v' },
          data: { promises: [{ id: 'a' }, { id: 'b' }, { id: 'c' }] },
        });
      },
    };
    expect(await loadDurable(net)).toEqual({ pending: 3, more: false });
    expect(body?.kind).toBe('promise.search');
    expect(body?.data.state).toBe('pending');
  });
});

describe('overview page', () => {
  const fetched: string[] = [];
  const transport = createRouterTransport(({ service }) => {
    service(collection.CollectionService, {
      listCollections: () => ({ collections: [{ name: 'a' }, { name: 'b' }] }),
    });
  });
  const net: Net = {
    baseUrl: 'http://x',
    fetch: async (url) => {
      const u = String(url);
      fetched.push(u);
      if (u.endsWith('/durable/')) {
        return new Response(
          JSON.stringify({
            kind: 'k',
            head: { corrId: 'c', status: 200, version: 'v' },
            data: { promises: [{ id: 'p' }] },
          }),
        );
      }
      return new Response(JSON.stringify({ streams: [{ name: 's' }], links: [] }));
    },
  };
  const page = (d: ReturnType<typeof fakeDesktop>['api']) => (
    <SlotProvider registry={new SlotRegistry()}>
      <OverviewPage desktop={d} transport={transport} net={net} />
    </SlotProvider>
  );

  it('local_engine_stopped_says_not_running_and_reads_nothing', async () => {
    const { api } = fakeDesktop({ engine: { phase: 'stopped' } });
    fetched.length = 0;
    render(page(api));
    await waitFor(() => expect(within(card('engine')).getByText('Not running')).toBeTruthy());
    for (const id of ['data', 'durable', 'streams']) {
      expect(within(card(id)).getByText('Not running')).toBeTruthy();
    }
    expect(fetched).toEqual([]);
  });

  it('ready_engine_reads_every_card', async () => {
    const { api } = fakeDesktop({ engine: ready });
    render(page(api));
    await waitFor(() => expect(within(card('data')).getByText('2')).toBeTruthy());
    await waitFor(() => expect(within(card('durable')).getByText('1')).toBeTruthy());
    await waitFor(() => expect(within(card('streams')).getByText('Max link lag')).toBeTruthy());
    expect(within(card('connectors')).getByText('Preview')).toBeTruthy();
    expect(within(card('factory')).getByText('1 of 2 connected')).toBeTruthy();
    expect(within(card('postgres')).getByText('Running')).toBeTruthy();
    expect(within(card('wesql')).getByText('Not running')).toBeTruthy();
  });

  it('refresh_re_reads_everything_and_keeps_data_on_screen', async () => {
    const { api, calls } = fakeDesktop({ engine: ready });
    fetched.length = 0;
    render(page(api));
    await waitFor(() => expect(within(card('streams')).getByText('Max link lag')).toBeTruthy());
    await waitFor(() => expect(within(card('durable')).getByText('1')).toBeTruthy());
    const before = { fetched: fetched.length, calls: calls.length };
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    // No flash to Loading: the numbers stay while the reads run.
    expect(within(card('data')).getByText('2')).toBeTruthy();
    expect(screen.queryByText('Loading')).toBeNull();
    await waitFor(() => expect(fetched.length).toBeGreaterThan(before.fetched));
    await waitFor(() => expect(calls.length).toBeGreaterThan(before.calls));
    expect(calls.filter((c) => c === 'factory.list')).toHaveLength(2);
    expect(calls.filter((c) => c === 'connectors.catalog')).toHaveLength(2);
    expect(calls.filter((c) => c === 'stacks.state:postgres')).toHaveLength(2);
  });

  it('remote_server_shows_local_only_for_live_postgres_and_wesql', async () => {
    const { api } = fakeDesktop({ engine: ready, active: 'demo' });
    render(page(api));
    await waitFor(() => expect(within(card('engine')).getByText('Connected')).toBeTruthy());
    for (const id of ['live', 'postgres', 'wesql']) {
      expect(within(card(id)).getByText('Local only')).toBeTruthy();
      expect(
        within(card(id)).getByText(/available when This computer is the active server/),
      ).toBeTruthy();
    }
    // The server-backed cards still read from the remote server.
    await waitFor(() => expect(within(card('data')).getByText('2')).toBeTruthy());
  });
});

describe('useLoad', () => {
  it('keeps_data_for_the_same_scope_and_drops_it_when_the_server_changes', async () => {
    let release: (v: string) => void = () => undefined;
    const fn = () =>
      new Promise<string>((r) => {
        release = r;
      });
    const Probe = ({ k, scope }: { k: number; scope: string }) => {
      const v = useLoad(fn, k, true, scope);
      return <p data-testid="v">{v.state === 'ok' ? v.data : v.state}</p>;
    };
    const { rerender } = render(<Probe k={1} scope="a" />);
    await act(async () => release('from-a'));
    expect(screen.getByTestId('v').textContent).toBe('from-a');
    rerender(<Probe k={2} scope="a" />); // refresh, same server
    expect(screen.getByTestId('v').textContent).toBe('from-a');
    await act(async () => release('from-a-2'));
    rerender(<Probe k={3} scope="b" />); // another server
    expect(screen.getByTestId('v').textContent).toBe('loading');
  });
});
