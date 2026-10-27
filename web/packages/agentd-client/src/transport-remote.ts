// The web transport to a remote loams-agentd (design §53 §6.3, D909; plan SD1
// Task 7). The browser calls `loams-agentd serve --remote` directly over TLS with
// the person's own short-lived access token for the audience `loams-agentd`; the
// daemon's own token never reaches a browser.
//
// - `getToken` is read on every request (tokens are short-lived and refreshed by
//   whoever holds them: the identity plugin, or a pasted development token).
// - Requests send no cookies (`credentials: 'omit'`) and no referrer, and never
//   follow a redirect, so the bearer only ever goes to the configured host.
//
// `createAgentdTransport({ kind: 'remote', … })` (Task 6's `connect.ts`) builds
// this transport; the options carry the `kind` so they pass straight through.

import { Code, ConnectError, type Transport } from '@connectrpc/connect';
import { createConnectTransport } from '@connectrpc/connect-web';

export type TokenGetter = () => string | undefined | Promise<string | undefined>;

export interface RemoteAgentdTransportOptions {
  kind: 'remote';
  /** The remote agentd's URL (`https://dev-box.example.ts.net`), from the host registry. */
  baseUrl: string;
  /** The person's access token for the audience `loams-agentd`, read per request. */
  getToken: TokenGetter;
  /** Tests: the fetch the transport uses. */
  fetch?: typeof globalThis.fetch;
  /** Binary by default; JSON for debugging. */
  useBinaryFormat?: boolean;
}

function isLoopback(hostname: string): boolean {
  return (
    hostname === 'localhost' ||
    hostname === '[::1]' ||
    /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(hostname)
  );
}

/**
 * `value` as an agentd base URL: `https:` (or `http:` on a loopback host, for local
 * development), with no credentials, query or fragment, and no trailing `/`.
 * Undefined when it is not one.
 */
export function agentdBaseUrl(value: string): string | undefined {
  let url: URL;
  try {
    url = new URL(value.trim());
  } catch {
    return undefined;
  }
  const secure =
    url.protocol === 'https:' || (url.protocol === 'http:' && isLoopback(url.hostname));
  if (!secure || url.username || url.password || url.search || url.hash || !url.hostname) {
    return undefined;
  }
  return `${url.origin}${url.pathname}`.replace(/\/+$/, '');
}

/** A fetch that adds `Authorization: Bearer <getToken()>` and keeps everything else home. */
export function remoteFetch(
  getToken: TokenGetter,
  base: typeof globalThis.fetch = (...args) => globalThis.fetch(...args),
): typeof globalThis.fetch {
  return async (input, init) => {
    const token = await getToken();
    if (!token) {
      throw new ConnectError(
        'No access token for this agentd host; sign in again',
        Code.Unauthenticated,
      );
    }
    const headers = new Headers(init?.headers);
    headers.set('authorization', `Bearer ${token}`);
    headers.delete('cookie');
    return base(input, {
      ...init,
      headers,
      credentials: 'omit',
      redirect: 'error',
      referrerPolicy: 'no-referrer',
    });
  };
}

/** The Connect transport to one remote agentd host. */
export function createRemoteAgentdTransport(options: RemoteAgentdTransportOptions): Transport {
  const baseUrl = agentdBaseUrl(options.baseUrl);
  if (!baseUrl) {
    throw new ConnectError(
      'An agentd host URL must be https:// (http:// only on loopback), with no credentials or query',
      Code.InvalidArgument,
    );
  }
  return createConnectTransport({
    baseUrl,
    useBinaryFormat: options.useBinaryFormat ?? true,
    fetch: remoteFetch(options.getToken, options.fetch),
  });
}
