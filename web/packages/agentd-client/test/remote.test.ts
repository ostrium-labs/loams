// The web transport and the host registry (plan SD1 Task 7). No network: the
// transport runs over a recording fetch.

import { createClient } from '@connectrpc/connect';
import { ProjectService } from '@loams/proto/agentd';
import { describe, expect, it } from 'vite-plus/test';
import {
  type AgentdHost,
  createHostRegistry,
  fingerprintOf,
  HOSTS_KEY,
  hostFromParam,
  localHostStore,
  parseHosts,
} from '../src/hosts';
import { agentdBaseUrl, createRemoteAgentdTransport, remoteFetch } from '../src/transport-remote';

const HOST = 'https://devbox.example.ts.net:8443';

/** A fetch that records each request and answers a Connect unary ListRepos. */
function recording() {
  const seen: { url: string; init: RequestInit; headers: Headers }[] = [];
  const fetch: typeof globalThis.fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    seen.push({ url, init: init ?? {}, headers: new Headers(init?.headers) });
    return new Response(JSON.stringify({ repos: [{ path: '/src/app', name: 'app' }] }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  };
  return { seen, fetch };
}

describe('the remote transport', () => {
  it('transport_remote_sends_bearer', async () => {
    const { seen, fetch } = recording();
    let token = 'access-1';
    const transport = createRemoteAgentdTransport({
      kind: 'remote',
      baseUrl: `${HOST}/`,
      getToken: () => token,
      fetch,
      useBinaryFormat: false,
    });
    const projects = createClient(ProjectService, transport);
    const reply = await projects.listRepos({});
    expect(reply.repos.map((r) => r.name)).toEqual(['app']);
    expect(seen).toHaveLength(1);
    expect(seen[0]?.url).toBe(`${HOST}/loams.agentd.v1.ProjectService/ListRepos`);
    expect(seen[0]?.headers.get('authorization')).toBe('Bearer access-1');
    expect(seen[0]?.init.credentials).toBe('omit');
    expect(seen[0]?.init.redirect).toBe('error');
    expect(seen[0]?.init.referrerPolicy).toBe('no-referrer');

    // The token is read per request, so a refreshed one is used at once.
    token = 'access-2';
    await projects.listRepos({});
    expect(seen[1]?.headers.get('authorization')).toBe('Bearer access-2');
  });

  it('refuses to call without a token, and never sends a cookie', async () => {
    const { seen, fetch } = recording();
    const call = remoteFetch(async () => undefined, fetch);
    await expect(call(`${HOST}/x`, { method: 'POST' })).rejects.toThrow(/access token/);
    expect(seen).toHaveLength(0);
    const withToken = remoteFetch(() => 't', fetch);
    await withToken(`${HOST}/x`, { method: 'POST', headers: { cookie: 'session=1' } });
    expect(seen[0]?.headers.get('cookie')).toBeNull();
  });

  it('takes https hosts only (http on loopback for development)', () => {
    expect(agentdBaseUrl('https://devbox.example.ts.net:8443/')).toBe(HOST);
    expect(agentdBaseUrl('https://example.com/agentd/')).toBe('https://example.com/agentd');
    expect(agentdBaseUrl('http://localhost:8443')).toBe('http://localhost:8443');
    expect(agentdBaseUrl('http://127.0.0.1:8443')).toBe('http://127.0.0.1:8443');
    for (const bad of [
      'http://devbox.example.com',
      'https://user:pw@devbox.example.com',
      'https://devbox.example.com/?x=1',
      'https://devbox.example.com/#x',
      'javascript:alert(1)',
      'file:///etc/passwd',
      'devbox:8443',
      '',
    ]) {
      expect(agentdBaseUrl(bad), bad).toBeUndefined();
    }
    expect(() =>
      createRemoteAgentdTransport({
        kind: 'remote',
        baseUrl: 'http://evil.example',
        getToken: () => 't',
      }),
    ).toThrow(/https/);
  });
});

/** An in-memory Storage. */
function memoryStorage(): Storage {
  const map = new Map<string, string>();
  return {
    get length() {
      return map.size;
    },
    clear: () => map.clear(),
    getItem: (k) => map.get(k) ?? null,
    key: (i) => [...map.keys()][i] ?? null,
    removeItem: (k) => void map.delete(k),
    setItem: (k, v) => void map.set(k, String(v)),
  };
}

describe('the host registry', () => {
  it('host_param_is_id_not_url', () => {
    const registry = createHostRegistry(localHostStore(memoryStorage));
    const host = registry.add({ name: 'Dev box', url: HOST });
    expect(hostFromParam(registry.list(), host.id)).toEqual(host);
    for (const param of [
      'https://evil.example',
      HOST,
      'devbox.example.ts.net',
      'h-unknown',
      `${host.id}/`,
      '',
      null,
      undefined,
    ]) {
      expect(hostFromParam(registry.list(), param), String(param)).toBeUndefined();
    }
  });

  it('hosts_storage_failure_tolerated', () => {
    const throwing = () =>
      ({
        getItem() {
          throw new Error('SecurityError');
        },
        setItem() {
          throw new Error('QuotaExceededError');
        },
      }) as unknown as Storage;
    const registry = createHostRegistry(localHostStore(throwing));
    expect(registry.list()).toEqual([]);
    const host = registry.add({ name: 'Dev box', url: HOST });
    expect(registry.list()).toEqual([host]);
    registry.remove(host.id);
    expect(registry.list()).toEqual([]);

    // No storage at all, and garbage in storage.
    expect(createHostRegistry(localHostStore(() => undefined)).list()).toEqual([]);
    const storage = memoryStorage();
    storage.setItem(HOSTS_KEY, '{not json');
    expect(createHostRegistry(localHostStore(() => storage)).list()).toEqual([]);
  });

  it('persists hosts under loams.studio.hosts and drops malformed rows', () => {
    const storage = memoryStorage();
    const registry = createHostRegistry(localHostStore(() => storage));
    const host = registry.add({
      name: ' Dev box ',
      url: `${HOST}/`,
      fingerprint: `sha256:${'ab'.repeat(32)}`,
    });
    expect(host.name).toBe('Dev box');
    expect(host.url).toBe(HOST);
    expect(host.fingerprint).toBe(Array(32).fill('AB').join(':'));
    const again = createHostRegistry(localHostStore(() => storage));
    expect(again.list()).toEqual([host]);

    const rows: unknown[] = [
      host,
      { id: 'h-2', name: 'evil', url: 'http://evil.example' },
      { id: 'https://x', name: 'x', url: HOST },
      { id: host.id, name: 'dup', url: 'https://other.example' },
      'nope',
      null,
    ];
    expect(parseHosts(rows)).toEqual([host]);
    expect(parseHosts({})).toEqual([]);
  });

  it('refuses bad URLs, bad fingerprints and duplicates', () => {
    const registry = createHostRegistry(localHostStore(memoryStorage));
    expect(() => registry.add({ name: 'x', url: 'http://devbox.example.com' })).toThrow(/https/);
    expect(() => registry.add({ name: 'x', url: HOST, fingerprint: 'abc' })).toThrow(/fingerprint/);
    registry.add({ name: 'x', url: HOST });
    expect(() => registry.add({ name: 'y', url: `${HOST}/` })).toThrow(/already/);
    expect(fingerprintOf('AB'.repeat(32))).toBe(Array(32).fill('AB').join(':'));
    expect(fingerprintOf('zz')).toBeUndefined();
  });

  it('notifies subscribers of changes', () => {
    const registry = createHostRegistry(localHostStore(memoryStorage));
    const calls: AgentdHost[][] = [];
    const off = registry.subscribe(() => calls.push([...registry.list()]));
    const host = registry.add({ name: 'a', url: HOST });
    registry.remove(host.id);
    off();
    registry.add({ name: 'b', url: HOST });
    expect(calls).toEqual([[host], []]);
  });
});
