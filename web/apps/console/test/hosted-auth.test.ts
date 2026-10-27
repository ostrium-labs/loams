// @vitest-environment node
// The hosted console's OIDC sign-in (src/hosted/auth.ts) against a fake issuer:
// Authorization Code + PKCE, tokens in memory only, silent renew, refresh, logout.

import { createWebPlatform, webFetch } from '@loams/platform-web';
import { describe, expect, it, vi } from 'vite-plus/test';
import { OidcAuth } from '../src/hosted/auth.js';

const ISSUER = 'https://auth.loams.dev';
const REDIRECT = 'https://console.loams.dev/';
const CLIENT = 'loams-console';

const b64url = (v: unknown) =>
  btoa(JSON.stringify(v)).replace(/=+$/, '').replace(/\+/g, '-').replace(/\//g, '_');

function idToken(nonce: string, now: number) {
  const s = Math.floor(now / 1000);
  return [
    b64url({ alg: 'RS256', typ: 'JWT' }),
    b64url({
      iss: ISSUER,
      aud: CLIENT,
      sub: 'usr_1',
      email: 'omar@example.com',
      nonce,
      iat: s,
      exp: s + 300,
    }),
    'c2lnbmF0dXJl',
  ].join('.');
}

class MemoryStorage {
  map = new Map<string, string>();
  getItem(k: string) {
    return this.map.get(k) ?? null;
  }
  setItem(k: string, v: string) {
    this.map.set(k, v);
  }
  removeItem(k: string) {
    this.map.delete(k);
  }
}

function issuer(opts: { expiresIn?: number; endSession?: boolean } = {}) {
  let nonce = '';
  let now = Date.now();
  const tokenBodies: URLSearchParams[] = [];
  let issued = 0;
  const fetch = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(input instanceof Request ? input.url : String(input));
    if (url.pathname === '/.well-known/openid-configuration') {
      return Response.json({
        issuer: ISSUER,
        authorization_endpoint: `${ISSUER}/authorize`,
        token_endpoint: `${ISSUER}/token`,
        jwks_uri: `${ISSUER}/jwks`,
        ...(opts.endSession === false ? {} : { end_session_endpoint: `${ISSUER}/logout` }),
      });
    }
    if (url.pathname === '/token') {
      const body = new URLSearchParams(String(init?.body));
      tokenBodies.push(body);
      issued += 1;
      return Response.json({
        access_token: `at-${issued}`,
        token_type: 'Bearer',
        expires_in: opts.expiresIn ?? 3600,
        refresh_token: `rt-${issued}`,
        id_token: idToken(nonce, now),
      });
    }
    return new Response('not found', { status: 404 });
  });
  return {
    fetch: fetch as unknown as typeof globalThis.fetch,
    tokenBodies,
    setNonce: (n: string) => {
      nonce = n;
    },
    now: () => now,
    advance: (ms: number) => {
      now += ms;
    },
  };
}

function setup(opts: Parameters<typeof issuer>[0] = {}) {
  const idp = issuer(opts);
  const storage = new MemoryStorage();
  const assigned: string[] = [];
  const auth = new OidcAuth({ issuer: ISSUER, clientId: CLIENT, scope: 'openid email' }, REDIRECT, {
    fetch: idp.fetch,
    storage,
    assign: (u) => assigned.push(u),
    now: idp.now,
  });
  /** Signs in and returns from the issuer with a code. */
  const signIn = async (returnTo = '#/live') => {
    await auth.signIn(returnTo);
    const url = new URL(assigned.at(-1) ?? '');
    idp.setNonce(url.searchParams.get('nonce') ?? '');
    const back = new URL(REDIRECT);
    back.searchParams.set('code', 'code-1');
    back.searchParams.set('state', url.searchParams.get('state') ?? '');
    return { url, result: await auth.completeSignIn(back) };
  };
  return { idp, storage, assigned, auth, signIn };
}

