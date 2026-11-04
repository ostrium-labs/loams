// OCI registry access for `loams-listing verify` (OP2-B Task 6, D2606): resolve
// `<repo>@<digest>` through the registry's own API, with the anonymous Bearer
// token flow. The input is a community submission, so everything here is
// bounded: https only, no IP-literal or single-label hosts, capped bodies, a
// capped number of redirects (each re-checked), and no shell.
import { createHash } from 'node:crypto';
import { isPublicHost, safeRequest } from './http.js';

export const MAX_MANIFEST_BYTES = 4 * 1024 * 1024;
const MAX_TOKEN_BYTES = 64 * 1024;

export const MANIFEST_ACCEPT = [
  'application/vnd.oci.image.index.v1+json',
  'application/vnd.oci.image.manifest.v1+json',
  'application/vnd.docker.distribution.manifest.list.v2+json',
  'application/vnd.docker.distribution.manifest.v2+json',
].join(', ');

const INDEX_TYPES = ['application/vnd.oci.image.index.v1+json', 'application/vnd.docker.distribution.manifest.list.v2+json'];

export const DIGEST_RE = /^sha256:[0-9a-f]{64}$/;
const REPOSITORY_RE = /^[a-z0-9]+(?:[._-][a-z0-9]+)*(?:\/[a-z0-9]+(?:[._-][a-z0-9]+)*)*$/;

export interface OciRef {
  registry: string;
  repository: string;
  digest: string;
}

export interface HttpRequest {
  url: string;
  method: 'GET' | 'HEAD';
  headers: Record<string, string>;
  maxBytes: number;
}

export interface HttpResponse {
  status: number;
  /** Lower-cased names. */
  headers: Record<string, string | undefined>;
  body: Uint8Array;
}

/** One HTTP exchange with no redirects followed; `ociHttp()` is the network one, tests pass a fake. */
export type OciHttp = (request: HttpRequest) => Promise<HttpResponse>;

export interface Resolved {
  mediaType: string;
  /** `linux/amd64`-style, from an image index; empty for a single manifest. */
  platforms: string[];
}

/** `ghcr.io/acme/notes` → registry and repository; Docker Hub names map to registry-1.docker.io. Throws on anything odd. */
export function parseImage(image: string): { registry: string; repository: string } {
  if (image.includes('@') || /:[^/]*$/.test(image.split('/').slice(1).join('/')) || image.includes('://')) {
    throw new Error(`${image}: an image reference has no scheme, tag or digest`);
  }
  const parts = image.split('/');
  let registry: string;
  let rest: string[];
  if (parts.length > 1 && (parts[0]!.includes('.') || parts[0]!.includes(':'))) {
    registry = parts[0]!;
    rest = parts.slice(1);
  } else {
    registry = 'docker.io';
    rest = parts;
  }
  if (registry === 'docker.io' || registry === 'index.docker.io') {
    registry = 'registry-1.docker.io';
    if (rest.length === 1) rest = ['library', rest[0]!];
  }
  if (!isPublicHost(registry)) throw new Error(`${image}: ${registry} is not a registry host name`);
  const repository = rest.join('/');
  if (!REPOSITORY_RE.test(repository)) throw new Error(`${image}: ${repository} is not a valid repository name`);
  return { registry, repository };
}

function sha256Of(bytes: Uint8Array): string {
  return `sha256:${createHash('sha256').update(bytes).digest('hex')}`;
}

function parseChallenge(header: string | undefined): { realm: string; service?: string; scope?: string } | undefined {
  if (!header || !/^bearer\s/i.test(header)) return undefined;
  const params: Record<string, string> = {};
  for (const m of header.slice(7).matchAll(/([a-z_]+)="([^"]*)"/gi)) params[m[1]!.toLowerCase()] = m[2]!;
  return params.realm ? { realm: params.realm, service: params.service, scope: params.scope } : undefined;
}

function text(bytes: Uint8Array): string {
  return new TextDecoder().decode(bytes);
}

