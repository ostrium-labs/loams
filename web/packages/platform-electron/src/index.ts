// @loams/platform-electron: `platform`, `transport` and `desktop` in the
// Electron shell (AP1e). The console runs at `loams-app://console`; the main
// process proxies `/v1`, `/api` etc. to the active server, so the base URL is
// this page's origin. The three side effects go over IPC (`window.loamsDesktop`).

import type { Interceptor } from '@connectrpc/connect';
import { createConnectTransport } from '@connectrpc/connect-web';
import type { PlatformService, PluginModule } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import type { LoamsDesktopApi } from '@loams/desktop/contracts';

export type { LoamsDesktopApi };

declare module '@loams/console-host' {
  interface Services {
    /** The desktop bridge; only desktop plugins (`inject: ['desktop']`) get it. */
    desktop: LoamsDesktopApi;
  }
}

/** apps-mock's dev token (crates/loams-apps-mock/src/auth.rs); demo server only. */
export const DEMO_BEARER = 'mock-access-usr_omar';

const GET_INSTANCE = { service: 'loams.instance.v1.InstanceService', method: 'GetInstance' };

/** The active server's kind, read once: a server switch reloads the window. */
function activeKind(api: LoamsDesktopApi) {
  let cached: Promise<string | undefined> | undefined;
  return () => {
    cached ??= api.servers
      .list()
      .then(({ servers, activeId }) => servers.find((s) => s.id === activeId)?.kind)
      .catch(() => undefined);
    return cached;
  };
}

export function desktopFetch(
  api: LoamsDesktopApi,
  kind: () => Promise<string | undefined> = activeKind(api),
): typeof globalThis.fetch {
  return async (input, init) => {
    const headers = new Headers(
      init?.headers ?? (input instanceof Request ? input.headers : undefined),
    );
    // Only the demo server (apps-mock) wants a fake bearer; never local or remote.
    if ((await kind()) === 'demo') headers.set('authorization', `Bearer ${DEMO_BEARER}`);
    return globalThis.fetch(input, { ...init, headers, credentials: 'include' });
  };
}

/** Merges `features.desktop` (always) and `features.local` (local engine) into GetInstance. */
export function instanceFeaturesInterceptor(kind: () => Promise<string | undefined>): Interceptor {
  return (next) => async (req) => {
    const res = await next(req);
    if (
      req.service.typeName === GET_INSTANCE.service &&
      req.method.name === GET_INSTANCE.method &&
      !res.stream
    ) {
      const message = res.message as { features?: Record<string, boolean> };
      message.features = {
        ...message.features,
        desktop: true,
        ...((await kind()) === 'local' ? { local: true } : {}),
      };
    }
    return res;
  };
}

export function createElectronPlatform(api: LoamsDesktopApi): PluginModule {
  return {
    name: 'platform-electron',
    apply(ctx: Context) {
      const baseUrl = globalThis.location?.origin;
      if (!baseUrl) throw new Error('platform-electron: no page origin');
      const kind = activeKind(api);
      const fetch = desktopFetch(api, kind);
      const platform: PlatformService = {
        kind: 'desktop',
        fetch,
        baseUrl,
        async openExternal(url) {
          const result = await api.shell.openExternal(url);
          if (!result.ok) throw new Error(result.message);
        },
        async notify({ title, body }) {
          await api.shell.notify(title, body);
        },
        clipboardWrite: (text) => api.shell.clipboardWrite(text),
      };
      ctx.provide('platform', platform);
      ctx.provide(
        'transport',
        createConnectTransport({
          baseUrl,
          useBinaryFormat: true,
          fetch,
          interceptors: [instanceFeaturesInterceptor(kind)],
        }),
      );
      ctx.provide('desktop', api);
    },
  };
}

/**
 * Deep links (`loams://open/console/<path>`): subscribe first, then take a link
 * that arrived before the page was ready, so none is lost or doubled.
 */
export async function wireDeepLinks(
  api: LoamsDesktopApi,
  navigate: (path: string) => void = (path) => {
    globalThis.location.hash = path.startsWith('#')
      ? path
      : `#${path.startsWith('/') ? path : `/${path}`}`;
  },
): Promise<() => void> {
  const off = api.shell.onNavigate(navigate);
  const pending = await api.shell.takePendingNavigation();
  if (pending) navigate(pending);
  return off;
}
