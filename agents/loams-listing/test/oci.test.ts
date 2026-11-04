import { describe, expect, it } from 'vite-plus/test';
import { indexBody, INDEX_TYPE } from './helpers.js';
import { type HttpRequest, type HttpResponse, type OciHttp, parseImage, resolveOciDigest } from '../src/oci.js';

const enc = (s: string) => new TextEncoder().encode(s);
describe('resolveOciDigest', () => {
  const ref = (digest: string) => ({ registry: 'ghcr.io', repository: 'acme/notes', digest });

  it('answers a HEAD for a single manifest', async () => {
    const digest = `sha256:${'a'.repeat(64)}`;
    const seen: HttpRequest[] = [];
    const http: OciHttp = async (r) => {
      seen.push(r);
      return { status: 200, headers: { 'content-type': 'application/vnd.oci.image.manifest.v1+json', 'docker-content-digest': digest }, body: new Uint8Array() };
    };
    expect(await resolveOciDigest(ref(digest), http)).toEqual({ mediaType: 'application/vnd.oci.image.manifest.v1+json', platforms: [] });
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({ method: 'HEAD', url: `https://ghcr.io/v2/acme/notes/manifests/${digest}` });
    expect(seen[0]!.headers.accept).toContain('application/vnd.docker.distribution.manifest.list.v2+json');
  });

  it('follows the anonymous Bearer token flow', async () => {
    const { body, digest } = indexBody(['linux/amd64', 'linux/arm64']);
    const calls: HttpRequest[] = [];
    const http: OciHttp = async (r) => {
      calls.push(r);
      if (r.url.startsWith('https://ghcr.io/token')) return { status: 200, headers: {}, body: enc('{"token":"t0k"}') };
      if (r.headers.authorization !== 'Bearer t0k') {
        return { status: 401, headers: { 'www-authenticate': 'Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:acme/notes:pull"' }, body: new Uint8Array() };
      }
      return { status: 200, headers: { 'content-type': INDEX_TYPE }, body: r.method === 'GET' ? enc(body) : new Uint8Array() };
    };
    const out = await resolveOciDigest(ref(digest), http);
    expect(out.platforms).toEqual(['linux/amd64', 'linux/arm64']);
    const token = calls.find((c) => c.url.includes('/token'))!;
    expect(token.url).toContain('service=ghcr.io');
    expect(token.url).toContain('scope=repository%3Aacme%2Fnotes%3Apull');
    expect(token.headers.authorization).toBeUndefined();
  });

  it('404 is an error', async () => {
    const http: OciHttp = async () => ({ status: 404, headers: {}, body: new Uint8Array() });
    await expect(resolveOciDigest(ref(`sha256:${'b'.repeat(64)}`), http)).rejects.toThrow(/no such manifest/);
  });

  it('an index whose bytes do not hash to the digest is refused', async () => {
    const { body } = indexBody(['linux/amd64']);
    const http: OciHttp = async (r): Promise<HttpResponse> => ({ status: 200, headers: { 'content-type': INDEX_TYPE }, body: r.method === 'GET' ? enc(body) : new Uint8Array() });
    await expect(resolveOciDigest(ref(`sha256:${'c'.repeat(64)}`), http)).rejects.toThrow(/hash to/);
  });

  it('a token realm that is not https is refused', async () => {
    const http: OciHttp = async () => ({ status: 401, headers: { 'www-authenticate': 'Bearer realm="http://evil.example/token"' }, body: new Uint8Array() });
    await expect(resolveOciDigest(ref(`sha256:${'d'.repeat(64)}`), http)).rejects.toThrow(/not https/);
  });

  it('a malformed digest is refused before any request', async () => {
    const http: OciHttp = async () => {
      throw new Error('called');
    };
    await expect(resolveOciDigest(ref('sha256:ABC'), http)).rejects.toThrow(/sha256/);
  });
});

describe('parseImage', () => {
  it('splits registry and repository', () => {
    expect(parseImage('ghcr.io/acme/notes')).toEqual({ registry: 'ghcr.io', repository: 'acme/notes' });
    expect(parseImage('ubuntu')).toEqual({ registry: 'registry-1.docker.io', repository: 'library/ubuntu' });
    expect(parseImage('docker.io/acme/notes')).toEqual({ registry: 'registry-1.docker.io', repository: 'acme/notes' });
  });
  it.each(['ghcr.io/acme/notes:1.0', 'ghcr.io/acme/notes@sha256:abc', 'https://ghcr.io/acme/notes', '127.0.0.1/acme/notes', 'localhost:5000/acme/notes', 'ghcr.io/Acme/../x'])('refuses %s', (image) => {
    expect(() => parseImage(image)).toThrow();
  });
});
