// GR1 Task 8: the Graph page against the desktop contract fixtures of Task 7, through a
// fake Connect transport (test/fixtures.ts). Where a fixture's request is the page's own
// call, the page's request must equal it.

import { create, fromJson, type JsonObject } from '@bufbuild/protobuf';
import { validateManifest } from '@loams/console-host';
import { graph, instance } from '@loams/proto';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import pkg from '../package.json';
import { PAGE_MAX_ROWS, STREAM_MAX_ROWS, withProfile } from '../src/client.js';
import { graphAvailability } from '../src/detect.js';
import { spanRange } from '../src/Editor.js';
import { GraphPage } from '../src/GraphPage.js';
import { layoutGraph, MAX_DRAWN_NODES } from '../src/GraphView.js';
import { HISTORY_LIMIT, statementHistory } from '../src/history.js';
import { collectElements, parametersFromJson, valueText, valueType } from '../src/values.js';
import { type Call, type FakeOptions, FIXTURES, fakeServer, memoryStorage } from './fixtures.js';

afterEach(cleanup);

// The empty state's grain canvas observes its size; jsdom has no ResizeObserver.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};
// jsdom has no modal <dialog>: open it by attribute.
HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};

function mount(opts: FakeOptions = {}) {
  const server = fakeServer(opts);
  const storage = memoryStorage();
  const openExternal = vi.fn();
  render(<GraphPage transport={server.transport} openExternal={openExternal} storage={storage} />);
  return { server, storage, openExternal };
}

async function openMovies(opts: FakeOptions = {}) {
  const m = mount(opts);
  fireEvent.click(await screen.findByRole('button', { name: /^movies/ }));
  await screen.findByLabelText('Statement');
  return m;
}

const statementBox = () => screen.getByLabelText('Statement') as HTMLTextAreaElement;
const setStatement = (text: string) =>
  fireEvent.change(statementBox(), { target: { value: text } });
const run = () => fireEvent.click(screen.getByRole('button', { name: 'Run' }));

function last(calls: Call[], method: string): JsonObject {
  const c = calls.filter((x) => x.method.endsWith(`/${method}`)).at(-1);
  if (!c) throw new Error(`no ${method} call`);
  return c.request;
}

const fixtureStatement = (ex: { request: JsonObject }) =>
  (ex.request.statement ?? (ex.request.request as JsonObject).statement) as string;

function rowsResponse(columns: string[], rows: graph.Value[][]): graph.ExecuteResponse {
  return create(graph.ExecuteResponseSchema, {
    rows: {
      columns,
      columnTypes: columns.map(() => 'ANY'),
      rows: rows.map((values) => ({ values })),
    },
  });
}

const node = (id: number, label: string, name: string) =>
  create(graph.ValueSchema, {
    kind: {
      case: 'node',
      value: {
        id: BigInt(id),
        labels: [label],
        properties: { name: { kind: { case: 'string', value: name } } },
      },
    },
  });

describe('detect', () => {
  it('availability_states_render', async () => {
    const empty = { services: [] };
    expect(graphAvailability(empty)).toBe('absent');
    const listed = fromJson(instance.GetInstanceResponseSchema, FIXTURES.instance.response ?? {});
    expect(graphAvailability(listed)).toBe('available');
    for (const s of listed.services) s.available = false;
    expect(graphAvailability(listed)).toBe('not_in_variant');

    mount({ availability: 'absent' });
    expect(await screen.findByText('Graph is not served by this server')).toBeTruthy();
    cleanup();

    const { openExternal } = mount({ availability: 'not_in_variant' });
    expect(await screen.findByText("Graph is not in this server's variant")).toBeTruthy();
    expect(screen.getByText(/runs the standard variant/)).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Read the docs' }));
    expect(openExternal).toHaveBeenCalledWith('https://loams.dev/docs');
    cleanup();

    mount();
    expect(await screen.findByRole('button', { name: /^movies/ })).toBeTruthy();
    expect(screen.queryByText(/not served|variant/)).toBeNull();
  });
});

