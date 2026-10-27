// The Analytics (House) page against the sample House (src/mock.ts), which follows the
// `loams.house.v1` contract of §49 §18.2 until HS1 Task 8's service is served.

import { validateManifest } from '@loams/console-host';
import { codeEditorText, resultWindow, setCodeEditorText } from '@loams/ui';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import pkg from '../package.json';
import { HouseError, PAGE_MAX_ROWS, type QueryFrame, SERVER_MAX_ROWS } from '../src/client.js';
import { HousePage } from '../src/HousePage.js';
import { classify, createMockHouseClient, type MockOptions } from '../src/mock.js';
import { isAllowedSetting, settingProblem, settingsMap } from '../src/settings.js';

afterEach(cleanup);

// jsdom has no layout: the code editor measures text ranges for its selection and tooltips.
Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect ??= () => new DOMRect();

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
globalThis.requestAnimationFrame ??= (cb: FrameRequestCallback) =>
  setTimeout(() => cb(0), 0) as unknown as number;

function mount(opts: MockOptions = {}) {
  const client = createMockHouseClient({ delayMs: 0, ...opts });
  render(<HousePage client={client} />);
  return client;
}

const sqlBox = () => screen.getByLabelText('SQL');
const setSql = (text: string) => act(() => setCodeEditorText(sqlBox(), text));
const run = () => fireEvent.click(screen.getByRole('button', { name: 'Run' }));

async function collect(it: AsyncIterable<QueryFrame>): Promise<QueryFrame[]> {
  const out: QueryFrame[] = [];
  for await (const f of it) out.push(f);
  return out;
}

const base = {
  database: 'default',
  settings: {},
  params: {},
  format: 'ARROW_IPC' as const,
  maxRows: PAGE_MAX_ROWS,
  timeoutMs: 30_000,
  readOnly: true,
};

describe('manifest', () => {
  it('manifest_is_valid', () => {
    expect(() => validateManifest(pkg)).not.toThrow();
    expect(pkg.loams.plugin.editions).toEqual(['oss', 'desktop', 'cloud']);
  });
});

describe('mock house', () => {
  it('frames_arrive_in_contract_order_and_truncation_is_reported', async () => {
    const c = createMockHouseClient({ delayMs: 0, batchRows: 100 });
    const frames = await collect(
      c.executeQuery({ ...base, sql: 'SELECT number FROM numbers(1500)' }),
    );
    expect(frames[0]?.kind).toBe('header');
    expect(frames.at(-1)?.kind).toBe('summary');
    const rows = frames.flatMap((f) => (f.kind === 'batch' ? f.rows : []));
    expect(rows).toHaveLength(PAGE_MAX_ROWS);
    const summary = frames.at(-1);
    expect(summary?.kind === 'summary' && summary.summary.truncated).toBe(true);
  });

  it('max_rows_is_capped_at_the_server_ceiling', async () => {
    const c = createMockHouseClient({ delayMs: 0, batchRows: 50_000 });
    const frames = await collect(
      c.executeQuery({ ...base, sql: 'SELECT number FROM numbers(200000)', maxRows: 1e9 }),
    );
    const rows = frames.flatMap((f) => (f.kind === 'batch' ? f.rows : []));
    expect(rows).toHaveLength(SERVER_MAX_ROWS);
  });

  it('into_outfile_is_refused_with_344', () => {
    let err: unknown;
    try {
      classify({ sql: "SELECT 1 INTO OUTFILE 'x.csv'", readOnly: true, settings: {} });
    } catch (e) {
      err = e;
    }
    expect(err).toBeInstanceOf(HouseError);
    const f = (err as HouseError).failure;
    expect(f.reason).toBe('house_disabled_function');
    expect(f.metadata.clickhouse_code).toBe('344');
    expect(f.metadata.clickhouse_name).toBe('SUPPORT_IS_DISABLED');
  });

  it('keywords_inside_strings_and_comments_do_not_count', () => {
    expect(() =>
      classify({ sql: "SELECT 'INTO OUTFILE' -- FROM INFILE\n", readOnly: true, settings: {} }),
    ).not.toThrow();
  });

  it('read_only_refuses_writes_with_reason', () => {
    expect(() =>
      classify({ sql: 'INSERT INTO events VALUES (1)', readOnly: true, settings: {} }),
    ).toThrow(HouseError);
    expect(() =>
      classify({ sql: 'INSERT INTO events VALUES (1)', readOnly: false, settings: {} }),
    ).not.toThrow();
  });
});

