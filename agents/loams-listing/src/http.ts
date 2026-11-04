// Bounded, https-only HTTP for `loams-listing verify` (OP2-B Task 6). Every URL
// comes from a community submission, so: https only (re-checked on every
// redirect hop), a public-looking host name (dotted, no IP literal, no
// localhost), a hop limit, credentials never sent to another host, and the
// body cap enforced while streaming (content-length is only a hint).

export const MAX_HOPS = 3;
const HOST_RE = /^(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z][a-z0-9-]*(?::\d{1,5})?$/;

/** `host[:port]`, lower-case, dotted, with an alphabetic top-level label: not an IP literal, `localhost` or a single-label name. */
export function isPublicHost(host: string): boolean {
  return HOST_RE.test(host);
}

export interface SafeRequest {
  url: string;
  method: 'GET' | 'HEAD';
  headers: Record<string, string>;
  maxBytes: number;
  timeoutMs?: number;
}

export interface SafeResponse {
  status: number;
  /** Lower-cased names. */
  headers: Record<string, string>;
  body: Uint8Array;
}

function checkUrl(target: string): URL {
  let url: URL;
  try {
    url = new URL(target);
  } catch {
    throw new Error(`not a URL: ${target}`);
  }
  if (url.protocol !== 'https:') throw new Error(`refusing a non-https URL: ${target}`);
  if (url.username || url.password) throw new Error(`refusing a URL with credentials: ${target}`);
  if (!isPublicHost(url.host)) throw new Error(`refusing a non-public host in ${target}`);
  return url;
}

async function readCapped(response: Response, maxBytes: number, url: string): Promise<Uint8Array> {
  const over = () => new Error(`${url}: over the ${maxBytes}-byte cap`);
  if (Number(response.headers.get('content-length') ?? '0') > maxBytes) {
    await response.body?.cancel().catch(() => undefined);
    throw over();
  }
  if (!response.body) return new Uint8Array();
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.length;
    if (total > maxBytes) {
      await reader.cancel().catch(() => undefined);
      throw over();
    }
    chunks.push(value);
  }
  const out = new Uint8Array(total);
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
}

/** One exchange, following at most MAX_HOPS redirects that are each https to a public host; `authorization` is dropped when the host changes. */
export async function safeRequest(request: SafeRequest): Promise<SafeResponse> {
  let target = request.url;
  let previousHost: string | undefined;
  let headers: Record<string, string> = { ...request.headers };
  for (let hop = 0; hop <= MAX_HOPS; hop++) {
    const url = checkUrl(target);
    if (previousHost !== undefined && url.host !== previousHost) {
      headers = Object.fromEntries(Object.entries(headers).filter(([k]) => k.toLowerCase() !== 'authorization'));
    }
    const response = await fetch(url, {
      method: request.method,
      headers: { ...headers, 'user-agent': 'loams-listing' },
      redirect: 'manual',
      signal: AbortSignal.timeout(request.timeoutMs ?? 60_000),
    });
    const location = response.headers.get('location');
    if (response.status >= 300 && response.status < 400 && location) {
      await response.body?.cancel().catch(() => undefined);
      previousHost = url.host;
      target = new URL(location, url).toString();
      continue;
    }
    const out: Record<string, string> = {};
    response.headers.forEach((v, k) => (out[k.toLowerCase()] = v));
    if (request.method === 'HEAD') {
      await response.body?.cancel().catch(() => undefined);
      return { status: response.status, headers: out, body: new Uint8Array() };
    }
    return { status: response.status, headers: out, body: await readCapped(response, request.maxBytes, target) };
  }
  throw new Error(`${request.url}: too many redirects`);
}