describe('OidcAuth', () => {
  it('sends the person to the issuer with PKCE S256, state and nonce', async () => {
    const { auth, assigned } = setup();
    await auth.signIn('#/graph');
    const url = new URL(assigned[0] ?? '');
    expect(url.origin + url.pathname).toBe(`${ISSUER}/authorize`);
    expect(Object.fromEntries(url.searchParams)).toMatchObject({
      client_id: CLIENT,
      redirect_uri: REDIRECT,
      response_type: 'code',
      scope: 'openid email',
      code_challenge_method: 'S256',
    });
    expect(url.searchParams.get('code_challenge')).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(url.searchParams.get('state')).toBeTruthy();
    expect(url.searchParams.get('nonce')).toBeTruthy();
    expect(url.searchParams.has('prompt')).toBe(false);
  });

  it('exchanges the code with the verifier and keeps the tokens in memory only', async () => {
    const { auth, idp, storage, signIn } = setup();
    const { result } = await signIn('#/live');
    expect(result).toEqual({ kind: 'signed-in', returnTo: '#/live' });
    const body = idp.tokenBodies[0];
    expect(body?.get('grant_type')).toBe('authorization_code');
    expect(body?.get('code')).toBe('code-1');
    expect(body?.get('code_verifier')).toMatch(/^[A-Za-z0-9_-]{43,}$/);
    expect(await auth.accessToken()).toBe('at-1');
    expect(auth.user()).toEqual({ subject: 'usr_1', email: 'omar@example.com', name: undefined });
    // Nothing that grants access is stored: only the "signed in" marker remains.
    expect([...storage.map.keys()]).toEqual(['loams.auth.session']);
    expect([...storage.map.values()].join()).not.toMatch(/at-1|rt-1|code-1/);
    expect(auth.wasSignedIn()).toBe(true);
  });

  it('refuses a callback whose state does not match', async () => {
    const { auth, assigned } = setup();
    await auth.signIn();
    expect(assigned).toHaveLength(1);
    const back = new URL(REDIRECT);
    back.searchParams.set('code', 'code-1');
    back.searchParams.set('state', 'forged');
    const result = await auth.completeSignIn(back);
    expect(result.kind).toBe('error');
    expect(auth.signedIn()).toBe(false);
  });

  it('turns an unreadable stored request into a sign-in error', async () => {
    const { auth, storage } = setup();
    storage.setItem('loams.auth.pending', '{not json');
    const back = new URL(REDIRECT);
    back.searchParams.set('code', 'c');
    back.searchParams.set('state', 's');
    expect((await auth.completeSignIn(back)).kind).toBe('error');
    expect(storage.getItem('loams.auth.pending')).toBeNull();
  });

  it('ignores a page load that is not a callback', async () => {
    const { auth } = setup();
    expect(await auth.completeSignIn(new URL(`${REDIRECT}#/live`))).toEqual({ kind: 'none' });
  });

  it('renews silently with prompt=none and falls back to the sign-in screen', async () => {
    const { auth, assigned } = setup();
    await auth.signIn('#/', true);
    const url = new URL(assigned[0] ?? '');
    expect(url.searchParams.get('prompt')).toBe('none');
    const back = new URL(REDIRECT);
    back.searchParams.set('error', 'login_required');
    back.searchParams.set('state', url.searchParams.get('state') ?? '');
    expect(await auth.completeSignIn(back)).toEqual({ kind: 'interaction-required' });
    expect(auth.wasSignedIn()).toBe(false);
  });

  it('refreshes an access token that is about to expire', async () => {
    const { auth, idp, signIn } = setup({ expiresIn: 120 });
    await signIn();
    expect(await auth.accessToken()).toBe('at-1');
    idp.advance(90_000);
    expect(await auth.accessToken()).toBe('at-2');
    expect(idp.tokenBodies[1]?.get('grant_type')).toBe('refresh_token');
    expect(idp.tokenBodies[1]?.get('refresh_token')).toBe('rt-1');
  });

  it('signs out at the issuer with the id token hint and forgets everything', async () => {
    const { auth, assigned, signIn } = setup();
    await signIn();
    await auth.signOut();
    const url = new URL(assigned.at(-1) ?? '');
    expect(url.origin + url.pathname).toBe(`${ISSUER}/logout`);
    expect(url.searchParams.get('post_logout_redirect_uri')).toBe(REDIRECT);
    expect(url.searchParams.get('id_token_hint')).toMatch(/^ey/);
    expect(await auth.accessToken()).toBeUndefined();
    expect(auth.wasSignedIn()).toBe(false);
  });

  it('without RP logout, signing out returns to the console', async () => {
    const { auth, assigned, signIn } = setup({ endSession: false });
    await signIn();
    await auth.signOut();
    expect(assigned.at(-1)).toBe(REDIRECT);
  });
});

describe('platform-web with a token', () => {
  it('sends the bearer and no cookies to the engine', async () => {
    const seen: RequestInit[] = [];
    const real = globalThis.fetch;
    globalThis.fetch = (async (_i: RequestInfo | URL, init?: RequestInit) => {
      seen.push(init ?? {});
      return new Response('{}');
    }) as typeof fetch;
    try {
      await webFetch(undefined, async () => 'at-1')('https://loams.tail1.ts.net/health');
      await webFetch(undefined, () => undefined)('https://loams.tail1.ts.net/health');
      await webFetch()('/health');
    } finally {
      globalThis.fetch = real;
    }
    expect(new Headers(seen[0]?.headers).get('authorization')).toBe('Bearer at-1');
    expect(seen[0]?.credentials).toBe('omit');
    expect(new Headers(seen[1]?.headers).has('authorization')).toBe(false);
    expect(seen[1]?.credentials).toBe('omit');
    // The engine-served console keeps its cookie session.
    expect(seen[2]?.credentials).toBe('include');
    expect(createWebPlatform({ token: () => 'x' }).name).toBe('platform-web');
  });
});