describe('settings', () => {
  it('the_allowlist_matches_the_house', () => {
    expect(isAllowedSetting('max_threads')).toBe(true);
    expect(isAllowedSetting('output_format_json_quote_64bit_integers')).toBe(true);
    expect(isAllowedSetting('loams_snapshot_id')).toBe(true);
    expect(isAllowedSetting('allow_experimental_analyzer')).toBe(true);
    expect(isAllowedSetting('allow_experimental_eval')).toBe(false);
    expect(isAllowedSetting('format_schema')).toBe(false);
    expect(isAllowedSetting('output_format_schema')).toBe(false);
    expect(isAllowedSetting('user_files_path')).toBe(false);
    expect(isAllowedSetting('max-threads')).toBe(false);
  });

  it('caps_are_checked_before_the_run', () => {
    expect(settingProblem('max_memory_usage', '0')).toMatch(/4 GiB/);
    expect(settingProblem('max_memory_usage', String(5 * 1024 ** 3))).toMatch(/4 GiB/);
    expect(settingProblem('max_execution_time', '301')).toMatch(/300/);
    expect(settingProblem('max_execution_time', '60')).toBeUndefined();
    expect(
      settingsMap([
        { id: 1, name: 'max_threads', value: '4' },
        { id: 2, name: 'MAX_THREADS', value: '2' },
      ]),
    ).toEqual({ ok: false, problem: 'MAX_THREADS is set twice.' });
  });
});

describe('editor helpers', () => {
  it('grid_draws_only_rows_in_view', () => {
    const w = resultWindow(28 * 5000, 100_000);
    expect(w.first).toBeGreaterThan(4900);
    expect(w.last - w.first).toBeLessThan(60);
  });
});

