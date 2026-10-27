// The factory in a browser (SF1 amendment F1-F6): its reads come from the selected
// agentd host's FactoryService, the daemon that holds the apps' credentials on its
// own machine. Studio provides this as `studio.factory`, for @loams/plugin-factory's
// pages and for its own sidebar. There is no embedding in a browser: an app opens in
// a new tab (F5), and its credentials are configured in Loams Desktop on the
// daemon's machine (F4).

import { type Client, createClient, type Transport } from '@connectrpc/connect';
import type {
  FactoryAppId,
  FactoryAppInfo,
  FactoryHealth,
  IpcResult,
  LoamsDesktopApi,
} from '@loams/desktop/contracts';
import { type FactoryApp, FactoryService, FactoryHealth as PbHealth } from '@loams/proto/agentd';
import { httpUrl } from './factory.js';

/** What `studio.factory` serves: the reads of `desktop.factory`, and a new tab. */
export type FactoryBridge = Pick<
  LoamsDesktopApi['factory'],
  'list' | 'configure' | 'test' | 'remove' | 'query' | 'openApp'
> & {
  /** False: the browser opens apps in a tab and cannot configure them. */
  readonly embeds: false;
};

/** Why a browser cannot store an app's credentials (SF1 amendment F4). */
export const CONFIGURE_ON_DESKTOP =
  "An app's credentials are stored by the agent daemon on its own machine: configure it in Loams Desktop there.";

export const NO_HOST =
  'The factory reads its apps through an agentd host. Connect one in Studio settings › Agentd hosts.';

const HEALTH: Record<number, FactoryHealth> = {
  [PbHealth.OK]: 'ok',
  [PbHealth.AUTH_FAILED]: 'auth_failed',
  [PbHealth.UNREACHABLE]: 'unreachable',
  [PbHealth.UNCONFIGURED]: 'unconfigured',
  [PbHealth.LOCKED]: 'locked',
};

/** A daemon app as the factory pages' `FactoryAppInfo`. */
export function appInfo(a: FactoryApp): FactoryAppInfo {
  const configured = a.url !== undefined;
  return {
    id: a.id as FactoryAppId,
    label: a.label,
    ...(configured ? { url: a.url } : {}),
    health: HEALTH[a.health] ?? 'unreachable',
    hasPanels: a.hasPanels,
    ...(a.preview ? { preview: a.preview } : {}),
    credentialFields: a.credentialFields.map((f) => ({
      key: f.key,
      label: f.label,
      secret: f.secret,
    })),
    ...(configured ? { fields: { ...a.fields } } : {}),
    persistent: a.persistent,
  };
}

const fail = (code: string, message: string) => ({ ok: false, code, message }) as const;

export function agentdFactoryBridge(
  transport: () => Transport | undefined,
  openTab: (url: string) => void,
): FactoryBridge {
  const apps = new Map<string, FactoryAppInfo>();
  const client = (): Client<typeof FactoryService> => {
    const t = transport();
    if (!t) throw new Error(NO_HOST);
    return createClient(FactoryService, t);
  };
  const reply = (r: {
    result:
      | { case: 'app'; value: FactoryApp }
      | { case: 'error'; value: { code: string; message: string } }
      | { case: undefined; value?: undefined };
  }): IpcResult<FactoryAppInfo> => {
    if (r.result.case === 'app') {
      const info = appInfo(r.result.value);
      apps.set(info.id, info);
      return { ok: true, value: info };
    }
    if (r.result.case === 'error') return fail(r.result.value.code, r.result.value.message);
    return fail('internal', 'The daemon gave no answer');
  };
  return {
    embeds: false,
    async list() {
      const r = await client().listFactoryApps({});
      apps.clear();
      return r.apps.map((a) => {
        const info = appInfo(a);
        apps.set(info.id, info);
        return info;
      });
    },
    async test(app) {
      try {
        return reply(await client().testFactoryApp({ app }));
      } catch (e) {
        return fail('unreachable', e instanceof Error ? e.message : String(e));
      }
    },
    async query<T = unknown>(q: {
      app: FactoryAppId;
      op: string;
      params: Record<string, unknown>;
    }) {
      try {
        const r = await client().queryFactory({
          app: q.app,
          op: q.op,
          paramsJson: JSON.stringify(q.params ?? {}),
        });
        if (r.result.case === 'valueJson')
          return { ok: true, value: JSON.parse(r.result.value) as T } as const;
        if (r.result.case === 'error') return fail(r.result.value.code, r.result.value.message);
        return fail('internal', 'The daemon gave no answer');
      } catch (e) {
        return fail('unreachable', e instanceof Error ? e.message : String(e));
      }
    },
    async configure() {
      return fail('desktop_only', CONFIGURE_ON_DESKTOP);
    },
    async remove() {
      throw new Error(CONFIGURE_ON_DESKTOP);
    },
    // A new tab on the app's own UI: Plane's App URL first (F5).
    async openApp(app) {
      const info = apps.get(app);
      const url = httpUrl(info?.fields?.appUrl || info?.url);
      if (!url) return fail('unconfigured', 'This app is not configured yet');
      openTab(url);
      return { ok: true, value: undefined };
    },
  };
}
