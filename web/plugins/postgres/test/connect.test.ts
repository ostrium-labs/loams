// The loams.postgres.v1 Connect JSON client against a fake pg-control (PG2 R9.1).
import { describe, expect, it } from 'vite-plus/test';
import {
  createConnectPgControl,
  PG_CONTROL_STATUS_HEADER,
  PgControlUnreachable,
  resolvePgControl,
} from '../src/control/connect.js';
import { createMockPgControl } from '../src/control/mock.js';
import { PgControlError } from '../src/control/types.js';

const BASE = 'loams-app://console/pg-control/';

type Answer = { status?: number; body: unknown; headers?: Record<string, string> };

function fake(answer: (path: string, body: Record<string, unknown>) => Answer) {
  const calls: { path: string; body: Record<string, unknown> }[] = [];
  const fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const path = String(input).slice(BASE.length - 1);
    const body = JSON.parse(String(init?.body ?? '{}'));
    calls.push({ path, body });
    const a = answer(path, body);
    return new Response(JSON.stringify(a.body), {
      status: a.status ?? 200,
      headers: { 'content-type': 'application/json', ...a.headers },
    });
  }) as typeof globalThis.fetch;
  return { calls, fetch };
}

const PROJECT = {
  id: 'prj-1',
  namespace: 'default',
  name: 'chinook',
  pgVersion: 17,
  region: 'local',
  historyRetention: '604800s',
  defaultBranchId: 'br-main',
  walPool: 'default',
  state: 'PROJECT_STATE_READY',
  createTime: '2026-10-18T00:00:00Z',
  updateTime: '2026-10-18T00:00:00Z',
  version: '3',
};

describe('createConnectPgControl', () => {
  it('lists projects across pages and maps proto3 JSON', async () => {
    const f = fake((_p, body) =>
      body.pageToken
        ? { body: { projects: [{ ...PROJECT, id: 'prj-2', state: 'PROJECT_STATE_CREATING' }] } }
        : { body: { projects: [PROJECT], nextPageToken: 't2' } },
    );
    const client = createConnectPgControl({ baseUrl: BASE, fetch: f.fetch });
    const list = await client.listProjects('default');
    expect(f.calls.map((c) => c.path)).toEqual([
      '/loams.postgres.v1.PostgresService/ListProjects',
      '/loams.postgres.v1.PostgresService/ListProjects',
    ]);
    expect(f.calls[0]?.body).toMatchObject({ namespace: 'default', pageToken: '' });
    expect(list[0]).toMatchObject({
      id: 'prj-1',
      name: 'chinook',
      pgVersion: 17,
      historyRetentionSeconds: 604800,
      state: 'ready',
      version: 3,
    });
    expect(list[1]?.state).toBe('creating');
    expect(client.mock).toBeUndefined();
  });

  it('sends the mask, the expected version and an idempotency key', async () => {
    const f = fake(() => ({
      body: {
        branch: { id: 'br-1', projectId: 'prj-1', protected: true, state: 'BRANCH_STATE_READY' },
      },
    }));
    const client = createConnectPgControl({
      baseUrl: BASE,
      fetch: f.fetch,
      idempotencyKey: () => 'k1',
    });
    const b = await client.setBranchProtected(
      'default',
      { id: 'br-1', projectId: 'prj-1', version: 7 } as never,
      true,
    );
    expect(f.calls[0]).toEqual({
      path: '/loams.postgres.v1.PostgresService/UpdateBranch',
      body: {
        namespace: 'default',
        projectId: 'prj-1',
        branch: { id: 'br-1', projectId: 'prj-1', protected: true },
        updateMask: 'protected',
        expectedVersion: '7',
        idempotencyKey: 'k1',
      },
    });
    expect(b).toMatchObject({ protected: true, state: 'ready' });
  });

  it('reads operations from loams.operations.v1, newest first', async () => {
    const f = fake(() => ({
      body: {
        operations: [
          { id: 'op-1', state: 'OPERATION_STATE_SUCCEEDED', createdAt: '2026-10-18T00:00:00Z' },
          {
            id: 'op-2',
            state: 'OPERATION_STATE_RUNNING',
            createdAt: '2026-10-18T00:01:00Z',
            progress: { done: '1', total: '3', unit: 'steps' },
          },
        ],
      },
    }));
    const ops = await createConnectPgControl({ baseUrl: BASE, fetch: f.fetch }).listOperations(
      'default',
    );
    expect(f.calls[0]?.path).toBe('/loams.operations.v1.OperationsService/ListOperations');
    expect(ops.map((o) => [o.id, o.state])).toEqual([
      ['op-2', 'running'],
      ['op-1', 'succeeded'],
    ]);
    expect(ops[0]?.progress).toMatchObject({ done: 1, total: 3 });
  });

  it('turns a Connect error into PgControlError with the ErrorInfo reason', async () => {
    // ErrorInfo { reason: "branch_has_children" }: field 1, length 19.
    const info = new Uint8Array([10, 19, ...new TextEncoder().encode('branch_has_children')]);
    const value = btoa(String.fromCharCode(...info)).replace(/=+$/, '');
    const f = fake(() => ({
      status: 412,
      body: {
        code: 'failed_precondition',
        message: 'the branch has children',
        details: [{ type: 'loams.errors.v1.ErrorInfo', value }],
      },
    }));
    const client = createConnectPgControl({ baseUrl: BASE, fetch: f.fetch });
    const e = await client
      .deleteBranch('default', { id: 'b', projectId: 'p', version: 1 } as never)
      .catch((x) => x);
    expect(e).toBeInstanceOf(PgControlError);
    expect(e).toMatchObject({ reason: 'branch_has_children', message: 'the branch has children' });
  });

  it('says unreachable when main marks the answer as its own', async () => {
    const f = fake(() => ({
      status: 503,
      body: { code: 'unavailable', message: 'No Postgres control plane answered' },
      headers: { [PG_CONTROL_STATUS_HEADER]: 'unreachable' },
    }));
    await expect(
      createConnectPgControl({ baseUrl: BASE, fetch: f.fetch }).listProjects('default'),
    ).rejects.toBeInstanceOf(PgControlUnreachable);
  });
});

describe('resolvePgControl', () => {
  it('uses pg-control when it answers, a refusal included', async () => {
    const f = fake(() => ({
      status: 401,
      body: { code: 'unauthenticated', message: 'no credential' },
    }));
    const c = await resolvePgControl({ baseUrl: BASE, fetch: f.fetch }, createMockPgControl);
    expect(c.mock).toBeUndefined();
  });

  it('falls back to the labelled mock when nothing answers', async () => {
    const fetch = (async () => {
      throw new TypeError('fetch failed');
    }) as typeof globalThis.fetch;
    const c = await resolvePgControl({ baseUrl: BASE, fetch }, createMockPgControl);
    expect(c.mock).toBe(true);
  });
});

describe('endpoint', () => {
  it('reports the host:port pg-control is reached at', () => {
    expect(createConnectPgControl({ baseUrl: 'http://127.0.0.1:7680' }).endpoint).toBe(
      '127.0.0.1:7680',
    );
  });
});
