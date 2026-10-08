import { validateManifest } from '@loams/console-host';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import pkg from '../package.json';
import { createStreamsClient, parseJsonU64 } from '../src/client.js';
import { LagChart, LinkPage } from '../src/pages/link.js';
import { ListPage } from '../src/pages/list.js';
import { backoffDelay, StreamPage, TailPanel, validateCloudEvent } from '../src/pages/stream.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
// jsdom has no modal dialogs.
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
const b64 = (s: string) => btoa(s);

type Handler = (url: URL, init?: RequestInit) => Response | Promise<Response>;
function clientWith(handler: Handler) {
  const fetchFn = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) =>
    handler(new URL(String(input)), init),
  ) as unknown as typeof fetch;
  return { client: createStreamsClient(fetchFn, 'loams-app://console'), fetchFn };
}

describe('streams plugin', () => {
  it('declares a valid desktop manifest', () => {
    expect(validateManifest(pkg).editions).toEqual(['desktop']);
  });

  it('streams_list_and_create', async () => {
    const streams = [
      { name: 'orders', partitions: 3 },
      { name: '_collection.articles', partitions: 1 },
    ];
    const created: unknown[] = [];
    const { client } = clientWith((url, init) => {
      if (init?.method === 'POST' && url.pathname.endsWith('/streams')) {
        const body = JSON.parse(String(init.body));
        created.push(body);
        streams.push({ name: body.name, partitions: body.partitions });
        return json({ id: 9 }, 201);
      }
      if (url.pathname.endsWith('/streams')) return json({ streams });
      return json({ links: [] });
    });
    const nav = vi.fn();
    render(<ListPage client={client} ns="default" tab="streams" navigate={nav} />);
    await screen.findByText('orders');
    // Internal streams are hidden until asked for.
    expect(screen.queryByText('_collection.articles')).toBeNull();
    fireEvent.click(screen.getByLabelText('Show internal'));
    expect(screen.getByText('_collection.articles')).toBeTruthy();
    fireEvent.click(screen.getByLabelText('Show internal'));

    fireEvent.click(screen.getAllByRole('button', { name: 'New stream' })[0] as HTMLElement);
    fireEvent.change(screen.getByLabelText('Name'), { target: { value: 'clicks' } });
    fireEvent.change(screen.getByLabelText('Partitions'), { target: { value: '4' } });
    fireEvent.change(screen.getByLabelText('Retention age (seconds)'), { target: { value: '60' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create stream' }));
    await waitFor(() => expect(created).toHaveLength(1));
    expect(created[0]).toEqual({
      name: 'clicks',
      partitions: 4,
      retention: { max_age_ms: 60000 },
    });
    await waitFor(() => expect(nav).toHaveBeenCalledWith('/streams/default/stream/clicks'));
  });

  it('tail_caps_at_500', async () => {
    const all = Array.from({ length: 600 }, (_, i) => ({
      offset: i,
      key: null,
      value: b64(`v${i}`),
      timestamp_ms: 0,
    }));
    let describes = 0;
    const { client } = clientWith((url) => {
      if (url.pathname.endsWith('/records')) {
        const from = Number(url.searchParams.get('offset'));
        return json({
          records: all.slice(from),
          next_offset: 600,
          high_watermark: 600,
          log_start_offset: 0,
        });
      }
      describes += 1;
      return json({
        id: 1,
        partitions: [{ partition: 0, log_start_offset: 0, high_watermark: 0 }],
        retention: {},
      });
    });
    render(<TailPanel client={client} ns="default" stream="orders" partition={0} pollMs={50} />);
    await screen.findByText('v599');
    expect(screen.queryByText('v99')).toBeNull();
    expect(screen.getByText('v100')).toBeTruthy();
    expect(screen.getByText(/\(500 shown\)/)).toBeTruthy();
    expect(describes).toBe(1);
  });

  it('tail_pauses_and_resumes', async () => {
    let fetches = 0;
    const { client } = clientWith((url) => {
      if (url.pathname.endsWith('/records')) {
        fetches += 1;
        return json({ records: [], next_offset: 0, high_watermark: 0, log_start_offset: 0 });
      }
      return json({
        id: 1,
        partitions: [{ partition: 0, log_start_offset: 0, high_watermark: 0 }],
        retention: {},
      });
    });
    render(<TailPanel client={client} ns="default" stream="orders" partition={0} pollMs={20} />);
    await waitFor(() => expect(fetches).toBeGreaterThan(1));
    fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
    await new Promise((r) => setTimeout(r, 60));
    const at = fetches;
    await new Promise((r) => setTimeout(r, 100));
    expect(fetches).toBe(at);
    fireEvent.click(screen.getByRole('button', { name: 'Resume' }));
    await waitFor(() => expect(fetches).toBeGreaterThan(at));
  });

  it('link_lag_bars', async () => {
    const { client } = clientWith(() =>
      json({
        id: 1,
        name: 'sink',
        source: 'orders',
        target: { kind: 'collection', name: 'orders_view' },
        options: {},
        status: 'running',
        version: 4,
        applied: [{ partition: 0, offset: 7 }],
        lag: [
          { partition: 0, records: 10 },
          { partition: 1, records: 5 },
          { partition: 2, records: 0 },
        ],
      }),
    );
    render(<LinkPage client={client} ns="default" name="sink" navigate={() => {}} />);
    await screen.findByRole('img', { name: 'Lag per partition' });
    const rect = (p: number) =>
      screen.getByTestId(`lag-bar-${p}`).querySelector('rect') as SVGRectElement;
    expect(rect(0).getAttribute('data-records')).toBe('10');
    expect(Number(rect(0).getAttribute('height'))).toBe(120);
    expect(Number(rect(1).getAttribute('height'))).toBe(60);
    expect(Number(rect(2).getAttribute('height'))).toBeLessThanOrEqual(1);
    expect(screen.getByText('Registered')).toBeTruthy();
    expect(screen.getByText('7')).toBeTruthy();
  });

  it('unregistered_status_badge', async () => {
    const { client } = clientWith((url) =>
      url.pathname.endsWith('/streams')
        ? json({ streams: [] })
        : json({
            links: [
              {
                name: 'a',
                source: 'orders',
                target: { kind: 'collection', name: 'x' },
                status: 'running',
              },
              {
                name: 'b',
                source: 'orders',
                target: { kind: 'mystery', name: 'y' },
                status: 'unregistered',
              },
            ],
          }),
    );
    render(<ListPage client={client} ns="default" tab="links" navigate={() => {}} />);
    const bad = await screen.findByText('Unregistered');
    expect(bad.closest('[title]')?.getAttribute('title')).toMatch(/No factory is registered/);
    const ok = screen.getByText('Registered');
    expect(ok.closest('[title]')?.getAttribute('title')).toMatch(/not a liveness check/);
  });

  it('creates_a_link_with_options', async () => {
    const posted: unknown[] = [];
    const { client } = clientWith((url, init) => {
      if (init?.method === 'POST') {
        posted.push(JSON.parse(String(init.body)));
        return json({ id: 2 }, 201);
      }
      if (url.pathname.endsWith('/streams'))
        return json({ streams: [{ name: 'orders', partitions: 1 }] });
      return json({ links: [] });
    });
    render(<ListPage client={client} ns="default" tab="links" navigate={() => {}} />);
    await screen.findByText('No links yet');
    fireEvent.click(screen.getAllByRole('button', { name: 'New link' })[0] as HTMLElement);
    await waitFor(() => expect(screen.getByLabelText('Source stream')).toBeTruthy());
    fireEvent.change(screen.getByLabelText('Link name'), { target: { value: 'sink' } });
    fireEvent.change(screen.getByLabelText('Target name'), { target: { value: 'view' } });
    fireEvent.change(screen.getByLabelText('Options (JSON)'), { target: { value: '{"a":"b"}' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create link' }));
    await waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0]).toEqual({
      name: 'sink',
      source: 'orders',
      target: { kind: 'counter', name: 'view' },
      options: { a: 'b' },
    });
  });

  const bounds = (hwm: number | string, partition = 0) => ({
    id: 1,
    partitions: [{ partition, log_start_offset: 0, high_watermark: hwm }],
    retention: {},
  });
  const recs = (...offsets: number[]) =>
    offsets.map((o) => ({ offset: o, key: null, value: b64(`v${o}`), timestamp_ms: 0 }));

  it('backoff_grows_to_30s_and_resets', async () => {
    expect([0, 1, 2, 3, 4, 5, 6, 20].map((n) => backoffDelay(2000, n))).toEqual([
      2000, 4000, 8000, 16000, 30000, 30000, 30000, 30000,
    ]);
    vi.useFakeTimers();
    try {
      let fail = true;
      const stamps: number[] = [];
      const { client } = clientWith((url) => {
        if (url.pathname.endsWith('/records')) {
          stamps.push(Date.now());
          return fail
            ? json({ error: 'unavailable', message: 'down' }, 503)
            : json({ records: [], next_offset: 0, high_watermark: 0, log_start_offset: 0 });
        }
        return json(bounds(0));
      });
      render(<TailPanel client={client} ns="default" stream="s" partition={0} pollMs={1000} />);
      await vi.advanceTimersByTimeAsync(0);
      await vi.advanceTimersByTimeAsync(2000 + 4000 + 8000);
      expect(stamps.length).toBe(4);
      expect(stamps.slice(1).map((t, i) => t - (stamps[i] as number))).toEqual([2000, 4000, 8000]);
      fail = false;
      await vi.advanceTimersByTimeAsync(16000);
      const after = stamps.length;
      expect(after).toBe(5);
      // Back at the base interval after a success.
      await vi.advanceTimersByTimeAsync(2000);
      expect(stamps.length - after).toBe(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('tail_keeps_cursor_over_a_transient_outage', async () => {
    let hwm = 0;
    let fail = false;
    let describes = 0;
    const log = [0, 1, 2, 3];
    const { client } = clientWith((url) => {
      if (url.pathname.endsWith('/records')) {
        if (fail) return json({ error: 'unavailable', message: 'down' }, 503);
        const from = Number(url.searchParams.get('offset'));
        const out = log.filter((o) => o >= from && o < hwm);
        return json({
          records: recs(...out),
          next_offset: hwm,
          high_watermark: hwm,
          log_start_offset: 0,
        });
      }
      describes += 1;
      return json(bounds(0));
    });
    render(<TailPanel client={client} ns="default" stream="s" partition={0} pollMs={20} />);
    await waitFor(() => expect(describes).toBe(1));
    fail = true;
    await waitFor(() => screen.getByText('down'));
    hwm = 2; // records 0 and 1 are produced during the outage
    fail = false;
    await screen.findByText('v0', {}, { timeout: 3000 });
    expect(screen.getByText('v1')).toBeTruthy();
    expect(describes).toBe(1);
  });

  it('tail_restarts_from_the_watermark_when_the_offset_is_out_of_range', async () => {
    let describes = 0;
    const seen: string[] = [];
    const { client } = clientWith((url) => {
      if (url.pathname.endsWith('/records')) {
        seen.push(url.searchParams.get('offset') as string);
        if (url.searchParams.get('offset') === '0') {
          return json(
            {
              error: 'offset_out_of_range',
              message: 'out of range',
              log_start_offset: 40,
              high_watermark: 50,
            },
            416,
          );
        }
        return json({
          records: recs(50),
          next_offset: 51,
          high_watermark: 51,
          log_start_offset: 40,
        });
      }
      describes += 1;
      return json(bounds(describes === 1 ? 0 : 50));
    });
    render(<TailPanel client={client} ns="default" stream="s" partition={0} pollMs={20} />);
    await screen.findByText('v50', {}, { timeout: 3000 });
    expect(describes).toBe(2);
    expect(seen.slice(0, 2)).toEqual(['0', '50']);
  });

  it('tail_ignores_a_stale_poll_after_the_partition_changes', async () => {
    const fetched: string[] = [];
    const both = {
      id: 1,
      partitions: [
        { partition: 0, log_start_offset: 0, high_watermark: 100 },
        { partition: 1, log_start_offset: 0, high_watermark: 5 },
      ],
      retention: {},
    };
    let describes = 0;
    let releaseFirst: () => void = () => {};
    const held = new Promise<void>((r) => {
      releaseFirst = r;
    });
    const { client } = clientWith(async (url) => {
      if (url.pathname.endsWith('/records')) {
        fetched.push(`${url.pathname.split('/').at(-2)}@${url.searchParams.get('offset')}`);
        return json({ records: [], next_offset: 5, high_watermark: 5, log_start_offset: 0 });
      }
      describes += 1;
      // The first (partition 0) describe is still in flight when the partition changes.
      if (describes === 1) await held;
      return json(both);
    });
    const view = render(
      <TailPanel client={client} ns="default" stream="s" partition={0} pollMs={20} />,
    );
    await waitFor(() => expect(describes).toBe(1));
    view.rerender(<TailPanel client={client} ns="default" stream="s" partition={1} pollMs={20} />);
    await waitFor(() => expect(fetched).toContain('1@5'));
    releaseFirst();
    await new Promise((r) => setTimeout(r, 80));
    expect(fetched.some((f) => f.startsWith('0@'))).toBe(false);
    expect(fetched.every((f) => f.startsWith('1@'))).toBe(true);
  });

  it('tail_stops_polling_on_unmount', async () => {
    let fetches = 0;
    const { client } = clientWith((url) => {
      if (url.pathname.endsWith('/records')) {
        fetches += 1;
        return json({ records: [], next_offset: 0, high_watermark: 0, log_start_offset: 0 });
      }
      return json(bounds(0));
    });
    const view = render(
      <TailPanel client={client} ns="default" stream="s" partition={0} pollMs={20} />,
    );
    await waitFor(() => expect(fetches).toBeGreaterThan(1));
    view.unmount();
    await new Promise((r) => setTimeout(r, 30));
    const at = fetches;
    await new Promise((r) => setTimeout(r, 100));
    expect(fetches).toBe(at);
  });

  it('record_values_render_inert', async () => {
    const { client } = clientWith((url) => {
      if (url.pathname.endsWith('/records')) {
        return json({
          records: [
            {
              offset: 0,
              key: b64('<b>k</b>'),
              value: b64('<img src=x onerror=alert(1)>'),
              timestamp_ms: 0,
            },
          ],
          next_offset: 1,
          high_watermark: 1,
          log_start_offset: 0,
        });
      }
      return json(bounds(0));
    });
    const { container } = render(
      <TailPanel client={client} ns="default" stream="s" partition={0} pollMs={50} />,
    );
    await screen.findByText('<img src=x onerror=alert(1)>');
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('b')).toBeNull();
  });

  it('offsets_survive_past_2_pow_53', async () => {
    const big = 2n ** 60n;
    const text = `{"records":[{"offset":${big},"key":null,"value":"${b64('x')}","timestamp_ms":0}],"next_offset":${big + 1n},"high_watermark":${big + 1n},"log_start_offset":${big - 5n}}`;
    expect(parseJsonU64(text)).toMatchObject({ next_offset: String(big + 1n) });
    const urls: string[] = [];
    const fetchFn = (async (u: RequestInfo | URL) => {
      urls.push(String(u));
      return new Response(text, { status: 200 });
    }) as unknown as typeof fetch;
    const client = createStreamsClient(fetchFn, 'x://c');
    const page = await client.fetch('default', 's', 0, String(big));
    expect(urls[0]).toContain(`offset=${big}`);
    expect(page.nextOffset).toBe(String(big + 1n));
    expect(page.records[0]?.offset).toBe(String(big));
    // The chart scales with BigInt: 2^60 and 2^59 give a 2:1 pair of bars.
    render(
      <LagChart
        lag={[
          { partition: 0, records: String(big) },
          { partition: 1, records: String(big / 2n) },
          { partition: 2, records: String(big + 1n) },
        ]}
      />,
    );
    const h = (p: number) =>
      Number(screen.getByTestId(`lag-bar-${p}`).querySelector('rect')?.getAttribute('height'));
    expect(h(2)).toBe(120);
    expect(h(1)).toBe(59);
    expect(screen.getByText(String(big + 1n))).toBeTruthy();
  });

  it('cloudevent_is_validated_before_posting', async () => {
    expect(validateCloudEvent({ specversion: '1.0', id: 'a', source: '/s', type: 't' })).toEqual(
      [],
    );
    expect(validateCloudEvent({ id: '', source: 5 }).length).toBe(4);
    const posts: string[] = [];
    let status = 'duplicate';
    const { client } = clientWith((url, init) => {
      if (url.pathname.endsWith('/records') && init?.method !== 'POST') {
        return json({ records: [], next_offset: 0, high_watermark: 0, log_start_offset: 0 });
      }
      if (init?.method === 'POST') {
        posts.push(String(init.body));
        return json({ events: [{ status, partition: 0, offset: 3 }], token: [] });
      }
      return json(bounds(0));
    });
    render(<StreamPage client={client} ns="default" name="s" navigate={() => {}} />);
    await screen.findByText('Produce a test record');
    fireEvent.change(screen.getByLabelText('Format'), { target: { value: 'cloudevent' } });
    fireEvent.change(screen.getByLabelText('CloudEvent (JSON)'), {
      target: { value: '{"id":"x"}' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Produce' }));
    await screen.findByText(/"specversion" is required/);
    expect(screen.getByText(/"type" is required/)).toBeTruthy();
    expect(posts).toHaveLength(0);
    // A valid event whose status is not "appended" is an error.
    fireEvent.change(screen.getByLabelText('Format'), { target: { value: 'json' } });
    fireEvent.change(screen.getByLabelText('Format'), { target: { value: 'cloudevent' } });
    fireEvent.click(screen.getByRole('button', { name: 'Produce' }));
    await screen.findByText(/status: duplicate/);
    expect(posts).toHaveLength(1);
    status = 'appended';
    fireEvent.click(screen.getByRole('button', { name: 'Produce' }));
    await screen.findByText(/Event appended at offset 3/);
  });
});
