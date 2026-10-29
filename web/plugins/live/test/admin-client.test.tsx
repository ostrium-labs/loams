// createLiveAdminApi against an in-memory LiveAdminService shaped like the engine's
// (crates/loams-live/src/admin.rs): the wire enums, the Validator oneof, AIP-158
// pages, TailLogs batches with `skipped`, and the errors the views branch on.

import { create } from '@bufbuild/protobuf';
import { Code, ConnectError, createRouterTransport } from '@connectrpc/connect';
import { errors, liveAdmin, liveValue } from '@loams/proto';
import { cleanup, render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { DeployConflictError, isSkipped, type LogLine, type LogsSkipped } from '../src/admin.js';
import { ADMIN_UNSERVED, createLiveAdminApi, PAGE_SIZE } from '../src/admin-client.js';
import { DeploymentsPage, PREVIEW_NOTICE } from '../src/deployments.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const S = liveAdmin.DeploymentState;
const id = (n: number) => `01K7B${String(n).padStart(21, '0')}`;
const T0 = 1_760_000_000_000n;

/** `count` deployments, newest first; the newest is active, one is failed. */
function deployments(count: number) {
  return Array.from({ length: count }, (_, i) => {
    const n = count - i;
    return {
      id: id(n),
      bundleSha256: 'ab'.repeat(32),
      createdAtMs: T0 + BigInt(n),
      createdBy: 'apikey:k1',
      message: `deploy ${n}`,
      parent: n > 1 ? id(n - 1) : '',
      state: n === count ? S.ACTIVE : n === 2 ? S.FAILED : S.SUPERSEDED,
      failureReason: n === 2 ? 'live_schema_violation: 3 documents' : '',
    };
  });
}

const int64 = create(liveValue.ValueSchema, { kind: { case: 'stringValue', value: 'up' } });

function engine(opts: { count?: number; unserved?: boolean } = {}) {
  const all = deployments(opts.count ?? 3);
  let active = all[0]?.id;
  const pages: string[] = [];
  const transport = createRouterTransport(({ service }) => {
    if (opts.unserved) return;
    service(liveAdmin.LiveAdminService, {
      listDeployments(req) {
        pages.push(req.pageToken);
        const size = req.pageSize || 100;
        const from = req.pageToken ? all.findIndex((d) => d.id === req.pageToken) : 0;
        const page = all.slice(from, from + size);
        const next = all[from + size];
        return { deployments: page, nextPageToken: next ? next.id : '' };
      },
      getDeployment(req) {
        const d = all.find((x) => x.id === req.id);
        if (!d) throw new ConnectError(`deployment ${req.id} not found`, Code.NotFound);
        return {
          deployment: d,
          functions: [
            {
              path: 'messages:react',
              kind: liveAdmin.FunctionKind.MUTATION,
              visibility: liveAdmin.FunctionVisibility.PUBLIC,
              args: {
                kind: {
                  case: 'object',
                  value: {
                    fields: {
                      message: { kind: { case: 'id', value: 'messages' } },
                      emoji: {
                        kind: {
                          case: 'union',
                          value: {
                            members: [
                              { kind: { case: 'literal', value: int64 } },
                              { kind: { case: 'primitive', value: 7 } },
                            ],
                          },
                        },
                      },
                      count: {
                        kind: {
                          case: 'optional',
                          value: { kind: { case: 'primitive', value: 2 } },
                        },
                      },
                    },
                  },
                },
              },
            },
            {
              path: 'internal/cleanup:prune',
              kind: liveAdmin.FunctionKind.QUERY,
              visibility: liveAdmin.FunctionVisibility.INTERNAL,
            },
          ],
        };
      },
      activateDeployment(req) {
        if (req.expectedActive !== active) {
          const info = create(errors.ErrorInfoSchema, { reason: 'live_deploy_conflict' });
          throw new ConnectError(
            `the active deployment is ${active}`,
            Code.FailedPrecondition,
            undefined,
            [{ desc: errors.ErrorInfoSchema, value: info }],
          );
        }
        active = req.id;
        const d = all.find((x) => x.id === req.id);
        return { deployment: { ...d, state: S.ACTIVE, failureReason: '' } };
      },
      async *tailLogs(req) {
        const entry = (n: number, fn: string, level: liveAdmin.LogLevel) => ({
          tsMs: T0 + BigInt(n),
          requestId: `r${n}`,
          function: fn,
          deployment: n === 3 ? '' : id(3),
          level,
          line: `line ${n}`,
          truncated: n === 2,
        });
        const want = (fn: string) => !req.function || req.function === fn;
        yield {
          entries: [
            entry(1, 'messages:send', liveAdmin.LogLevel.INFO),
            entry(2, 'messages:list', liveAdmin.LogLevel.DEBUG),
          ].filter((e) => want(e.function)),
          skipped: 0n,
        };
        yield {
          entries: [entry(3, '_system:get', liveAdmin.LogLevel.ERROR)].filter((e) =>
            want(e.function),
          ),
          skipped: 4n,
        };
      },
    });
  });
  return { api: createLiveAdminApi(transport), pages };
}

async function drain(it: AsyncIterable<LogLine | LogsSkipped>) {
  const out: Array<LogLine | LogsSkipped> = [];
  for await (const e of it) out.push(e);
  return out;
}

describe('createLiveAdminApi', () => {
  it('maps deployments and reads every page', async () => {
    const { api, pages } = engine({ count: PAGE_SIZE + 5 });
    expect(api.mocked).toBe(false);
    const ds = await api.listDeployments();
    expect(ds).toHaveLength(PAGE_SIZE + 5);
    expect(pages).toEqual(['', id(5)]);
    const [newest] = ds;
    expect(newest).toEqual({
      id: id(PAGE_SIZE + 5),
      bundleSha256: 'ab'.repeat(32),
      createdAtMs: Number(T0) + PAGE_SIZE + 5,
      createdBy: 'apikey:k1',
      message: `deploy ${PAGE_SIZE + 5}`,
      parent: id(PAGE_SIZE + 4),
      state: 'active',
      failureReason: undefined,
    });
    const failed = ds.find((d) => d.id === id(2));
    expect(failed?.state).toBe('failed');
    expect(failed?.failureReason).toMatch(/live_schema_violation/);
    expect(ds.at(-1)?.parent).toBeUndefined();
  });

  it('maps functions and the Validator oneof', async () => {
    const { api } = engine();
    const detail = await api.getDeployment(id(3));
    expect(detail.deployment.state).toBe('active');
    expect(detail.functions).toEqual([
      {
        path: 'messages:react',
        kind: 'mutation',
        visibility: 'public',
        args: {
          kind: 'object',
          fields: {
            message: { kind: 'id', table: 'messages' },
            emoji: {
              kind: 'union',
              members: [{ kind: 'literal', value: 'up' }, { kind: 'any' }],
            },
            count: { kind: 'optional', inner: { kind: 'int64' } },
          },
        },
      },
      { path: 'internal/cleanup:prune', kind: 'query', visibility: 'internal' },
    ]);
  });

  it('activates with the expected active deployment, and a lost race is a conflict', async () => {
    const { api } = engine();
    await expect(api.activateDeployment(id(1), id(2))).rejects.toBeInstanceOf(DeployConflictError);
    const d = await api.activateDeployment(id(1), id(3));
    expect(d).toMatchObject({ id: id(1), state: 'active' });
  });

  it('flattens TailLogs batches and reports skipped lines', async () => {
    const { api } = engine();
    const all = await drain(api.tailLogs({ follow: false }, new AbortController().signal));
    expect(all.map((e) => (isSkipped(e) ? `skipped ${e.skipped}` : e.line))).toEqual([
      'line 1',
      'line 2',
      'skipped 4',
      'line 3',
    ]);
    const [first, second] = all as LogLine[];
    expect(first).toEqual({
      ts: Number(T0) + 1,
      requestId: 'r1',
      function: 'messages:send',
      deployment: id(3),
      level: 'info',
      line: 'line 1',
    });
    expect(second).toMatchObject({ level: 'debug', truncated: true });
    expect((all[3] as LogLine).deployment).toBeUndefined();
    const sent = await drain(
      api.tailLogs({ follow: false, function: 'messages:send' }, new AbortController().signal),
    );
    expect(sent.filter((e) => !isSkipped(e)).map((e) => (e as LogLine).function)).toEqual([
      'messages:send',
    ]);
  });

  it('says so when the engine does not serve the admin service', async () => {
    const { api } = engine({ unserved: true });
    await expect(api.listDeployments()).rejects.toThrow(ADMIN_UNSERVED);
  });
});

describe('DeploymentsPage over the engine', () => {
  it('shows no preview notice and no worker health', async () => {
    const { api } = engine();
    render(<DeploymentsPage api={api} />);
    const history = await screen.findByRole('table', { name: 'Deploy history' });
    expect(within(history).getByText(/live_schema_violation/)).toBeTruthy();
    expect(screen.queryByText(new RegExp(PREVIEW_NOTICE))).toBeNull();
    expect(screen.queryByText('Worker health')).toBeNull();
    const log = await screen.findByRole('log', { name: 'Function log lines' });
    expect(await within(log).findByText(/line 3/)).toBeTruthy();
    expect(await screen.findByText(/4 lines were overwritten/)).toBeTruthy();
    // The deployment that ran a line shows without hovering.
    expect(within(log).getAllByText(new RegExp(`deployment ${id(3).slice(-8)}`)).length).toBe(2);
  });
});
