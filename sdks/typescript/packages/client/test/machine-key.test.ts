// IA4 Task 2 (plan IA1, design 76 §10): `machineKey`, an agent's token from its private-key
// JWT key. The token endpoint is a fake `fetch`; the assertion is verified with the public key.

import { generateKeyPairSync, createVerify } from 'node:crypto';
import { describe, expect, it } from 'vite-plus/test';
import {
  JWT_BEARER_GRANT,
  MACHINE_KEY_ASSERTION_TTL_S,
  MACHINE_KEY_REFRESH_EARLY_S,
  machineKey,
} from '../src/runtime/machine-key.js';

function keyPair(format: 'pkcs1' | 'pkcs8') {
  const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  return {
    pem: privateKey.export({ type: format, format: 'pem' }).toString(),
    publicPem: publicKey.export({ type: 'spki', format: 'pem' }).toString(),
  };
}

function fakeEndpoint(answer: { expiresIn?: number; status?: number } = {}) {
  const calls: { url: string; form: URLSearchParams }[] = [];
  const fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), form: new URLSearchParams(String(init?.body)) });
    const n = calls.length;
    if (answer.status !== undefined && answer.status !== 200) {
      return new Response('{"error":"invalid_grant"}', { status: answer.status });
    }
    return Response.json({
      access_token: `at-${n}`,
      token_type: 'Bearer',
      expires_in: answer.expiresIn ?? 900,
    });
  }) as typeof globalThis.fetch;
  return { calls, fetch };
}

const decodePart = (part: string): Record<string, unknown> =>
  JSON.parse(Buffer.from(part, 'base64url').toString('utf8')) as Record<string, unknown>;

describe('machineKey', () => {
  for (const format of ['pkcs1', 'pkcs8'] as const) {
    it(`signs an RFC 7523 assertion with a ${format} key`, async () => {
      const { pem, publicPem } = keyPair(format);
      const endpoint = fakeEndpoint();
      const source = machineKey({
        keyFile: JSON.stringify({ type: 'serviceaccount', keyId: 'k-1', key: pem, userId: 'agent-7' }),
        issuer: 'https://auth.example.com/',
        project: '3141',
        fetch: endpoint.fetch,
      });
      expect(source.userId).toBe('agent-7');
      expect(await source.token()).toBe('at-1');
      const { url, form } = endpoint.calls.at(0) ?? { url: '', form: new URLSearchParams() };
      expect(url).toBe('https://auth.example.com/oauth/v2/token');
      expect(form.get('grant_type')).toBe(JWT_BEARER_GRANT);
      expect(form.get('scope')).toBe('openid urn:zitadel:iam:org:project:id:3141:aud');
      const [header = '', claims = '', signature = ''] = (form.get('assertion') ?? '').split('.');
      const verify = createVerify('RSA-SHA256');
      verify.update(`${header}.${claims}`);
      expect(verify.verify(publicPem, Buffer.from(signature, 'base64url'))).toBe(true);
      expect(decodePart(header)).toMatchObject({ alg: 'RS256', kid: 'k-1' });
      const c = decodePart(claims);
      expect(c).toMatchObject({ iss: 'agent-7', sub: 'agent-7', aud: 'https://auth.example.com' });
      expect((c.exp as number) - (c.iat as number)).toBe(MACHINE_KEY_ASSERTION_TTL_S);
    });
  }

  it('caches until a minute before expiry, refreshes on demand, and shares one trade', async () => {
    const { pem } = keyPair('pkcs1');
    const endpoint = fakeEndpoint({ expiresIn: 900 });
    let clock = 1_000_000;
    const source = machineKey({
      keyFile: { keyId: 'k', key: pem, userId: 'u' },
      issuer: 'https://i.example',
      project: 'p',
      fetch: endpoint.fetch,
      now: () => clock,
    });
    expect(await source.token()).toBe('at-1');
    expect(await source.token()).toBe('at-1');
    clock += (900 - MACHINE_KEY_REFRESH_EARLY_S + 1) * 1000;
    expect(await source.token()).toBe('at-2');
    await source.refresh?.();
    expect(await source.token()).toBe('at-3');
    clock += 3_600_000;
    const tokens = await Promise.all(Array.from({ length: 8 }, () => source.token()));
    expect(new Set(tokens)).toEqual(new Set(['at-4']));
    expect(endpoint.calls.length).toBe(4);
  });

  it('a refused trade sends no credential, and refresh reports it', async () => {
    const { pem } = keyPair('pkcs8');
    const endpoint = fakeEndpoint({ status: 400 });
    const source = machineKey({
      keyFile: { keyId: 'k', key: pem, userId: 'u' },
      issuer: 'https://i.example',
      project: 'p',
      fetch: endpoint.fetch,
    });
    expect(await source.token()).toBeUndefined();
    await expect(source.refresh?.()).rejects.toThrow(/invalid_grant/u);
  });

  it('refuses what is not a machine key', () => {
    const { pem } = keyPair('pkcs1');
    const base = { issuer: 'https://i.example', project: 'p' };
    expect(() =>
      machineKey({ ...base, keyFile: { type: 'application', keyId: 'k', key: pem, userId: 'u' } }),
    ).toThrow();
    expect(() => machineKey({ ...base, keyFile: '{' })).toThrow();
    expect(() => machineKey({ ...base, keyFile: { keyId: 'k', key: 'nope', userId: 'u' } })).toThrow();
    expect(() =>
      machineKey({ ...base, project: '', keyFile: { keyId: 'k', key: pem, userId: 'u' } }),
    ).toThrow();
    expect(() =>
      machineKey({ ...base, issuer: 'auth.example', keyFile: { keyId: 'k', key: pem, userId: 'u' } }),
    ).toThrow();
  });
});
