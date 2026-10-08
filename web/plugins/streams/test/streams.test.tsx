import { validateManifest } from '@loams/console-host';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import pkg from '../package.json';
import { createStreamsClient } from '../src/client.js';
import { LinkPage } from '../src/pages/link.js';
import { ListPage } from '../src/pages/list.js';
import { TailPanel } from '../src/pages/stream.js';

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
});
