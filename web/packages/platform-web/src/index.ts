// @loams/platform-web: `platform` and `transport` in a browser (§37 §5.4).
//
// The engine-served console keeps the cookie session (§19 §6): requests go
// with `credentials: 'include'`. The hosted and BYOC console instead signs in
// with OIDC and passes `token`, which returns the access token held in memory;
// it is sent as `Authorization: Bearer` and cookies stay home (`credentials:
// 'omit'`), since the engine is another origin. For local development against
// `loams-apps-mock`, `devBearer` adds the mock's fake token; it is never set
// in a production build. `transport` lets the demo mode pass an in-memory
// mock instead of a network transport.

import type { Transport } from '@connectrpc/connect';
import { createConnectTransport } from '@connectrpc/connect-web';
import {
  localThemeStore,
  type PlatformService,
  type PluginModule,
  ThemeController,
} from '@loams/console-host';
import type { Context } from '@loams/cordis';

export interface WebPlatformOptions {
  /** The Connect base URL; default: this page's origin. */
  baseUrl?: string;
  /** Development only: `Authorization: Bearer <devBearer>` on every request. */
  devBearer?: string;
  /** The server SDK snippets name, when it is not `baseUrl` (a dev server's proxy target). */
  serverUrl?: string;
  /** Use this transport instead of the network (the demo mode). */
  transport?: Transport;
  /**
   * The current access token (OIDC or an API key), read on every request. When set,
   * requests carry it as a bearer and send no cookies.
   */
  token?: () => string | undefined | Promise<string | undefined>;
}

export function webFetch(
  devBearer?: string,
  token?: WebPlatformOptions['token'],
  csrf: () => string | undefined = () => undefined,
): typeof globalThis.fetch {
  return async (input, init) => {
    const headers = new Headers(init?.headers);
    const bearer = token ? await token() : devBearer;
    if (bearer) headers.set('authorization', `Bearer ${bearer}`);
    // The cookie session's CSRF token on everything but a GET (design §19 §6, API2
    // ruling 6/7.9). Connect sends unary calls as POST. A bearer needs none.
    const method = (
      init?.method ?? (input instanceof Request ? input.method : 'GET')
    ).toUpperCase();
    const csrfToken = token ? undefined : csrf();
    if (csrfToken && method !== 'GET') headers.set('x-csrf-token', csrfToken);
    return globalThis.fetch(input, {
      ...init,
      headers,
      credentials: token ? 'omit' : 'include',
    });
  };
}

export function createWebPlatform(options: WebPlatformOptions = {}): PluginModule {
  return {
    name: 'platform-web',
    apply(ctx: Context) {
      const baseUrl = options.baseUrl ?? globalThis.location?.origin;
      if (!baseUrl) throw new Error('platform-web: no baseUrl and no page origin');
      let csrf: string | undefined;
      const fetch = webFetch(options.devBearer, options.token, () => csrf);
      const platform: PlatformService = {
        kind: 'web',
        fetch,
        baseUrl,
        ...(options.serverUrl ? { serverUrl: options.serverUrl } : {}),
        setCsrfToken(token) {
          csrf = token;
        },
        async openExternal(url) {
          // Web links only: never `javascript:`, `data:` or a custom scheme.
          const { protocol } = new URL(url, baseUrl);
          if (protocol !== 'https:' && protocol !== 'http:') {
            throw new Error(`openExternal refuses ${protocol} URLs`);
          }
          globalThis.open?.(url, '_blank', 'noopener,noreferrer');
        },
        async notify({ title, body, route }) {
          if (typeof Notification === 'undefined') return;
          if (Notification.permission === 'default') await Notification.requestPermission();
          if (Notification.permission !== 'granted') return;
          const n = new Notification(title, { body });
          // A click brings the page forward and opens the route (CF1 Task 16, W15).
          if (route) {
            n.onclick = () => {
              globalThis.focus?.();
              globalThis.location.hash = route;
              n.close();
            };
          }
        },
        async clipboardWrite(text) {
          await navigator.clipboard.writeText(text);
        },
      };
      ctx.provide('platform', platform);
      // System, Dark or Light, kept in this browser (D1074).
      ctx.provide('theme', new ThemeController(localThemeStore()));
      ctx.provide(
        'transport',
        options.transport ?? createConnectTransport({ baseUrl, useBinaryFormat: true, fetch }),
      );
      // No `agentd.endpoint` yet: the browser reaches a remote agentd through the
      // host registry of SD1 Task 7, which provides it here (an `AgentdService` over
      // `createAgentdTransport({ kind: 'remote', … })`). Until then `agentd` says
      // "no host" and Studio shows "Studio needs a Loams server with agentd".
    },
  };
}
