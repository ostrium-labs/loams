import { fromBinary, toBinary } from '@bufbuild/protobuf';
import { Code, ConnectError, createRouterTransport } from '@connectrpc/connect';
import { validateManifest } from '@loams/console-host';
import { live, liveValue } from '@loams/proto';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import pkg from '../package.json';
import { createLiveApi } from '../src/client.js';
import { ENGINE_NOT_READY, LIVE_NOT_RUNNING, LivePage } from '../src/pages.js';
import { liveTransport } from '../src/transport.js';
import { fromJs, type Json, parseJson, toJs } from '../src/value.js';

afterEach(cleanup);

// The empty state's grain canvas observes its size; jsdom has no ResizeObserver.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};

const TABLES = [
  { name: 'messages', id: '10', indexes: [{ name: 'by_author', fields: ['author'] }] },
  { name: 'users', id: '11', indexes: [] },
];
const doc = (id: string, over: Record<string, Json> = {}) => ({
  _id: id,
  _creationTime: 1_700_000_000_000,
  ...over,
});

/** A channel the watch handler drains: push() sends a Transition. */
function channel<T>() {
  const queue: T[] = [];
  let wake: (() => void) | undefined;
  return {
    push(v: T) {
      queue.push(v);
      wake?.();
    },
    async *drain(signal: AbortSignal) {
      while (!signal.aborted) {
        if (queue.length) yield queue.shift() as T;
        else await new Promise<void>((r) => (wake = r));
      }
    },
    cancel() {
      wake?.();
    },
  };
}

const transition = (rows: Json[]) =>
  ({
    sessionId: 's1',
    updates: [{ queryId: 1, update: { case: 'value', value: fromJs(rows) } }],
  }) as never;

interface Server {
  queryRows?: Json[];
  mutate?: (req: live.MutateRequest) => void;
  tablesError?: ConnectError;
  queryError?: ConnectError;
  endWatch?: boolean;
  watchSeen?: { aborted: boolean; started: number };
  feed?: ReturnType<typeof channel<unknown>>;
}

function apiWith(s: Server = {}) {
  const transport = createRouterTransport(({ service }) => {
    service(live.LiveService, {
      query(req) {
        if (req.function === '_system:tables') {
          if (s.tablesError) throw s.tablesError;
          return { ts: 1n, result: fromJs(TABLES) };
        }
        if (s.queryError) throw s.queryError;
        return { ts: 1n, result: fromJs(s.queryRows ?? []) };
      },
      mutate(req) {
        s.mutate?.(req);
        return { commitTs: 42n, result: fromJs('doc-new') };
      },
      async *watch(_req, ctx) {
        if (s.watchSeen) s.watchSeen.started++;
        ctx.signal.addEventListener('abort', () => {
          if (s.watchSeen) s.watchSeen.aborted = true;
          s.feed?.cancel();
        });
        const feed = s.feed;
        if (!feed || s.endWatch) return;
        for await (const t of feed.drain(ctx.signal)) yield t as never;
      },
    });
  });
  return createLiveApi(liveTransport({ transport }));
}

const stacksFake = (phase: 'stopped' | 'running' = 'stopped') => {
  const start = vi.fn(async () => ({ ok: true as const, value: undefined }));
  return {
    start,
    desktop: {
      stacks: {
        state: async () => ({ phase }) as never,
        start,
        stop: vi.fn(async () => ({ ok: true as const, value: undefined })),
        onState: () => () => {},
        openLogs: vi.fn(async () => ({ ok: true as const, value: undefined })),
      },
    },
  };
};