describe('analytics page', () => {
  it('page_hidden_without_house_service', async () => {
    mount({ available: false });
    expect(await screen.findByText('Analytics is not configured on this server')).toBeTruthy();
    expect(screen.queryByLabelText('SQL')).toBeNull();
  });

  it('house_not_configured_shows_coming_soon_without_an_error', async () => {
    // The engine lists House, but the House answers house_not_configured (the preview).
    const notConfigured = () =>
      new HouseError({
        code: 'failed_precondition',
        message: 'House is not configured on this engine',
        reason: 'house_not_configured',
        metadata: {},
      });
    const client = createMockHouseClient({ delayMs: 0 });
    client.listDatabases = async () => {
      throw notConfigured();
    };
    client.executeQuery = () => ({
      [Symbol.asyncIterator]: () => ({
        next: () => Promise.reject(notConfigured()),
      }),
    });
    render(<HousePage client={client} />);
    // The file's first render is cold (the editor's modules load); on a busy CI runner it takes
    // over the 1s findBy default (#804).
    expect(
      await screen.findByText('Analytics — coming soon in a later preview', undefined, {
        timeout: 10_000,
      }),
    ).toBeTruthy();
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByLabelText('SQL')).toBeNull();
  });

  it('an_unimplemented_house_probe_shows_coming_soon', async () => {
    const client = createMockHouseClient({ delayMs: 0 });
    client.available = async () => {
      throw new HouseError({ code: 'unimplemented', message: 'not served', metadata: {} });
    };
    render(<HousePage client={client} />);
    expect(await screen.findByText('Analytics — coming soon in a later preview')).toBeTruthy();
    expect(screen.queryByText('Could not reach the server')).toBeNull();
  });

  it('console_runs_read_only_by_default', async () => {
    const client = mount();
    await screen.findByLabelText('SQL');
    setSql('SELECT kind, count() AS n FROM default.events GROUP BY kind');
    run();
    const results = await screen.findByRole('region', { name: 'Result rows' });
    expect(within(results).getByText('kind')).toBeTruthy();
    expect(within(results).getByText('LowCardinality(String)')).toBeTruthy();
    // The page also loads system.functions for completion in the background, through
    // the same client and at its own row cap; under CI load that request can land after
    // this one, so look for the console's own query, not whichever came last.
    const req = client.requests.filter((r) => !r.sql.includes('FROM system.')).at(-1);
    expect(req?.readOnly).toBe(true);
    expect(req?.maxRows).toBe(PAGE_MAX_ROWS);
    expect(req?.format).toBe('ARROW_IPC');
  });

  it('streams_rows_and_reports_truncation', async () => {
    const client = mount({ batchRows: 200 });
    await screen.findByLabelText('SQL');
    setSql('SELECT number FROM numbers(5000)');
    run();
    const banner = await screen.findByTestId('truncated-banner');
    expect(within(banner).getByText(/first 1,000 rows/)).toBeTruthy();
    fireEvent.click(within(banner).getByRole('button', { name: /Stream up to/ }));
    await waitFor(() => expect(client.requests.at(-1)?.maxRows).toBe(SERVER_MAX_ROWS));
    // The status line and the result footer can both say it.
    expect((await screen.findAllByText(/5,000 rows/)).length).toBeGreaterThan(0);
  });

  it('refused_statement_shows_clickhouse_code', async () => {
    mount();
    await screen.findByLabelText('SQL');
    setSql("SELECT * FROM default.events INTO OUTFILE 'out.csv'");
    run();
    const alert = await screen.findByTestId('house-failure');
    expect(within(alert).getByText(/^Code 344 · SUPPORT_IS_DISABLED · /)).toBeTruthy();
    // The shared editor marks itself invalid after the render that shows the
    // error; wait for it (it raced on a busy CI runner).
    await waitFor(() => expect(sqlBox().getAttribute('aria-invalid')).toBe('true'));
  });

  it('write_requires_confirmation', async () => {
    const client = mount();
    await screen.findByLabelText('SQL');
    setSql('INSERT INTO default.events VALUES (7)');
    run();
    const refused = await screen.findByTestId('house-failure');
    expect(within(refused).getByText(/Allow writes/)).toBeTruthy();
    expect(client.requests.every((r) => r.readOnly)).toBe(true);

    fireEvent.click(screen.getByLabelText('Allow writes'));
    run();
    const dialog = await screen.findByRole('dialog', {
      name: 'Run a statement that changes data?',
    });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Run' }));
    await waitFor(() => expect(client.requests.at(-1)?.readOnly).toBe(false));
  });

  it('disallowed_setting_is_flagged_and_not_sent', async () => {
    const client = mount();
    await screen.findByLabelText('SQL');
    fireEvent.click(screen.getByRole('button', { name: 'Add setting' }));
    fireEvent.change(screen.getByLabelText('Setting 1 name'), {
      target: { value: 'user_files_path' },
    });
    fireEvent.change(screen.getByLabelText('Setting 1 value'), { target: { value: '/x' } });
    expect(screen.getByText(/cannot be changed on the House/)).toBeTruthy();
    setSql('SELECT 1');
    run();
    expect(await screen.findByTestId('house-failure')).toBeTruthy();
    // Not a count: the page's own mount-time queries may still be landing.
    expect(client.requests.some((r) => JSON.stringify(r).includes('user_files_path'))).toBe(false);
  });

  it('history_loads_into_editor', async () => {
    mount();
    await screen.findByLabelText('SQL');
    setSql('');
    const list = await screen.findByRole('list', { name: 'Past queries' });
    const open = await within(list).findAllByRole('button', { name: /^Open in editor/ });
    // Scope MINE: a teammate's query is not listed.
    expect(within(list).queryByText('SELECT count() FROM analytics.daily')).toBeNull();
    fireEvent.click(open.at(-1) as HTMLElement);
    await waitFor(() => expect(codeEditorText(sqlBox())).toMatch(/GROUP BY kind/));
  });

  it('examples_come_from_the_schema_not_a_fixed_table', async () => {
    mount();
    await screen.findByLabelText('SQL');
    expect(codeEditorText(sqlBox())).toBe('');
    const examples = await screen.findByRole('region', { name: 'Example queries' });
    const first = await within(examples).findByRole('button', { name: /Rows of events/ });
    fireEvent.click(first);
    expect(await screen.findByRole('region', { name: 'Result rows' })).toBeTruthy();
  });

  it('syntax_error_marks_the_span', async () => {
    mount();
    await screen.findByLabelText('SQL');
    setSql('SELEC 1');
    run();
    await screen.findByTestId('house-failure');
    await waitFor(() => expect(document.querySelector('[data-error]')?.textContent).toBe('SELEC'));
  });

  it('cancel_stops_the_run', async () => {
    const client = mount({ delayMs: 20, batchRows: 10 });
    await screen.findByLabelText('SQL');
    setSql('SELECT number FROM numbers(1000)');
    run();
    fireEvent.click(await screen.findByRole('button', { name: 'Cancel' }));
    expect(await screen.findByText(/Cancelled/)).toBeTruthy();
    expect(client.cancelled.length).toBe(1);
  });
});

