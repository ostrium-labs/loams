import { create, type JsonObject } from '@bufbuild/protobuf';
import { Code, ConnectError, createRouterTransport } from '@connectrpc/connect';
import { validateManifest } from '@loams/console-host';
import { collection, documents, query } from '@loams/proto';
import { setCodeEditorText } from '@loams/ui';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import pkg from '../package.json';
import { createDataClient, toDocumentId } from '../src/client.js';
import plugin, { DataRoute } from '../src/index.js';
import { parseNdjson } from '../src/ingest-parse.js';
import { DocumentsTab } from '../src/pages/collection.js';
import { CollectionsPage } from '../src/pages/collections.js';
import { IngestTab } from '../src/pages/ingest.js';
import { SearchTab } from '../src/pages/search.js';
import { SqlPage } from '../src/pages/sql.js';

afterEach(cleanup);

// jsdom has no layout: the code editor measures text ranges for its selection and tooltips.
Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect ??= () => new DOMRect();

// The empty state's grain canvas observes its size; jsdom has no ResizeObserver.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const uid = (n: number) =>
  create(documents.DocumentIdSchema, { id: { case: 'uint', value: BigInt(n) } });
const noFetch = (async () => {
  throw new Error('no fetch');
}) as unknown as typeof fetch;

interface Handlers {
  scroll?: (req: documents.ScrollDocumentsRequest) => documents.ScrollDocumentsResponse;
  write?: (req: documents.WriteDocumentsRequest) => documents.WriteDocumentsResponse;
  search?: (req: query.SearchRequest) => query.SearchResponse;
}

function clientWith(h: Handlers = {}, fetch: typeof globalThis.fetch = noFetch) {
  const transport = createRouterTransport(({ service }) => {
    service(collection.CollectionService, {
      listCollections: () =>
        create(collection.ListCollectionsResponseSchema, {
          collections: [
            { name: 'articles', namespace: 'default', liveDocCount: 42n, manifestVersion: 7n },
            { name: 'logs', namespace: 'default', liveDocCount: 3n, manifestVersion: 1n },
          ],
        }),
    });
    service(documents.DocumentService, {
      scrollDocuments: (req) =>
        h.scroll?.(req) ?? create(documents.ScrollDocumentsResponseSchema, {}),
      writeDocuments: (req) =>
        h.write?.(req) ?? create(documents.WriteDocumentsResponseSchema, { token: 't' }),
    });
    service(query.QueryService, {
      search: (req) => h.search?.(req) ?? create(query.SearchResponseSchema, {}),
    });
  });
  return { client: createDataClient(transport, fetch, 'loams-app://console'), transport };
}

