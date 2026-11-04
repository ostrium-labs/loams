import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import { isPublicHost, MAX_HOPS, safeRequest } from '../src/http.js';
import { ociHttp, resolveOciDigest } from '../src/oci.js';
import { httpFetcher } from '../src/verify.js';

type Call = { url: string; headers: Record<string, string> };

function stub(handler: (call: Call) => Response): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal('fetch', async (url: URL | string, init: RequestInit) => {
    const call = { url: String(url), headers: { ...(init.headers as Record<string, string>) } };
    calls.push(call);
    return handler(call);
  });
  return calls;
}

const redirect = (to: string) => new Response(null, { status: 302, headers: { location: to } });

/** A chunked body of `n` bytes with no content-length, or one that lies about its length. */
function streamed(n: number, headers: Record<string, string> = {}): Response {
  let sent = 0;
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (sent >= n) return controller.close();
      const size = Math.min(1024, n - sent);
      sent += size;
      controller.enqueue(new Uint8Array(size));
    },
  });
  return new Response(body, { status: 200, headers });
}

afterEach(() => vi.unstubAllGlobals());

const get = (url: string, maxBytes = 4096, headers: Record<string, string> = {}) => safeRequest({ url, method: 'GET', headers, maxBytes });

describe('safeRequest', () => {
  it('refuses a redirect to http://', async () => {
    stub(() => redirect('http://example.com/x'));
    await expect(get('https://example.com/a')).rejects.toThrow(/non-https/);
  });

  it('refuses a redirect to a non-public host', async () => {
    for (const to of ['https://127.0.0.1/x', 'https://localhost/x', 'https://intranet/x', 'https://[::1]/x']) {
      stub(() => redirect(to));
      await expect(get('https://example.com/a')).rejects.toThrow(/non-public host/);
    }
  });

  it('refuses a request URL that is not https or not public', async () => {
    stub(() => new Response('x'));
    await expect(get('http://example.com/a')).rejects.toThrow(/non-https/);
    await expect(get('https://10.0.0.1/a')).rejects.toThrow(/non-public/);
  });

  it('drops authorization on a cross-host redirect, without mutating the caller headers', async () => {
    const calls = stub((c) => (c.url.startsWith('https://registry.example.com') ? redirect('https://cdn.example.net/blob') : new Response('ok')));
    const headers = { authorization: 'Bearer t', accept: 'x' };
    await get('https://registry.example.com/v2/a', 4096, headers);
    expect(calls[0]!.headers.authorization).toBe('Bearer t');
    expect(calls[1]!.headers.authorization).toBeUndefined();
    expect(calls[1]!.headers.accept).toBe('x');
    expect(headers.authorization).toBe('Bearer t');
  });

  it('keeps authorization on a same-host redirect and compares against the previous hop', async () => {
    const calls = stub((c) => {
      if (c.url.endsWith('/a')) return redirect('https://cdn.example.net/b');
      if (c.url.endsWith('/b')) return redirect('https://registry.example.com/c');
      return new Response('ok');
    });
    await get('https://registry.example.com/a', 4096, { authorization: 'Bearer t' });
    // dropped at the first cross-host hop; not sent again when it returns
    expect(calls.map((c) => c.headers.authorization)).toEqual(['Bearer t', undefined, undefined]);
    const same = stub((c) => (c.url.endsWith('/a') ? redirect('/b') : new Response('ok')));
    await get('https://registry.example.com/a', 4096, { authorization: 'Bearer t' });
    expect(same[1]!.headers.authorization).toBe('Bearer t');
  });

  it('stops after the hop limit', async () => {
    const calls = stub(() => redirect('https://example.com/again'));
    await expect(get('https://example.com/a')).rejects.toThrow(/too many redirects/);
    expect(calls).toHaveLength(MAX_HOPS + 1);
  });

  it('enforces the byte cap on a chunked body with no content-length', async () => {
    stub(() => streamed(100_000));
    await expect(get('https://example.com/a', 4096)).rejects.toThrow(/over the 4096-byte cap/);
  });

  it('enforces the byte cap when content-length lies low', async () => {
    stub(() => streamed(100_000, { 'content-length': '10' }));
    await expect(get('https://example.com/a', 4096)).rejects.toThrow(/over the 4096-byte cap/);
  });

  it('rejects a declared content-length over the cap, and reads a body within it', async () => {
    stub(() => streamed(10, { 'content-length': '999999' }));
    await expect(get('https://example.com/a', 4096)).rejects.toThrow(/cap/);
    stub(() => streamed(2000));
    expect((await get('https://example.com/a', 4096)).body).toHaveLength(2000);
  });
});

describe('ociHttp', () => {
  it('goes through the same checks', async () => {
    stub(() => redirect('http://example.com/x'));
    await expect(ociHttp()({ url: 'https://ghcr.io/v2/a', method: 'GET', headers: {}, maxBytes: 100 })).rejects.toThrow(/non-https/);
    stub(() => streamed(100_000));
    await expect(ociHttp()({ url: 'https://ghcr.io/v2/a', method: 'GET', headers: {}, maxBytes: 100 })).rejects.toThrow(/cap/);
  });

  it('a token realm on a non-public host is refused', async () => {
    const http = async () => ({ status: 401, headers: { 'www-authenticate': 'Bearer realm="https://127.0.0.1/token"' }, body: new Uint8Array() });
    await expect(resolveOciDigest({ registry: 'ghcr.io', repository: 'a/b', digest: `sha256:${'a'.repeat(64)}` }, http)).rejects.toThrow(/not a public host/);
  });
});

describe('httpFetcher', () => {
  it('caps a chunked download, and refuses http redirects', async () => {
    stub(() => streamed(100_000));
    await expect(httpFetcher(4096).bytes('https://example.com/f')).rejects.toThrow(/cap/);
    stub(() => redirect('http://example.com/f'));
    await expect(httpFetcher().bytes('https://example.com/f')).rejects.toThrow(/non-https/);
    await expect(httpFetcher().json('https://example.com/f')).rejects.toThrow(/non-https/);
  });

  it('reads json and reports HTTP errors', async () => {
    stub(() => new Response('{"a":1}', { status: 200 }));
    expect(await httpFetcher().json('https://example.com/f')).toEqual({ a: 1 });
    stub(() => new Response('no', { status: 404 }));
    await expect(httpFetcher().json('https://example.com/f')).rejects.toThrow(/HTTP 404/);
  });
});

describe('isPublicHost', () => {
  it('accepts dotted names, refuses the rest', () => {
    expect(isPublicHost('ghcr.io')).toBe(true);
    expect(isPublicHost('registry.example.com:5000')).toBe(true);
    for (const h of ['localhost', 'intranet', '127.0.0.1', '[::1]', '10.0.0.1:80', 'a.b.1']) expect(isPublicHost(h)).toBe(false);
  });
});
