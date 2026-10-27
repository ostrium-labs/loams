// The console on Connect at the base path's root (API2 Task 10, AP1a Task 5): old
// addresses land on their hash routes, the cookie session's CSRF token rides on
// mutations, and nothing of the OpenAPI client is left.

import { webFetch } from '@loams/platform-web';
import { describe, expect, it, vi } from 'vite-plus/test';
import pkg from '../package.json';
import { legacyUrl } from '../src/cordis/legacy.js';

// The console tsconfig carries no Node types: node:fs by a variable specifier.
const fsId = 'node:fs';
const { readFileSync } = (await import(/* @vite-ignore */ fsId)) as {
  readFileSync(path: URL, enc: 'utf8'): string;
};
// node:fs takes the file: URL itself. Its `pathname` is not a path on Windows
// (`/D:/a/...`, which readFileSync resolves to `D:\D:\a\...`).
const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), 'utf8');

const at = (href: string, base = '/ui/') => {
  const url = new URL(href, 'https://loams.example');
  return legacyUrl({ pathname: url.pathname, search: url.search, hash: url.hash }, base);
};

describe('old console addresses', () => {
  it('old_deep_links_redirect_to_hash_routes', () => {
    expect(at('/ui/projects/docs/environments/dev')).toBe('/ui/#/projects/docs/environments/dev');
    expect(at('/ui/teams')).toBe('/ui/#/teams');
    expect(at('/ui/audit/')).toBe('/ui/#/audit');
    expect(at('/ui/projects/docs/agents/agt_1')).toBe('/ui/#/projects/docs/agents/agt_1');
    expect(at('/ui/sign-in?next=%2Fteams')).toBe('/ui/#/sign-in?next=%2Fteams');
    expect(at('/ui/consent?client_id=cc&code_challenge_method=S256')).toBe(
      '/ui/#/consent?client_id=cc&code_challenge_method=S256',
    );
  });

  it('the_old_cordis_entry_moves_to_the_root_and_keeps_its_flags', () => {
    expect(at('/ui/cordis.html')).toBe('/ui/');
    expect(at('/ui/cordis.html?desktop#/data')).toBe('/ui/?desktop#/data');
    expect(at('/ui/index.html#/teams')).toBe('/ui/#/teams');
    expect(at('/ui')).toBe('/ui/');
    expect(at('/ui/projects/docs?desktop')).toBe('/ui/?desktop=#/projects/docs');
  });

  it('leaves_current_addresses_alone', () => {
    expect(at('/ui/')).toBeUndefined();
    expect(at('/ui/#/teams')).toBeUndefined();
    expect(at('/ui/?demo')).toBeUndefined();
    expect(at('/elsewhere')).toBeUndefined();
  });

  it('works_at_the_root_base_too', () => {
    expect(at('/teams', '/')).toBe('/#/teams');
    expect(at('/cordis.html?desktop', '/')).toBe('/?desktop');
    expect(at('/', '/')).toBeUndefined();
  });
});

describe('the cookie session', () => {
  it('csrf_token_rides_on_mutations_only', async () => {
    const seen: Headers[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
        seen.push(new Headers(init?.headers));
        return new Response('{}');
      }),
    );
    let token: string | undefined = 'csrf_1';
    const f = webFetch(undefined, undefined, () => token);
    await f('/loams.admin.v1.KeyService/CreateKey', { method: 'POST' });
    await f('/loams.admin.v1.OrgService/GetOrg', { method: 'GET' });
    token = undefined;
    await f('/loams.auth.v1.AuthService/DeleteSession', { method: 'POST' });
    expect(seen.map((h) => h.get('x-csrf-token'))).toEqual(['csrf_1', null, null]);
    // A bearer session sends no cookie and needs no CSRF token.
    const bearer = webFetch(
      undefined,
      () => 'tok',
      () => 'csrf_1',
    );
    await bearer('/loams.admin.v1.KeyService/CreateKey', { method: 'POST' });
    expect(seen[3]?.get('x-csrf-token')).toBeNull();
    expect(seen[3]?.get('authorization')).toBe('Bearer tok');
    vi.unstubAllGlobals();
  });
});

describe('API2 Task 10', () => {
  it('console_has_no_openapi_dependency', () => {
    const all = { ...pkg.dependencies, ...pkg.devDependencies } as Record<string, string>;
    expect(Object.keys(all).filter((d) => d.startsWith('openapi-'))).toEqual([]);
    expect(Object.keys(pkg.scripts)).not.toContain('gen:api');
    const lock = read('../../../../pnpm-lock.yaml');
    expect(lock).not.toMatch(/^\s+'?openapi-(fetch|typescript)/m);
    expect(() => read('../src/api/schema.d.ts')).toThrow();
  });
});
