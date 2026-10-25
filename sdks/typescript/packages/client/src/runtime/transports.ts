// Transports (design §44 §4, D612).
//
// One port serves the Connect protocol, gRPC and gRPC-Web (D600), so the only
// question a client has is which of those it can speak:
//
// - **Browsers** cannot do HTTP/2 gRPC. They speak gRPC-Web and Connect, both
//   over `fetch`, which is what `@connectrpc/connect-web` implements. Because
//   the API serves gRPC-Web on the same port as everything else, the same
//   client code runs in a browser with no proxy and no sidecar.
// - **Node and Bun** can do HTTP/2 gRPC. `@connectrpc/connect-node`
//   implements it over `node:http2`, and the server's plaintext listener means
//   no ALPN to negotiate — the client asks for HTTP/2 with prior knowledge.
//
// The default is the `fetch` transport, deliberately: it is the one case where
// the *same* code path works everywhere, so it is what an application gets
// unless it asks for the Node one through `@loams/client/node`. Choosing the
// HTTP/2 transport in a browser bundle would be a build error, which is the
// right failure.

import { createConnectTransport } from '@connectrpc/connect-web';
import type { Interceptor, Transport } from '@connectrpc/connect';

/** What a caller may configure on the transport. */
export interface TransportOptions {
  /**
   * The instance's base URL. `Loams` sets this per call from its own
   * `endpoint`, so a caller building a transport by hand rarely needs it.
   */
  baseUrl?: string;
  /**
   * Extra headers on every request, unary and streaming: for a gateway, a
   * proxy, or a routing header such as the Live app a call is for. They are
   * attached by an interceptor (`headersInterceptor`), because neither
   * connect-web's nor connect-node's transport has a `headers` option; a
   * header the call already sets (a credential, a per-call header) wins.
   */
  headers?: Record<string, string>;
  /**
   * `useBinaryFormat` picks the encoding: `true` is protobuf, `false` is the
   * proto3 JSON mapping. Default `true`. JSON is what `curl` sends
   * (design §44 §4) and what a proxy that logs bodies can read; binary is
   * smaller and does not need a JSON codec on the wire.
   */
  useBinaryFormat?: boolean;
  /**
   * The deadline for a call that does not set its own, passed to the
   * transport as `defaultTimeoutMs`. A per-call `timeoutMs` still wins.
   */
  defaultTimeoutMs?: number;
  /**
   * CORS preflights for the browser: custom headers and the credential mode.
   * A browser sending `Authorization` needs the server's CORS policy to allow
   * it; this only tells the transport what the request will look like.
   */
  interceptors?: Parameters<typeof createConnectTransport>[0]['interceptors'];
  /**
   * The `fetch` the transport calls, for a runtime whose `fetch` needs
   * wrapping (a proxy, a test double, instrumentation). When set it is used
   * as given. When unset, the global `fetch` is used, wrapped to send
   * `redirect: "manual"` in Cloudflare Workers (see `workersFetch`).
   */
  fetch?: typeof globalThis.fetch;
}

/** True in Cloudflare Workers (workerd), which identifies itself this way. */
export function isWorkerd(): boolean {
  return typeof navigator !== 'undefined' && navigator.userAgent === 'Cloudflare-Workers';
}

/**
 * Wraps `fetch` to send `redirect: "manual"`.
 *
 * connect-web sets `redirect: "error"` on every request, which Workers rejects
 * ("Invalid redirect value, must be one of 'follow' or 'manual'"). Loams calls
 * never redirect, so `manual` loses nothing there; it is applied only in
 * workerd because in a browser `manual` turns a redirect into an opaque
 * response that hides the failure `error` reports.
 */
export function workersFetch(inner: typeof globalThis.fetch): typeof globalThis.fetch {
  return (input, init) => inner(input, init === undefined ? init : { ...init, redirect: 'manual' });
}

/** The `fetch` a transport built from `options` uses, or undefined for connect-web's default. */
export function transportFetch(options: TransportOptions): typeof globalThis.fetch | undefined {
  if (options.fetch !== undefined) {
    return options.fetch;
  }
  // Resolve the global lazily so a later polyfill or test stub is honoured.
  return isWorkerd() ? workersFetch((input, init) => globalThis.fetch(input, init)) : undefined;
}

/**
 * The default transport: Connect over `fetch`.
 *
 * Works in a browser, in Node 18+ (which has `fetch`) and in Bun, and
 * speaks both the Connect unary protocol and gRPC-Web, which is what the server
 * serves on the same port (D600). This is the browser path as a first-class
 * case, not a fallback: it is the default in every runtime.
 */
export function createLoamsTransport(options: TransportOptions = {}): Transport {
  const interceptors = transportInterceptors(options);
  const fetchImpl = transportFetch(options);
  return createConnectTransport({
    baseUrl: options.baseUrl ?? '',
    useBinaryFormat: options.useBinaryFormat ?? true,
    ...(options.defaultTimeoutMs === undefined
      ? {}
      : { defaultTimeoutMs: options.defaultTimeoutMs }),
    ...(fetchImpl === undefined ? {} : { fetch: fetchImpl }),
    ...(interceptors.length === 0 ? {} : { interceptors }),
  });
}

/**
 * An interceptor that sets `headers` on every request, unary and streaming.
 *
 * A header already on the request is left alone, so a per-call header or the
 * credential the invoker attaches is never overwritten by a static default.
 */
export function headersInterceptor(headers: Record<string, string>): Interceptor {
  const entries = Object.entries(headers);
  return (next) => (request) => {
    for (const [name, value] of entries) {
      if (!request.header.has(name)) {
        request.header.set(name, value);
      }
    }
    return next(request);
  };
}

/**
 * The interceptors a transport built from `options` runs: the static headers
 * first (outermost), then the caller's own, so a caller's interceptor sees the
 * headers and may still change them.
 */
export function transportInterceptors(options: TransportOptions): Interceptor[] {
  const headers =
    options.headers === undefined || Object.keys(options.headers).length === 0
      ? []
      : [headersInterceptor(options.headers)];
  return [...headers, ...(options.interceptors ?? [])];
}
