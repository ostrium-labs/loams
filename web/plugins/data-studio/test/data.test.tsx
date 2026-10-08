import { create } from '@bufbuild/protobuf';
import { Code, ConnectError, createRouterTransport } from '@connectrpc/connect';
import { validateManifest } from '@loams/console-host';
import { collection, documents, query } from '@loams/proto';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import pkg from '../package.json';
import { createDataClient } from '../src/client.js';
import { DataRoute } from '../src/index.js';
import { parseNdjson } from '../src/ingest-parse.js';
import { DocumentsTab } from '../src/pages/collection.js';
import { CollectionsPage } from '../src/pages/collections.js';
import { IngestTab } from '../src/pages/ingest.js';
import { SearchTab } from '../src/pages/search.js';
import { SqlPage } from '../src/pages/sql.js';

afterEach(cleanup);

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

  it('search_sends_ir_and_renders_scores', async () => {
    let got: query.SearchRequest | undefined;
    const { client } = clientWith({
      search: (req) => {
        got = req;
        return create(query.SearchResponseSchema, {
          hits: [{ pk: uid(9), score: 0.8125, source: { title: 'refund' } }],
        });
      },
    });
    render(
      <SearchTab
        client={client}
        ns="default"
        coll="articles"
        schema={{ vectors: [{ name: 'embedding', dim: 3 }] }}
      />,
    );
    fireEvent.change(screen.getByLabelText('Text'), { target: { value: 'refund' } });
    fireEvent.change(screen.getByLabelText('Filter'), {
      target: { value: '{"term":{"field":"tenant","value":"a"}}' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Search' }));
    expect((await screen.findByTestId('score')).textContent).toBe('0.8125');
    expect(got?.namespace).toBe('default');
    expect(got?.collection).toBe('articles');
    expect(got?.retrievers[0]?.retriever.case).toBe('text');
    expect(got?.filter?.query.case).toBe('term');
    expect(got?.limit).toBe(10);
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
    fireEvent.change(box, { target: { value: 'SELEKT' } });
    fireEvent.keyDown(box, { key: 'Enter', ctrlKey: true });
    expect((await screen.findByRole('alert')).textContent).toContain('parse error near FROM');
    const calls = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls as unknown as [
      string,
      RequestInit,
    ][];
    expect(calls[0]?.[0]).toBe('loams-app://console/v1/namespaces/default/sql');
    expect(JSON.parse(String(calls[0]?.[1].body))).toEqual({ query: 'SELEKT' });
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

  it('manifest_is_valid', () => {
    expect(validateManifest(pkg).editions).toEqual(['desktop']);
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
