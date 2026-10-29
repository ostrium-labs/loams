// The Analytics page's Connect client (src/connect.ts) against an in-memory
// loams.house.v1 server: detection from GetInstance.services[], JSON_ROWS decoding,
// and refusals carried through as HouseError with the ClickHouse code.

import { create } from '@bufbuild/protobuf';
import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import { Code, ConnectError, createRouterTransport } from '@connectrpc/connect';
import { errors, house, instance } from '@loams/proto';
import { setCodeEditorText } from '@loams/ui';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import type { ExecuteQueryRequest, QueryFrame } from '../src/client.js';
import { HouseError } from '../src/client.js';
import { createHouseClient, decodeRows } from '../src/connect.js';
import { HousePage } from '../src/HousePage.js';

afterEach(cleanup);

// jsdom has no layout: the code editor measures text ranges for its selection and tooltips.
Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect ??= () => new DOMRect();

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const enc = (s: string) => new TextEncoder().encode(s);

function server(opts: { listed?: boolean; seen?: house.ExecuteQueryRequest[] } = {}) {
  return createRouterTransport(({ service }) => {
    service(instance.InstanceService, {
      getInstance: () =>
        create(instance.GetInstanceResponseSchema, {
          services:
            opts.listed === false
              ? []
              : [{ package: 'loams.house.v1', version: 'v1', available: true }],
        }),
    });
    service(house.HouseService, {
      async *executeQuery(req) {
        opts.seen?.push(req);
        if (req.sql.includes('nope')) {
          const info = create(errors.ErrorInfoSchema, {
            reason: 'house_unknown_table',
            metadata: { clickhouse_code: '60', clickhouse_name: 'UNKNOWN_TABLE' },
          });
          throw new ConnectError('Table default.nope does not exist', Code.NotFound, undefined, [
            { desc: errors.ErrorInfoSchema, value: info },
          ]);
        }
        yield {
          frame: {
            case: 'header',
            value: { columns: [{ name: 'n', clickhouseType: 'UInt64' }], queryId: 'q1' },
          },
        };
        yield {
          frame: {
            case: 'batch',
            value: { data: { case: 'rowsJson', value: enc('["1"]\n["2"]\n') } },
          },
        };
        yield {
          frame: {
            case: 'summary',
            value: { resultRows: 2n, queryId: 'q1', snapshotIds: { 'default.events': 7n } },
          },
        };
      },
      listQueryHistory: () => ({
        queries: [
          {
            queryId: 'q1',
            principal: 'usr_1',
            startedAt: timestampFromDate(new Date('2026-10-11T00:00:00Z')),
            status: house.QueryStatus.FAILED,
            errorCode: 60,
            query: 'SELECT 1',
            durationMs: 5n,
          },
        ],
        nextPageToken: '',
      }),
    });
  });
}

const REQ: ExecuteQueryRequest = {
  sql: 'SELECT number AS n FROM numbers(2)',
  database: '',
  settings: {},
  params: {},
  format: 'JSON_ROWS',
  maxRows: 1000,
  timeoutMs: 30_000,
  readOnly: true,
};

async function collect(it: AsyncIterable<QueryFrame>) {
  const out: QueryFrame[] = [];
  for await (const f of it) out.push(f);
  return out;
}

describe('createHouseClient', () => {
  it('detects the House from GetInstance.services[]', async () => {
    expect(await createHouseClient(server()).available()).toBe(true);
    expect(await createHouseClient(server({ listed: false })).available()).toBe(false);
  });

  it('asks for JSON rows and decodes header, batch and summary frames', async () => {
    const seen: house.ExecuteQueryRequest[] = [];
    const frames = await collect(createHouseClient(server({ seen })).executeQuery(REQ));
    expect(seen[0]?.format).toBe(house.ResultFormat.JSON_ROWS);
    expect(seen[0]?.readOnly).toBe(true);
    expect(frames).toEqual([
      { kind: 'header', columns: [{ name: 'n', clickhouseType: 'UInt64' }] },
      { kind: 'batch', rows: [['1'], ['2']] },
      {
        kind: 'summary',
        summary: expect.objectContaining({
          resultRows: 2,
          queryId: 'q1',
          snapshotIds: ['default.events@7'],
        }),
      },
    ]);
  });

  it('turns a refusal into a HouseError with the ClickHouse code', async () => {
    const run = collect(
      createHouseClient(server()).executeQuery({ ...REQ, sql: 'SELECT * FROM nope' }),
    );
    await expect(run).rejects.toBeInstanceOf(HouseError);
    const failure = await run.catch((e: HouseError) => e.failure);
    expect(failure).toMatchObject({
      code: 'not_found',
      reason: 'house_unknown_table',
      clickhouseCode: 60,
      clickhouseName: 'UNKNOWN_TABLE',
    });
  });

  it('maps history records', async () => {
    const page = await createHouseClient(server()).listQueryHistory({ scope: 'MINE' });
    expect(page.items[0]).toMatchObject({
      queryId: 'q1',
      status: 'FAILED',
      errorCode: 60,
      durationMs: 5,
      startedAt: '2026-10-11T00:00:00.000Z',
    });
  });

  it('decodes JSONCompactEachRow lines and skips blank ones', () => {
    expect(decodeRows(enc('[1,"a"]\n\n[null,[2]]'))).toEqual([
      [1, 'a'],
      [null, [2]],
    ]);
  });

  it('drives the page against the served House, without the sample notice', async () => {
    render(<HousePage client={createHouseClient(server())} />);
    await waitFor(() => screen.getByLabelText('SQL'));
    expect(screen.queryByText('Sample data')).toBeNull();
    act(() => setCodeEditorText(screen.getByLabelText('SQL'), REQ.sql));
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    await waitFor(() => expect(screen.getAllByText('2').length).toBeGreaterThan(0));
  });
});