describe('live page', () => {
  it('tables_listed', async () => {
    render(<LivePage api={apiWith()} desktop={stacksFake().desktop} />);
    const table = await screen.findByRole('table', { name: 'Tables' });
    expect(within(table).getByText('messages')).toBeTruthy();
    expect(within(table).getByText('by_author(author)')).toBeTruthy();
    expect(within(table).getByText('users')).toBeTruthy();
  });

  it('documents_page_with_load_more', async () => {
    const rows = Array.from({ length: 50 }, (_, i) => doc(`d${i}`, { n: i }));
    render(<LivePage api={apiWith({ queryRows: rows })} desktop={stacksFake().desktop} />);
    expect(await screen.findByText('d0')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Load more' })).toBeTruthy();
  });

  it('watch_updates_rows', async () => {
    const feed = channel<unknown>();
    render(<LivePage api={apiWith({ feed })} desktop={stacksFake().desktop} />);
    await screen.findByRole('table', { name: 'Tables' });
    fireEvent.click(screen.getByRole('tab', { name: 'Live query' }));
    fireEvent.click(screen.getByRole('button', { name: 'Watch' }));
    feed.push(transition([doc('a1', { text: 'hello' })]));
    expect(await screen.findByText('a1')).toBeTruthy();
    expect(screen.getByText(/hello/)).toBeTruthy();
    expect(screen.getByText('live')).toBeTruthy();
    // A second Transition updates the rows in place and counts.
    feed.push(transition([doc('a1', { text: 'hello' }), doc('a2', { text: 'again' })]));
    expect(await screen.findByText('a2')).toBeTruthy();
    await waitFor(() => expect(screen.getByText('2', { selector: 'span.font-mono' })).toBeTruthy());
    // A heartbeat (no updates) does not count.
    feed.push({ sessionId: 's1', updates: [] });
    feed.push(transition([doc('a2', { text: 'again' })]));
    await waitFor(() => expect(screen.queryByText('a1')).toBeNull());
    expect(screen.getByText('3', { selector: 'span.font-mono' })).toBeTruthy();
  });

  it('unsubscribes_on_unmount', async () => {
    const watchSeen = { aborted: false, started: 0 };
    const feed = channel<unknown>();
    const { unmount } = render(
      <LivePage api={apiWith({ watchSeen, feed })} desktop={stacksFake().desktop} />,
    );
    await screen.findByRole('table', { name: 'Tables' });
    fireEvent.click(screen.getByRole('tab', { name: 'Live query' }));
    fireEvent.click(screen.getByRole('button', { name: 'Watch' }));
    await waitFor(() => expect(watchSeen.started).toBe(1));
    expect(watchSeen.aborted).toBe(false);
    unmount();
    await waitFor(() => expect(watchSeen.aborted).toBe(true));
  });

  it('mutate_confirms_and_sends_idempotency_key', async () => {
    const seen: live.MutateRequest[] = [];
    render(
      <LivePage api={apiWith({ mutate: (r) => seen.push(r) })} desktop={stacksFake().desktop} />,
    );
    await screen.findByRole('table', { name: 'Tables' });
    fireEvent.click(screen.getByRole('tab', { name: 'Mutate' }));
    fireEvent.click(screen.getByRole('button', { name: 'Insert' }));
    // Nothing is sent until the dialog is confirmed.
    expect(seen).toHaveLength(0);
    const dialog = await screen.findByRole('dialog', { hidden: true });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm' }));
    await waitFor(() => expect(seen).toHaveLength(1));
    expect(seen[0]?.function).toBe('_system:insert');
    expect(seen[0]?.idempotencyKey).toMatch(/^[0-9a-f-]{36}$/);
    expect(toJs(seen[0]?.args)).toEqual({ table: 'messages', fields: { name: 'example' } });
    expect(await screen.findByText(/committed at 42/)).toBeTruthy();

    // A second confirmation is a new call and gets a new key.
    fireEvent.change(screen.getAllByLabelText('Document id')[1] as HTMLElement, {
      target: { value: 'doc-9' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    const d2 = await screen.findByRole('dialog', { hidden: true });
    fireEvent.click(within(d2).getByRole('button', { name: 'Confirm' }));
    await waitFor(() => expect(seen).toHaveLength(2));
    expect(seen[1]?.function).toBe('_system:delete');
    expect(seen[1]?.idempotencyKey).not.toBe(seen[0]?.idempotencyKey);
  });

  it('cancel_sends_nothing', async () => {
    const seen: live.MutateRequest[] = [];
    render(
      <LivePage api={apiWith({ mutate: (r) => seen.push(r) })} desktop={stacksFake().desktop} />,
    );
    await screen.findByRole('table', { name: 'Tables' });
    fireEvent.click(screen.getByRole('tab', { name: 'Mutate' }));
    fireEvent.click(screen.getByRole('button', { name: 'Insert' }));
    const dialog = await screen.findByRole('dialog', { hidden: true });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(seen).toHaveLength(0);
  });

  it('needs_tikv_state', async () => {
    const { desktop, start } = stacksFake();
    const tablesError = new ConnectError(LIVE_NOT_RUNNING, Code.Unavailable);
    render(<LivePage api={apiWith({ tablesError })} desktop={desktop} retryMs={60_000} />);
    expect(await screen.findByText('Live needs the TiKV stack')).toBeTruthy();
    expect(screen.getByText('TiKV stack')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Start' }));
    await waitFor(() => expect(start).toHaveBeenCalledWith('tikv'));
  });

  it('leaves_the_needs_state_once_live_answers', async () => {
    const s: Server = { tablesError: new ConnectError(LIVE_NOT_RUNNING, Code.Unavailable) };
    render(<LivePage api={apiWith(s)} desktop={stacksFake().desktop} retryMs={30} />);
    await screen.findByText('Live needs the TiKV stack');
    s.tablesError = undefined;
    expect(await screen.findByRole('table', { name: 'Tables' })).toBeTruthy();
  });

  it('deploy_is_disabled_with_a_tooltip', async () => {
    render(<LivePage api={apiWith()} desktop={stacksFake().desktop} />);
    const b = screen.getByRole('button', { name: 'Deploy' }) as HTMLButtonElement;
    expect(b.disabled).toBe(true);
    expect(b.title).toBe('Not yet available (R1 Task 13)');
  });
});

describe('live page behaviour', () => {
  it('failed_mutate_then_confirm_reuses_the_key', async () => {
    const seen: live.MutateRequest[] = [];
    let n = 0;
    const api = apiWith({
      mutate: (r) => {
        seen.push(r);
        if (++n === 1) throw new ConnectError('boom', Code.Internal);
      },
    });
    render(<LivePage api={api} desktop={stacksFake().desktop} />);
    await screen.findByRole('table', { name: 'Tables' });
    fireEvent.click(screen.getByRole('tab', { name: 'Mutate' }));
    fireEvent.click(screen.getByRole('button', { name: 'Insert' }));
    const dialog = await screen.findByRole('dialog', { hidden: true });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm' }));
    expect(await screen.findByText('boom')).toBeTruthy();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm' }));
    await waitFor(() => expect(seen).toHaveLength(2));
    expect(seen[1]?.idempotencyKey).toBe(seen[0]?.idempotencyKey);
    expect(seen[0]?.idempotencyKey).toBeTruthy();
  });

  it('mutate_keeps_big_integers_exact', async () => {
    const seen: live.MutateRequest[] = [];
    render(
      <LivePage api={apiWith({ mutate: (r) => seen.push(r) })} desktop={stacksFake().desktop} />,
    );
    await screen.findByRole('table', { name: 'Tables' });
    fireEvent.click(screen.getByRole('tab', { name: 'Mutate' }));
    fireEvent.change(screen.getByLabelText('Fields (JSON)'), {
      target: { value: '{"n": 9223372036854775807}' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Insert' }));
    const dialog = await screen.findByRole('dialog', { hidden: true });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Confirm' }));
    await waitFor(() => expect(seen).toHaveLength(1));
    expect(toJs(seen[0]?.args)).toEqual({
      table: 'messages',
      fields: { n: { $int64: '9223372036854775807' } },
    });
  });

  it('engine_restarting_is_polled_not_an_error', async () => {
    const s: Server = { tablesError: new ConnectError(ENGINE_NOT_READY, Code.Unavailable) };
    render(<LivePage api={apiWith(s)} desktop={stacksFake().desktop} retryMs={30} />);
    await screen.findByText(/Loading tables/);
    expect(screen.queryByText('Could not load tables')).toBeNull();
    s.tablesError = undefined;
    expect(await screen.findByRole('table', { name: 'Tables' })).toBeTruthy();
  });

  it('other_unavailable_is_a_generic_error_with_retry', async () => {
    const tablesError = new ConnectError('connection refused', Code.Unavailable);
    render(<LivePage api={apiWith({ tablesError })} desktop={stacksFake().desktop} retryMs={20} />);
    expect(await screen.findByText('Could not load tables')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeTruthy();
    expect(screen.queryByText('Live needs the TiKV stack')).toBeNull();
  });

  it('live_going_away_on_the_ready_view_returns_to_needs_state', async () => {
    const s: Server = {};
    render(<LivePage api={apiWith(s)} desktop={stacksFake().desktop} retryMs={60_000} />);
    await screen.findByRole('table', { name: 'Tables' });
    s.queryError = new ConnectError(LIVE_NOT_RUNNING, Code.Unavailable);
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
    expect(await screen.findByText('Live needs the TiKV stack')).toBeTruthy();
  });

  it('shows_the_engines_live_notice', async () => {
    // Ruling T23-8: TiKV chosen but down, so Live shows local data and says so.
    const notice = 'Live on TiKV is unavailable; showing local data.';
    const desktop = {
      ...stacksFake().desktop,
      engine: {
        state: async () =>
          ({
            phase: 'ready',
            url: 'http://127.0.0.1:1',
            esUrl: '',
            flightUrl: '',
            durableUrl: '',
            liveUrl: 'http://127.0.0.1:2',
            liveNotice: notice,
            pid: 1,
          }) as const,
        onState: () => () => {},
      },
    };
    render(<LivePage api={apiWith()} desktop={desktop} />);
    await screen.findByRole('table', { name: 'Tables' });
    expect(await screen.findByText(notice)).toBeTruthy();
  });

  it('watch_clears_on_table_change_and_reports_a_closed_stream', async () => {
    const feed = channel<unknown>();
    const s: Server = { feed };
    render(<LivePage api={apiWith(s)} desktop={stacksFake().desktop} />);
    await screen.findByRole('table', { name: 'Tables' });
    fireEvent.click(screen.getByRole('tab', { name: 'Live query' }));
    fireEvent.click(screen.getByRole('button', { name: 'Watch' }));
    feed.push(transition([doc('a1')]));
    expect(await screen.findByText('a1')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Table'), { target: { value: 'users' } });
    await waitFor(() => expect(screen.queryByText('a1')).toBeNull());
    expect(screen.getByText('0', { selector: 'span.font-mono' })).toBeTruthy();
    // The server ending the stream is reported.
    s.endWatch = true;
    fireEvent.click(screen.getByRole('button', { name: 'Watch' }));
    expect(await screen.findByText('The watch ended')).toBeTruthy();
  });
});

describe('values', () => {
  const BIG = ['9223372036854775807', '-9223372036854775808', '9007199254740993'];

  it('roundtrips_plain_json', () => {
    const v = { a: 1, b: 1.5, c: 'x', d: [true, null], e: { f: 2 } };
    expect(toJs(fromJs(v))).toEqual(v);
    expect(fromJs(1).kind.case).toBe('int64Value');
    expect(fromJs(1.5).kind.case).toBe('doubleValue');
    // Big integers and bytes: exact through JS and through the wire encoding.
    for (const n of BIG) {
      const js = { $int64: n };
      const value = fromJs(js);
      expect(value.kind).toEqual({ case: 'int64Value', value: BigInt(n) });
      expect(toJs(value)).toEqual(js);
      const wire = fromBinary(liveValue.ValueSchema, toBinary(liveValue.ValueSchema, value));
      expect(toJs(wire)).toEqual(js);
    }
    const bytes = { $bytes: 'AQID' };
    const bv = fromJs(bytes);
    expect(bv.kind).toEqual({ case: 'bytesValue', value: new Uint8Array([1, 2, 3]) });
    expect(toJs(bv)).toEqual(bytes);
    // The display form is pasteable back.
    const doc = { n: { $int64: BIG[0] }, b: bytes, ok: 7 };
    expect(toJs(fromJs(doc))).toEqual(doc);
    expect(() => fromJs({ $int64: '9223372036854775808' })).toThrow(/int64 range/);
  });

  it('parseJson_keeps_big_integers_exact', () => {
    expect(
      parseJson('{"a": 9223372036854775807, "b": [-9223372036854775808, 9007199254740993]}'),
    ).toEqual({
      a: { $int64: '9223372036854775807' },
      b: [{ $int64: '-9223372036854775808' }, { $int64: '9007199254740993' }],
    });
    expect(parseJson('{"a": 12, "b": 1.5, "c": 1e30}')).toEqual({ a: 12, b: 1.5, c: 1e30 });
    expect(() => parseJson('{"a": 9223372036854775808}')).toThrow(/int64 range/);
  });
});

describe('manifest', () => {
  it('is_a_valid_desktop_plugin', () => {
    const m = validateManifest(pkg);
    expect(m.editions).toEqual(['desktop']);
    expect(m.inject).toContain('desktop');
    expect(m.inject).toContain('transport');
  });
});
