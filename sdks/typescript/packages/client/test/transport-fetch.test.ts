// A custom `fetch` reaches the transport, and Cloudflare Workers gets
// `redirect: "manual"` (connect-web's `"error"` is rejected by workerd).

import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { Loams } from '../src/loams.js';
import {
  isWorkerd,
  transportFetch,
  workersFetch,
} from '../src/runtime/transports.js';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('workersFetch', () => {
  it('forces redirect manual and keeps the rest of init', async () => {
    const seen: RequestInit[] = [];
    const f = workersFetch(async (_i, init) => {
      seen.push(init ?? {});
      return new Response('ok');
    });
    await f('http://x', { method: 'POST', redirect: 'error' });
    expect(seen[0]).toMatchObject({ method: 'POST', redirect: 'manual' });
  });
});

describe('transportFetch', () => {
  it('uses a caller fetch as given', () => {
    const mine = (async () => new Response()) as typeof fetch;
    expect(transportFetch({ fetch: mine })).toBe(mine);
  });

  it('is unset outside workerd', () => {
    expect(transportFetch({})).toBeUndefined();
  });

  it('wraps the global fetch in workerd', async () => {
    vi.stubGlobal('navigator', { userAgent: 'Cloudflare-Workers' });
    const calls: (RequestInit | undefined)[] = [];
    vi.stubGlobal('fetch', async (_i: unknown, init?: RequestInit) => {
      calls.push(init);
      return new Response('ok');
    });
    expect(isWorkerd()).toBe(true);
    await transportFetch({})!('http://x', { redirect: 'error' });
    expect(calls[0]?.redirect).toBe('manual');
  });
});

const refuse = (seen: { url: string; redirect?: RequestRedirect }[]) =>
  (async (input: RequestInfo | URL, init?: RequestInit) => {
    seen.push({
      url: input instanceof Request ? input.url : String(input),
      redirect: init?.redirect ?? (input instanceof Request ? input.redirect : undefined),
    });
    return new Response('{"code":"unimplemented"}', {
      status: 501,
      headers: { 'content-type': 'application/json' },
    });
  }) as typeof fetch;

describe('Loams over a custom fetch', () => {
  it('sends calls through transportOptions.fetch', async () => {
    const seen: { url: string; redirect?: RequestRedirect }[] = [];
    const loams = new Loams({
      endpoint: 'http://loams.test',
      transportOptions: { fetch: refuse(seen) },
      maxRetries: 0,
    });
    await expect(loams.live.admin.deploy({})).rejects.toThrow();
    expect(seen).toHaveLength(1);
    expect(seen[0]!.url).toBe('http://loams.test/loams.live.v1.LiveAdminService/Deploy');
  });

  it('sends redirect manual end to end when running in workerd', async () => {
    vi.stubGlobal('navigator', { userAgent: 'Cloudflare-Workers' });
    const seen: { url: string; redirect?: RequestRedirect }[] = [];
    vi.stubGlobal('fetch', refuse(seen));
    const loams = new Loams({ endpoint: 'http://loams.test', maxRetries: 0 });
    await expect(loams.live.admin.deploy({})).rejects.toThrow();
    expect(seen[0]!.redirect).toBe('manual');
  });
});