describe('graph page', () => {
  it('list_and_create_graph', async () => {
    const { server } = mount();
    const movies = await screen.findByRole('button', { name: /^movies/ });
    expect(last(server.calls, 'ListGraphs')).toEqual(FIXTURES.listGraphs.request);
    expect(within(movies).getByText('5 nodes · 4 relationships')).toBeTruthy();
    const kg = screen.getByRole('button', { name: /^kg/ });
    expect(within(kg).getByText('linked')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'New graph' }));
    const dialog = screen.getByRole('dialog', { name: 'New graph' });
    const create = within(dialog).getByRole('button', { name: 'Create' }) as HTMLButtonElement;
    fireEvent.change(within(dialog).getByLabelText('Name'), { target: { value: 'Bad Name' } });
    expect(create.disabled).toBe(true);
    expect(within(dialog).getByText('Not a valid graph name.')).toBeTruthy();
    fireEvent.change(within(dialog).getByLabelText('Name'), { target: { value: 'people' } });
    fireEvent.click(within(dialog).getByLabelText('Also accept Cypher'));
    fireEvent.click(create);

    await screen.findByRole('button', { name: /^people/, current: true });
    const req = last(server.calls, 'CreateGraph');
    expect(req).toMatchObject({
      namespace: 'default',
      name: 'people',
      mode: 'GRAPH_MODE_OWNED',
      languages: ['QUERY_LANGUAGE_GQL', 'QUERY_LANGUAGE_CYPHER'],
    });
    expect(typeof req.idempotencyKey).toBe('string');
    // A graph that accepts Cypher offers it in the editor.
    expect(screen.getByRole('option', { name: 'Cypher' })).toBeTruthy();
  });

  it('run_renders_table_with_typed_cells', async () => {
    const { server } = await openMovies();
    setStatement(fixtureStatement(FIXTURES.executeTable));
    run();
    await screen.findByText('Carrie-Anne Moss');
    expect(last(server.calls, 'Execute')).toEqual(FIXTURES.executeTable.request);

    const table = screen.getByRole('table');
    expect(within(table).getAllByRole('row')).toHaveLength(4);
    const released = within(table).getAllByText('1999');
    expect(released[0]?.getAttribute('data-type')).toBe('INT64');
    expect(within(table).getAllByText('8.7')[0]?.getAttribute('data-type')).toBe('FLOAT64');
    expect(within(table).getByText("['Trinity']").getAttribute('data-type')).toBe('LIST');
    expect(screen.getByText(/^3 rows/)).toBeTruthy();

    // INT64 is a bigint and prints exactly, past 2^53.
    cleanup();
    await openMovies({
      execute: () =>
        rowsResponse(
          ['big'],
          [[create(graph.ValueSchema, { kind: { case: 'int64', value: 9007199254740993n } })]],
        ),
    });
    setStatement('RETURN 9007199254740993 AS big');
    run();
    const big = await screen.findByText('9007199254740993');
    expect(big.getAttribute('data-type')).toBe('INT64');
  });

  it('values_fixture_formats_every_case', () => {
    const shown = new Map(
      FIXTURES.values.map((c) => {
        const v = fromJson(graph.ValueSchema, c.value);
        return [c.name, `${valueType(v)} ${valueText(v)}`];
      }),
    );
    expect(shown.size).toBe(FIXTURES.values.length);
    expect(shown.get('int64_max')).toBe('INT64 9223372036854775807');
    expect(shown.get('int64_2_53_plus_1')).toBe('INT64 9007199254740993');
    expect(shown.get('float64_nan')).toBe('FLOAT64 NaN');
    expect(shown.get('date')).toBe('DATE 2026-10-08');
    for (const [name, text] of shown) expect(text, name).not.toMatch(/undefined|\[object/);
  });

  it('run_renders_graph_view_capped_at_500', async () => {
    const { server } = await openMovies();
    setStatement(fixtureStatement(FIXTURES.executeGraph));
    run();
    fireEvent.click(await screen.findByRole('tab', { name: 'Graph' }));
    expect(last(server.calls, 'Execute')).toEqual(FIXTURES.executeGraph.request);
    const view = screen.getByTestId('graph-view');
    // Keanu Reeves has id 0, which proto3 JSON leaves out: he is still node "0".
    const ids = [...view.querySelectorAll('[data-node]')].map((n) => n.getAttribute('data-node'));
    expect(ids.sort()).toEqual(['0', '1', '3']);
    expect(view.querySelectorAll('[data-edge]')).toHaveLength(2);
    fireEvent.click(screen.getByRole('button', { name: /Keanu Reeves/ }));
    const details = screen.getByRole('complementary', { name: 'Selected node' });
    expect(within(details).getByText('1964')).toBeTruthy();

    cleanup();
    const many = Array.from({ length: 600 }, (_, i) => [node(i, 'N', `n${i}`)]);
    await openMovies({ execute: () => rowsResponse(['n'], many) });
    setStatement('MATCH (n:N) RETURN n');
    run();
    fireEvent.click(await screen.findByRole('tab', { name: 'Graph' }));
    expect(screen.getByText('Showing 500 of 600 nodes. The rest are in the table.')).toBeTruthy();
    expect(screen.getByTestId('graph-view').querySelectorAll('[data-node]')).toHaveLength(
      MAX_DRAWN_NODES,
    );

    // The layout is seeded: the same result draws the same picture.
    const elements = collectElements(
      many.slice(0, 40).map((values) => create(graph.RowSchema, { values })),
    );
    const a = layoutGraph(elements).nodes.map((d) => [d.x, d.y]);
    const b = layoutGraph(elements).nodes.map((d) => [d.x, d.y]);
    expect(a).toEqual(b);
  });

  it('order_by_relationship_as_int64_renders', async () => {
    // Grafeo 0.5.43 answers a relationship as INT64 0 under ORDER BY (R7.6).
    await openMovies({
      execute: () =>
        rowsResponse(['r'], [[create(graph.ValueSchema, { kind: { case: 'int64', value: 0n } })]]),
    });
    setStatement('MATCH (a)-[r]->(m) RETURN r ORDER BY a.name');
    run();
    const cell = await screen.findByText('0', { selector: '[data-type]' });
    expect(cell.getAttribute('data-type')).toBe('INT64');
    expect(screen.queryByRole('tab', { name: 'Graph' })).toBeNull();
  });

  it('truncated_banner_offers_stream_all', async () => {
    const [unary, stream] = FIXTURES.truncated;
    if (!unary || !stream) throw new Error('fixture');
    const { server } = await openMovies();
    setStatement(fixtureStatement(unary));
    run();
    const banner = await screen.findByTestId('truncated-banner');
    expect(within(banner).getByText(/Showing the first 2 rows/)).toBeTruthy();
    expect(last(server.calls, 'Execute')).toEqual({ ...unary.request, maxRows: PAGE_MAX_ROWS });

    fireEvent.click(within(banner).getByRole('button', { name: 'Stream all' }));
    await screen.findByText('Lana Wachowski');
    expect(last(server.calls, 'ExecuteStream')).toEqual({
      request: { ...(stream.request.request as JsonObject), maxRows: STREAM_MAX_ROWS },
    });
    expect(screen.getAllByRole('row')).toHaveLength(4);
    expect(screen.queryByTestId('truncated-banner')).toBeNull();
  });

  it('syntax_error_underlines_position', async () => {
    const { server } = await openMovies();
    const text = fixtureStatement(FIXTURES.errorSyntax);
    setStatement(text);
    run();
    const alert = await screen.findByRole('alert');
    expect(last(server.calls, 'Execute')).toEqual(FIXTURES.errorSyntax.request);
    expect(
      within(alert).getByText('GQLSTATUS 42001 · gql_syntax_error · line 2, column 17'),
    ).toBeTruthy();
    const mark = within(screen.getByTestId('error-span')).getByText('RETURN');
    expect(mark.tagName).toBe('MARK');
    const box = statementBox();
    expect(box.getAttribute('aria-invalid')).toBe('true');
    expect(box.value.slice(box.selectionStart, box.selectionEnd)).toBe('RETURN');
    // Columns count characters, not UTF-16 units.
    expect(spanRange('é\n𝔾 X', { line: 2, column: 3, length: 1 })).toEqual([5, 6]);
  });

  it('read_only_toggle_default_on', async () => {
    const executes: graph.ExecuteRequest[] = [];
    const { server } = await openMovies({
      execute: (req) => {
        executes.push(req);
        if (req.readOnly) return undefined; // the fixture's refusal
        return create(graph.ExecuteResponseSchema, {
          rows: { columns: [], rows: [] },
          counters: { nodesCreated: 1n, propertiesSet: 1n },
        });
      },
    });
    const toggle = screen.getByLabelText('Read-only') as HTMLInputElement;
    expect(toggle.checked).toBe(true);
    setStatement(fixtureStatement(FIXTURES.errorDenied));
    run();
    const alert = await screen.findByRole('alert');
    expect(within(alert).getByText(/graph_read_only/)).toBeTruthy();
    expect(last(server.calls, 'Execute')).toEqual(FIXTURES.errorDenied.request);
    // A refused write is not an admin refusal: the admin actions stay.
    expect(screen.getByRole('button', { name: 'New graph' })).toBeTruthy();

    fireEvent.click(toggle);
    run();
    await screen.findByText(/1 nodes created, 1 properties set/);
    expect(executes.map((r) => r.readOnly)).toEqual([true, false]);
  });

  it('history_never_stores_parameters', async () => {
    const { server, storage } = await openMovies({
      execute: () => rowsResponse(['p'], []),
    });
    const statement = 'MATCH (p:Person {name: $who}) RETURN p';
    setStatement(statement);
    fireEvent.change(screen.getByLabelText('Parameters (JSON object)'), {
      target: { value: '{"who": "hunter2-secret", "n": 7}' },
    });
    run();
    await screen.findByText('No rows.');
    expect(last(server.calls, 'Execute').parameters).toEqual({
      who: { string: 'hunter2-secret' },
      n: { int64: '7' },
    });
    const kept = [...Array(storage.length).keys()].map((i) =>
      storage.getItem(storage.key(i) ?? ''),
    );
    expect(kept.join()).toContain(statement);
    expect(kept.join()).not.toContain('hunter2');
    expect(screen.getByRole('option', { name: statement })).toBeTruthy();

    // 100 statements per server, newest first; servers do not share.
    const h = statementHistory('server-a', storage);
    for (let i = 0; i < HISTORY_LIMIT + 20; i++) h.add(`RETURN ${i}`);
    expect(h.list()).toHaveLength(HISTORY_LIMIT);
    expect(h.list()[0]).toBe(`RETURN ${HISTORY_LIMIT + 19}`);
    expect(statementHistory('server-b', storage).list()).toEqual([]);
    expect(() => parametersFromJson('[1]')).toThrow(/JSON object/);
  });

  it('delete_requires_typed_name', async () => {
    const { server } = mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Delete movies' }));
    const dialog = screen.getByRole('dialog', { name: 'Delete movies' });
    const del = within(dialog).getByRole('button', { name: 'Delete graph' }) as HTMLButtonElement;
    const input = within(dialog).getByLabelText('Graph name');
    expect(del.disabled).toBe(true);
    fireEvent.change(input, { target: { value: 'movie' } });
    expect(del.disabled).toBe(true);
    fireEvent.change(input, { target: { value: 'movies' } });
    expect(del.disabled).toBe(false);
    fireEvent.click(del);
    await waitFor(() => expect(screen.queryByRole('button', { name: /^movies/ })).toBeNull());
    expect(last(server.calls, 'DeleteGraph')).toMatchObject({
      namespace: 'default',
      name: 'movies',
    });
  });

  it('permission_denied_hides_admin_actions', async () => {
    const { server } = mount({ denyAdmin: true });
    await screen.findByRole('button', { name: /^movies/ });
    expect(screen.getByRole('button', { name: 'Delete movies' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'New graph' }));
    const dialog = screen.getByRole('dialog', { name: 'New graph' });
    fireEvent.change(within(dialog).getByLabelText('Name'), { target: { value: 'people' } });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Create' }));
    await screen.findByText(/You cannot create or delete graphs here/);
    expect(server.methods()).toContain('CreateGraph');
    expect(screen.queryByRole('button', { name: 'New graph' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Delete movies' })).toBeNull();
    // Reading still works.
    expect(screen.getByRole('button', { name: /^movies/ })).toBeTruthy();
  });

  it('explain_and_profile_render_plan', async () => {
    const [plain, profiled] = FIXTURES.explain;
    if (!plain || !profiled) throw new Error('fixture');
    const { server } = await openMovies();
    setStatement(fixtureStatement(plain));
    fireEvent.click(screen.getByRole('button', { name: 'Explain' }));
    await screen.findByText('Sort (title ASC)');
    expect(last(server.calls, 'Explain')).toEqual(plain.request);

    fireEvent.click(screen.getByRole('button', { name: 'Profile' }));
    await screen.findByRole('region', { name: 'Profile' });
    expect(last(server.calls, 'Explain')).toEqual(profiled.request);
    expect(screen.getAllByText(/^2 rows/).length).toBeGreaterThan(0);
    expect(withProfile('profile MATCH (n) RETURN n')).toBe('profile MATCH (n) RETURN n');
  });

  it('schema_sidebar_lists_labels_and_types', async () => {
    const { server } = await openMovies();
    const schema = await screen.findByRole('complementary', { name: 'Schema' });
    await within(schema).findByText(':Person');
    expect(last(server.calls, 'GetSchema')).toEqual(FIXTURES.schema.request);
    expect(within(schema).getByText(':ACTED_IN')).toBeTruthy();
    expect(within(schema).getByText('rating')).toBeTruthy();
    expect(within(schema).getByText('No indexes.')).toBeTruthy();
  });

  it('manifest_injects_transport', () => {
    const m = validateManifest(pkg);
    expect(m.inject).toContain('transport');
  });
});