describe('databases and history on a partial House', () => {
  const unimplemented = (what: string) =>
    new HouseError({
      code: 'unimplemented',
      message: `${what} is served from a later task`,
      reason: 'not_implemented',
      metadata: {},
    });

  afterEach(() => localStorage.clear());

  it('an_unimplemented_history_is_a_notice_not_an_error', async () => {
    const client = createMockHouseClient({ delayMs: 0 });
    client.listQueryHistory = async () => {
      throw unimplemented('The query log');
    };
    render(<HousePage client={client} />);
    await screen.findByLabelText('SQL');
    expect(await screen.findByText('Query history is not available on this server.')).toBeTruthy();
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByText(/later task/)).toBeNull();
  });

  it('databases_come_from_system_databases_without_list_databases', async () => {
    const client = createMockHouseClient({ delayMs: 0 });
    client.listDatabases = async () => {
      throw unimplemented('ListDatabases');
    };
    render(<HousePage client={client} />);
    const picker = (await screen.findByLabelText('Database')) as HTMLSelectElement;
    const names = [...picker.options].map((o) => o.value);
    expect(names).toEqual(['analytics', 'default']);
    expect(picker.value).toBe('default');
    expect(screen.queryByText('Analytics — coming soon in a later preview')).toBeNull();
  });

  it('the_chosen_database_is_remembered_per_server', async () => {
    const first = render(
      <HousePage client={createMockHouseClient({ delayMs: 0 })} serverKey="https://a" />,
    );
    const picker = (await screen.findByLabelText('Database')) as HTMLSelectElement;
    fireEvent.change(picker, { target: { value: 'analytics' } });
    expect(await screen.findByText('Tables in analytics')).toBeTruthy();
    first.unmount();

    const again = render(
      <HousePage client={createMockHouseClient({ delayMs: 0 })} serverKey="https://a" />,
    );
    await waitFor(() =>
      expect((screen.getByLabelText('Database') as HTMLSelectElement).value).toBe('analytics'),
    );
    expect(screen.getByText('Tables in analytics')).toBeTruthy();
    again.unmount();

    render(<HousePage client={createMockHouseClient({ delayMs: 0 })} serverKey="https://b" />);
    await waitFor(() =>
      expect((screen.getByLabelText('Database') as HTMLSelectElement).value).toBe('default'),
    );
  });
});
