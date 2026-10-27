// The /postgres route's pg-control client follows App settings > Postgres: a saved change
// drops the client, and the next resolve probes pg-control again (no restart).
import type { LoamsDesktopApi, PgControlSettings } from '@loams/desktop/contracts';
import { describe, expect, it } from 'vite-plus/test';
import { createPgControlSource } from '../src/route.js';

const BASE = 'loams-app://console/pg-control/';

const settings = (effectiveUrl: string): PgControlSettings => ({
  url: effectiveUrl,
  apiKeyFile: '',
  defaultUrl: 'http://127.0.0.1:7680',
  effectiveUrl,
  envOverrides: [],
  keySource: 'none',
});

function bridge(initial: string) {
  let current = settings(initial);
  const listeners = new Set<(s: PgControlSettings) => void>();
  const pgControl = {
    settings: async () => current,
    onChanged: (cb: (s: PgControlSettings) => void) => {
      listeners.add(cb);
      return () => void listeners.delete(cb);
    },
  } as unknown as NonNullable<LoamsDesktopApi['pgControl']>;
  return {
    desktop: { pgControl },
    save(url: string) {
      current = settings(url);
      for (const l of listeners) l(current);
    },
    listeners,
  };
}

describe('createPgControlSource', () => {
  it('keeps a client that answers, and probes again after a saved change', async () => {
    let up = true;
    let probes = 0;
    const fetch = (async () => {
      probes += 1;
      if (!up) throw new TypeError('fetch failed');
      return new Response('{"operations":[]}', { headers: { 'content-type': 'application/json' } });
    }) as typeof globalThis.fetch;
    const b = bridge('http://127.0.0.1:7680');
    const source = createPgControlSource(b.desktop, BASE, fetch);
    let changes = 0;
    source.onChange(() => {
      changes += 1;
    });

    const first = await source.resolve();
    expect(first.mock).toBeUndefined();
    // The subtitle shows pg-control's address, not the proxy's.
    expect(first.endpoint).toBe('127.0.0.1:7680');
    expect(await source.resolve()).toBe(first);
    expect(probes).toBe(1);

    up = false;
    b.save('http://10.0.0.5:7681');
    expect(changes).toBe(1);
    const second = await source.resolve();
    expect(second).not.toBe(first);
    expect(second.mock).toBe(true);
    expect(probes).toBe(2);

    // A mock is not kept: the next visit probes again.
    up = true;
    const third = await source.resolve();
    expect(third.mock).toBeUndefined();
    expect(third.endpoint).toBe('10.0.0.5:7681');

    source.dispose();
    expect(b.listeners.size).toBe(0);
  });

  it('works on a bridge without pg-control settings', async () => {
    const fetch = (async () =>
      new Response('{}', {
        headers: { 'content-type': 'application/json' },
      })) as typeof globalThis.fetch;
    const source = createPgControlSource({}, BASE, fetch);
    const c = await source.resolve();
    expect(c.mock).toBeUndefined();
    expect(c.endpoint).toBe('console');
  });
});
