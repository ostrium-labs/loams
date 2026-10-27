// The REST client builds the contract's paths (api/engines/openapi.json) and maps errors.

import { describe, expect, it, vi } from 'vite-plus/test';
import {
  ApiError,
  createEnginesClient,
  enginesPath,
  gibHours,
  isEntitlementExhausted,
  sizeText,
  vcpuHours,
} from '../src/client.js';
import { detectClient } from '../src/index.js';

function recorder(status = 200, body: unknown = {}) {
  const calls: { url: string; init?: RequestInit }[] = [];
  const fetchFn = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init });
    return new Response(status === 204 ? null : JSON.stringify(body), {
      status,
      headers: { 'content-type': 'application/json' },
    });
  }) as unknown as typeof globalThis.fetch;
  return { calls, fetchFn };
}

describe('paths', () => {
  it('encodes the namespace and instance', () => {
    expect(enginesPath('p 1')).toBe('/v1/namespaces/p%201/engines');
    expect(enginesPath('p', 'eng_x', 'sql')).toBe('/v1/namespaces/p/engines/eng_x/sql');
  });

  it('calls each operation at its contract path and method', async () => {
    const { calls, fetchFn } = recorder(200, { instances: [], sessions: [], jobs: [] });
    const c = createEnginesClient(fetchFn, 'https://srv');
    await c.list('p', 'flink');
    await c.get('p', 'e1');
    await c.create('p', { name: 'n', kind: 'spark', size: { tier: 'm', replicas: 2 } });
    await c.update('p', 'e1', { version: '2.1.0' });
    await c.suspend('p', 'e1');
    await c.resume('p', 'e1');
    await c.remove('p', 'e1');
    await c.usage('p', 'e1', { from: '2026-10-27T00:00:00Z' });
    await c.sql('p', 'e1', { sql: 'select 1' });
    await c.listSessions('p', 'e1');
    await c.createSession('p', 'e1', { kind: 'sql' });
    await c.listJobs('p', 'e1');
    await c.submitJob('p', 'e1', { name: 'j', kind: 'sql', entrypoint: 'select 1' });
    await c.cancelJob('p', 'e1', 'job 1');
    await c.catalog();
    expect(calls.map((x) => `${x.init?.method ?? 'GET'} ${x.url}`)).toEqual([
      'GET https://srv/v1/namespaces/p/engines?kind=flink',
      'GET https://srv/v1/namespaces/p/engines/e1',
      'POST https://srv/v1/namespaces/p/engines',
      'PATCH https://srv/v1/namespaces/p/engines/e1',
      'POST https://srv/v1/namespaces/p/engines/e1/suspend',
      'POST https://srv/v1/namespaces/p/engines/e1/resume',
      'DELETE https://srv/v1/namespaces/p/engines/e1',
      'GET https://srv/v1/namespaces/p/engines/e1/usage?from=2026-10-27T00%3A00%3A00Z',
      'POST https://srv/v1/namespaces/p/engines/e1/sql',
      'GET https://srv/v1/namespaces/p/engines/e1/sessions',
      'POST https://srv/v1/namespaces/p/engines/e1/sessions',
      'GET https://srv/v1/namespaces/p/engines/e1/jobs',
      'POST https://srv/v1/namespaces/p/engines/e1/jobs',
      'POST https://srv/v1/namespaces/p/engines/e1/jobs/job%201/cancel',
      'GET https://srv/v1/engines',
    ]);
    expect(JSON.parse(String(calls[2]?.init?.body))).toEqual({
      name: 'n',
      kind: 'spark',
      size: { tier: 'm', replicas: 2 },
    });
    expect(calls[2]?.init?.credentials).toBe('include');
  });

  it('closes a session with DELETE and an empty answer', async () => {
    const { calls, fetchFn } = recorder(204);
    await createEnginesClient(fetchFn).closeSession('p', 'e1', 's1');
    expect(calls[0]?.init?.method).toBe('DELETE');
    expect(calls[0]?.url).toBe('/v1/namespaces/p/engines/e1/sessions/s1');
  });
});

describe('errors', () => {
  it('maps the {error, message} body', async () => {
    const { fetchFn } = recorder(501, {
      error: 'engine_not_enabled',
      message: 'no adapter serves spark',
    });
    const err = await createEnginesClient(fetchFn)
      .get('p', 'e1')
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err).toMatchObject({
      code: 'engine_not_enabled',
      message: 'no adapter serves spark',
      status: 501,
    });
  });

  it('falls back to the status for a body that is not JSON', async () => {
    const fetchFn = (async () =>
      new Response('gateway down', { status: 502, statusText: 'Bad Gateway' })) as typeof fetch;
    const err = await createEnginesClient(fetchFn)
      .catalog()
      .catch((e: unknown) => e);
    expect(err).toMatchObject({ code: 'error', message: 'Bad Gateway', status: 502 });
  });

  it('lists the namespaces of all project environments, falling back to default', async () => {
    const fetchFn = (async (url: string | URL | Request) => {
      const u = String(url);
      const body = u.endsWith('/api/v1/projects')
        ? { projects: [{ slug: 'web' }, { slug: 'data' }] }
        : u.includes('/web/')
          ? { environments: [{ namespace: 'web-dev' }, { namespace: 'web-prod' }] }
          : { environments: [{ namespace: 'data-dev' }, { namespace: 'web-dev' }] };
      return new Response(JSON.stringify(body), { status: 200 });
    }) as typeof fetch;
    expect(await createEnginesClient(fetchFn).listNamespaces()).toEqual([
      'web-dev',
      'web-prod',
      'data-dev',
    ]);
    const down = recorder(500, { error: 'internal', message: 'x' });
    expect(await createEnginesClient(down.fetchFn).listNamespaces()).toEqual(['default']);
  });
});

describe('detectClient', () => {
  it('uses the server when it serves the catalog, else the sample', async () => {
    const served = recorder(200, { engines: [], sizes: [] });
    expect((await detectClient({ fetch: served.fetchFn, baseUrl: '' })).sample).toBe(false);
    const missing = recorder(404, { error: 'not_found', message: 'no route' });
    expect((await detectClient({ fetch: missing.fetchFn, baseUrl: '' })).sample).toBe(true);
    const odd = recorder(200, {});
    expect((await detectClient({ fetch: odd.fetchFn, baseUrl: '' })).sample).toBe(true);
  });
});

describe('entitlement', () => {
  it('recognises a 402 entitlement_exhausted', async () => {
    const { fetchFn } = recorder(402, {
      error: 'entitlement_exhausted',
      message: 'engine compute is at 100% of this month',
    });
    const err = await createEnginesClient(fetchFn)
      .resume('ns', 'e1')
      .catch((e: unknown) => e);
    expect(isEntitlementExhausted(err)).toBe(true);
    expect(isEntitlementExhausted(new ApiError('not_found', 'x', 404))).toBe(false);
  });

  it('converts metered units', () => {
    expect(vcpuHours(7_200_000)).toBe(2);
    expect(gibHours(1024 * 3_600_000)).toBe(1);
  });
});

describe('sizeText', () => {
  it('says tier, vCPU, memory and replicas', () => {
    expect(sizeText({ tier: 'm', vcpu_milli: 2000, memory_mib: 8192, replicas: 3 })).toBe(
      'M · 2 vCPU · 8 GiB × 3',
    );
    expect(sizeText({ tier: 'xs', vcpu_milli: 500, memory_mib: 512, replicas: 1 })).toBe(
      'XS · 0.5 vCPU · 512 MiB × 1',
    );
  });
});