describe('data studio', () => {
  it('lists_collections', async () => {
    const { client } = clientWith();
    const nav = vi.fn();
    render(<CollectionsPage client={client} ns="default" navigate={nav} />);
    await screen.findByText('articles');
    expect(screen.getByText('42')).toBeTruthy();
    expect(screen.getByText('logs')).toBeTruthy();
    expect(screen.getByText('articles').closest('a')?.getAttribute('href')).toBe(
      '#/data/default/articles',
    );
    expect(screen.getByRole('button', { name: 'New collection' })).toBeTruthy();
    expect(nav).not.toHaveBeenCalled();
  });

  it('pages_with_page_token', async () => {
    const seen: (string | undefined)[] = [];
    const { client } = clientWith({
      scroll: (req) => {
        const tok =
          req.pageToken?.id.case === 'uint' ? req.pageToken.id.value.toString() : undefined;
        seen.push(tok);
        return tok === undefined
          ? create(documents.ScrollDocumentsResponseSchema, {
              documents: [{ id: uid(1), source: { title: 'first' } }],
              next: uid(1),
            })
          : create(documents.ScrollDocumentsResponseSchema, {
              documents: [{ id: uid(2), source: { title: 'second' } }],
            });
      },
    });
    render(<DocumentsTab client={client} ns="default" coll="articles" />);
    await screen.findByText('first');
    fireEvent.click(screen.getByRole('button', { name: 'Next' }));
    await screen.findByText('second');
    expect(seen).toEqual([undefined, '1']);
    expect((screen.getByRole('button', { name: 'Next' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Previous' }));
    await screen.findByText('first');
  });

  const SEARCH_SCHEMA: JsonObject = {
    fields: [
      { name: 'title', kind: { text: { analyzer: 'standard' } }, indexed: true },
      { name: 'tenant', kind: 'keyword', indexed: true, fast: true },
    ],
    vectors: [{ name: 'embedding', dim: 3 }],
  };

  it('search_sends_ir_and_renders_scores', async () => {
    const got: query.SearchRequest[] = [];
    const onParams = vi.fn();
    const { client } = clientWith({
      search: (req) => {
        got.push(req);
        return create(query.SearchResponseSchema, {
          hits: [
            {
              pk: uid(9),
              score: 0.8125,
              source: { title: 'refund policy', tenant: 'a' },
              highlight: {
                title: {
                  kind: {
                    case: 'listValue',
                    value: {
                      values: [
                        { kind: { case: 'stringValue', value: '\uE000refund\uE001 policy' } },
                      ],
                    },
                  },
                },
              },
            },
          ],
          total: { value: 1n, relation: query.TotalRelation.EQ },
        });
      },
    });
    render(
      <SearchTab
        client={client}
        ns="default"
        coll="articles"
        schema={SEARCH_SCHEMA}
        onParams={onParams}
      />,
    );
    // The first search runs on its own: everything, filter only.
    await waitFor(() => expect(got).toHaveLength(1));
    const box = screen.getByRole('combobox', { name: 'Search query' });
    fireEvent.change(box, { target: { value: 'refund tenant:a' } });
    fireEvent.keyDown(box, { key: 'Enter' });
    await waitFor(() => expect(got).toHaveLength(2));
    expect((await screen.findByTestId('score')).textContent).toBe('0.8125');
    const req = got[1] as query.SearchRequest;
    expect(req.namespace).toBe('default');
    expect(req.collection).toBe('articles');
    const text = req.retrievers[0]?.retriever;
    expect(text?.case).toBe('text');
    const q = text?.case === 'text' ? text.value.query?.query : undefined;
    expect(q?.case === 'match' && q.value.field).toBe('title');
    expect(req.filter?.query.case).toBe('term');
    expect(req.limit).toBe(20);
    expect(req.highlight?.fields.map((f) => f.field)).toEqual(['title']);
    // The engine's highlight is marked, and the URL carries the query.
    expect(screen.getByText('refund', { selector: 'mark' })).toBeTruthy();
    expect(screen.getByText('1', { selector: 'strong' }).parentElement?.textContent).toContain(
      '1 hit',
    );
    expect(onParams).toHaveBeenLastCalledWith('q=refund+tenant%3Aa');
  });

  it('search_shows_query_errors_without_sending', async () => {
    const search = vi.fn(() => create(query.SearchResponseSchema, {}));
    const { client } = clientWith({ search });
    render(<SearchTab client={client} ns="default" coll="articles" schema={SEARCH_SCHEMA} />);
    await waitFor(() => expect(search).toHaveBeenCalledTimes(1));
    const box = screen.getByRole('combobox', { name: 'Search query' });
    fireEvent.change(box, { target: { value: 'tenat:a' } });
    fireEvent.keyDown(box, { key: 'Enter' });
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('Did you mean tenant?');
    expect(alert.querySelector('mark')?.textContent).toBe('tenat');
    expect(box.getAttribute('aria-invalid')).toBe('true');
    expect(search).toHaveBeenCalledTimes(1);
  });

  it('search_suggests_fields_and_takes_the_keyboard', async () => {
    const { client } = clientWith();
    render(<SearchTab client={client} ns="default" coll="articles" schema={SEARCH_SCHEMA} />);
    const box = screen.getByRole('combobox', { name: 'Search query' }) as HTMLInputElement;
    fireEvent.focus(box);
    fireEvent.change(box, { target: { value: 'ten', selectionStart: 3 } });
    const list = await screen.findByRole('listbox');
    expect(
      within(list)
        .getAllByRole('option')
        .map((o) => o.textContent),
    ).toEqual([expect.stringContaining('tenant')]);
    fireEvent.keyDown(box, { key: 'ArrowDown' });
    expect(box.getAttribute('aria-activedescendant')).toBeTruthy();
    fireEvent.keyDown(box, { key: 'Enter' });
    expect(box.value).toBe('tenant ');
    fireEvent.keyDown(box, { key: 'Escape' });
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('search_state_comes_from_the_url', async () => {
    const got: query.SearchRequest[] = [];
    const { client } = clientWith({
      search: (req) => {
        got.push(req);
        return create(query.SearchResponseSchema, {});
      },
    });
    render(
      <SearchTab
        client={client}
        ns="default"
        coll="articles"
        schema={SEARCH_SCHEMA}
        params={{ q: 'refund', f: '[["tenant:b","n"]]', k: '50', mode: 'text' }}
      />,
    );
    await waitFor(() => expect(got).toHaveLength(1));
    const req = got[0] as query.SearchRequest;
    expect(req.limit).toBe(50);
    expect(req.filter?.query.case).toBe('bool');
    expect(screen.getByText('tenant:b')).toBeTruthy();
    expect(screen.getByText('NOT')).toBeTruthy();
  });

  it('sql_error_shown', async () => {
    const fetch = vi.fn(
      async () =>
        new Response(
          JSON.stringify({ error: 'invalid_argument', message: 'parse error near FROM' }),
          {
            status: 400,
          },
        ),
    ) as unknown as typeof globalThis.fetch;
    const { client } = clientWith({}, fetch);
    render(<SqlPage client={client} ns="default" navigate={() => {}} />);
    const box = screen.getByLabelText('SQL');
    act(() => setCodeEditorText(box, 'SELEKT'));
    fireEvent.keyDown(box, { key: 'Enter', ctrlKey: true });
    expect((await screen.findByRole('alert')).textContent).toContain('parse error near FROM');
    const calls = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls as unknown as [
      string,
      RequestInit,
    ][];
    // The page reads the schema first (information_schema), then runs the statement.
    const ran = calls.find((c) => JSON.parse(String(c[1].body)).query === 'SELEKT');
    expect(ran?.[0]).toBe('loams-app://console/v1/namespaces/default/sql');
  });

  it('sql_result_grid', async () => {
    const fetch = vi.fn(
      async () =>
        new Response(
          JSON.stringify({
            columns: [{ name: 'n', type: 'Int64' }],
            rows: [[1], [2]],
            truncated: false,
          }),
        ),
    ) as unknown as typeof globalThis.fetch;
    const { client } = clientWith({}, fetch);
    render(<SqlPage client={client} ns="default" navigate={() => {}} />);
    act(() => setCodeEditorText(screen.getByLabelText('SQL'), 'SELECT n FROM t'));
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    await screen.findByText('2 rows');
    expect(screen.getByText('Int64')).toBeTruthy();
  });

  it('ingest_batches_of_500_and_stops_on_error', async () => {
    const sizes: number[] = [];
    const keys = new Set<string>();
    const { client } = clientWith({
      write: (req) => {
        sizes.push(req.ops.length);
        keys.add(req.idempotencyKey);
        if (sizes.length === 2) throw new ConnectError('disk full', Code.Internal);
        return create(documents.WriteDocumentsResponseSchema, { token: 't' });
      },
    });
    render(<IngestTab client={client} ns="default" coll="articles" schema={{}} />);
    const docs = Array.from({ length: 1300 }, (_, i) => ({ id: i + 1, title: `t${i}` }));
    const file = new File([docs.map((d) => JSON.stringify(d)).join('\n')], 'docs.ndjson');
    fireEvent.change(screen.getByLabelText('File'), { target: { files: [file] } });
    await screen.findByText(/1300 documents in 3 batches/);
    fireEvent.click(screen.getByRole('button', { name: 'Start ingest' }));
    const alert = await screen.findByText(/Batch 2 failed/);
    expect(alert.textContent).toContain('disk full');
    expect(alert.textContent).toContain('500 of 1300');
    expect(sizes).toEqual([500, 500]);
    expect(keys.size).toBe(2);
  });

  it('ndjson_parse_reports_line_number_on_bad_line', async () => {
    expect(() => parseNdjson('{"id":1}\n\n{"id":2}\n{oops}\n')).toThrow(/Line 4/);
    expect(parseNdjson('{"id":1}\n{"id":2}\n')).toHaveLength(2);
    const { client } = clientWith();
    render(<IngestTab client={client} ns="default" coll="articles" schema={{}} />);
    const file = new File(['{"id":1}\nnot json\n'], 'x.ndjson');
    fireEvent.change(screen.getByLabelText('File'), { target: { files: [file] } });
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('Line 2'));
    expect(
      (screen.getByRole('button', { name: 'Start ingest' }) as HTMLButtonElement).disabled,
    ).toBe(true);
  });

  it('data_studio_renamed_label_only', () => {
    // Q812: "Data explorer" is the visible name; the route, page ids and plugin id stay.
    const pages: { id: string; path: string; title: string }[] = [];
    const nav: { name: string; plugin: string; meta?: Record<string, unknown> }[] = [];
    const ctx = {
      router: {
        page: (spec: (typeof pages)[number]) => {
          pages.push(spec);
          return () => {};
        },
        navigate() {},
      },
      slots: {
        register: (spec: (typeof nav)[number]) => {
          nav.push(spec);
          return () => {};
        },
      },
      transport: {},
      platform: { fetch: noFetch, baseUrl: 'loams-app://console' },
      flags: {},
      effect: (fn: () => unknown) => fn(),
    };
    plugin.apply(ctx as never, {});
    // UX-2: its SQL editor and the Search entry are Pages too, so each can be pinned.
    expect(nav.filter((n) => n.name === 'shell.nav.section')).toEqual([
      expect.objectContaining({
        plugin: 'data-studio',
        meta: expect.objectContaining({ id: 'data', label: 'Data explorer', href: '/data' }),
      }),
      expect.objectContaining({
        meta: expect.objectContaining({ id: 'data-sql', label: 'SQL', href: '/data/default/sql' }),
      }),
      expect.objectContaining({
        meta: expect.objectContaining({ id: 'search', label: 'Search', href: '/search' }),
      }),
    ]);
    expect(pages.find((p) => p.id === 'search')).toMatchObject({ path: '/search' });
    expect(pages.find((p) => p.id === 'data')).toMatchObject({
      path: '/data',
      title: 'Data explorer',
    });
    expect(pages.find((p) => p.id === 'data-ns')).toMatchObject({ path: '/data/:ns' });
  });

  it('manifest_is_valid', () => {
    expect(validateManifest(pkg).editions).toEqual(['desktop']);
    // UX-7: not gated on loams.collection.v1, so /data explains a server without a data
    // plane instead of "No plugin serves /data".
    expect(validateManifest(pkg).requires.api).toEqual([]);
  });

  it('leading_zero_string_id_stays_string', () => {
    expect(toDocumentId('007').id).toEqual({ case: 'string', value: '007' });
    expect(toDocumentId('42').id).toEqual({ case: 'string', value: '42' });
  });

  it('numeric_json_id_is_uint', () => {
    expect(toDocumentId(7).id).toEqual({ case: 'uint', value: 7n });
  });

  it('no_data_plane_empty_state', () => {
    const { transport } = clientWith();
    render(
      <DataRoute
        transport={transport}
        platform={{ fetch: noFetch, baseUrl: 'loams-app://console' }}
        flags={{ has: () => false }}
        navigate={() => {}}
        params={{}}
        view="collections"
      />,
    );
    expect(screen.getByText('This server has no data plane.')).toBeTruthy();
    expect(within(document.body).queryByText('New collection')).toBeNull();
  });
});