async function bearerToken(http: OciHttp, challenge: { realm: string; service?: string; scope?: string }, repository: string): Promise<string> {
  const url = new URL(challenge.realm);
  if (url.protocol !== 'https:') throw new Error(`the registry's token realm is not https: ${challenge.realm}`);
  if (!isPublicHost(url.host)) throw new Error(`the registry's token realm is not a public host: ${challenge.realm}`);
  if (challenge.service) url.searchParams.set('service', challenge.service);
  url.searchParams.set('scope', challenge.scope ?? `repository:${repository}:pull`);
  const response = await http({ url: url.toString(), method: 'GET', headers: { accept: 'application/json' }, maxBytes: MAX_TOKEN_BYTES });
  if (response.status !== 200) throw new Error(`the registry's token service answered HTTP ${response.status}`);
  const doc = JSON.parse(text(response.body)) as { token?: string; access_token?: string };
  const token = doc.token ?? doc.access_token;
  if (!token) throw new Error("the registry's token service returned no token");
  return token;
}

/**
 * Resolves `<registry>/<repository>@<digest>`: HEAD the manifest (anonymous Bearer token
 * when the registry asks for one), and for an image index GET it, check its bytes hash to
 * the digest, and list its platforms. Throws when the manifest is absent or the registry
 * answers another digest.
 */
export async function resolveOciDigest(ref: OciRef, http: OciHttp): Promise<Resolved> {
  if (!DIGEST_RE.test(ref.digest)) throw new Error(`${ref.digest} is not sha256:<64 lowercase hex>`);
  if (!isPublicHost(ref.registry) || !REPOSITORY_RE.test(ref.repository)) throw new Error(`${ref.registry}/${ref.repository} is not a registry reference`);
  const url = `https://${ref.registry}/v2/${ref.repository}/manifests/${ref.digest}`;
  let auth: string | undefined;
  const call = async (method: 'GET' | 'HEAD', maxBytes: number): Promise<HttpResponse> => {
    const request = (): Promise<HttpResponse> =>
      http({ url, method, headers: { accept: MANIFEST_ACCEPT, ...(auth ? { authorization: `Bearer ${auth}` } : {}) }, maxBytes });
    let response = await request();
    if (response.status === 401 && !auth) {
      const challenge = parseChallenge(response.headers['www-authenticate']);
      if (!challenge) throw new Error(`${url}: HTTP 401 without a Bearer challenge`);
      auth = await bearerToken(http, challenge, ref.repository);
      response = await request();
    }
    return response;
  };
  const head = await call('HEAD', 0);
  if (head.status === 404) throw new Error(`${ref.registry}/${ref.repository}@${ref.digest}: no such manifest (HTTP 404)`);
  if (head.status !== 200) throw new Error(`${url}: HTTP ${head.status}`);
  const mediaType = (head.headers['content-type'] ?? '').split(';')[0]!.trim();
  const claimed = head.headers['docker-content-digest'];
  if (claimed && claimed !== ref.digest) throw new Error(`the registry answered digest ${claimed}, not ${ref.digest}`);
  if (!INDEX_TYPES.includes(mediaType)) return { mediaType, platforms: [] };
  const get = await call('GET', MAX_MANIFEST_BYTES);
  if (get.status !== 200) throw new Error(`${url}: HTTP ${get.status} reading the index`);
  const actual = sha256Of(get.body);
  if (actual !== ref.digest) throw new Error(`the index bytes hash to ${actual}, not ${ref.digest}`);
  const index = JSON.parse(text(get.body)) as { manifests?: { platform?: { os?: string; architecture?: string } }[] };
  const platforms = (index.manifests ?? [])
    .map((m) => m.platform)
    .filter((p): p is { os: string; architecture: string } => !!p?.os && !!p.architecture)
    .map((p) => `${p.os}/${p.architecture}`);
  return { mediaType, platforms };
}

/** The network `OciHttp`: https only (each redirect re-checked), a streamed body cap, no cookies or credentials. */
export function ociHttp(): OciHttp {
  return (request) => safeRequest(request);
}
