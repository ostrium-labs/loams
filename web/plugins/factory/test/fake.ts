import type {
  FactoryAppId,
  FactoryAppInfo,
  FactoryHealth,
  FactoryQuery,
  IpcResult,
  LoamsDesktopApi,
} from '@loams/desktop/contracts';

const SSO = { key: 'ssoOrigin', label: 'SSO origin (optional)', secret: false };
const DEFS: Record<FactoryAppId, { label: string; fields: [string, string, boolean][] }> = {
  forgejo: { label: 'Forgejo', fields: [['token', 'Access token', true]] },
  zulip: {
    label: 'Zulip',
    fields: [
      ['email', 'Email', false],
      ['apiKey', 'API key', true],
    ],
  },
  plane: {
    label: 'Plane (ItsAPlan)',
    fields: [
      ['apiKey', 'API key', true],
      ['projectKey', 'Project key', false],
    ],
  },
  glitchtip: { label: 'GlitchTip', fields: [['token', 'API token', true]] },
  openpanel: {
    label: 'OpenPanel',
    fields: [
      ['clientId', 'Client ID', true],
      ['clientSecret', 'Client secret', true],
      ['projectId', 'Project ID', false],
    ],
  },
  matomo: {
    label: 'Matomo',
    fields: [
      ['apiToken', 'API token', true],
      ['idSite', 'Site ID', false],
    ],
  },
  langfuse: {
    label: 'Langfuse',
    fields: [
      ['publicKey', 'Public key', true],
      ['secretKey', 'Secret key', true],
    ],
  },
  openobserve: { label: 'OpenObserve', fields: [] },
};

export const IDS = Object.keys(DEFS) as FactoryAppId[];

export function info(
  id: FactoryAppId,
  health: FactoryHealth = 'unconfigured',
  persistent = true,
  fields?: Record<string, string>,
): FactoryAppInfo {
  const d = DEFS[id];
  return {
    id,
    label: d.label,
    url: health === 'unconfigured' ? undefined : `https://${id}.example.test`,
    health,
    hasPanels: id !== 'openobserve',
    credentialFields: [...d.fields.map(([key, label, secret]) => ({ key, label, secret })), SSO],
    ...(fields ? { fields } : {}),
    persistent,
  };
}

type Handler = (q: FactoryQuery) => IpcResult<unknown> | Promise<IpcResult<unknown>>;

/** A plain-object `desktop` service; `queries` answers `factory.query` per `app.op`. */
export function fakeDesktop(
  init: {
    apps?: FactoryAppInfo[];
    queries?: Record<string, Handler | unknown>;
    configure?: IpcResult<FactoryAppInfo>;
    tested?: FactoryAppInfo;
  } = {},
) {
  const calls: string[] = [];
  const queries: FactoryQuery[] = [];
  const configured: { app: string; url: string; fields: Record<string, string> }[] = [];
  const apps = init.apps ?? IDS.map((id) => info(id));
  const api = {
    factory: {
      list: async () => apps,
      configure: async (app: FactoryAppId, url: string, fields: Record<string, string>) => {
        calls.push(`configure:${app}`);
        configured.push({ app, url, fields });
        return init.configure ?? { ok: true, value: info(app, 'unreachable') };
      },
      test: async (app: FactoryAppId) => {
        calls.push(`test:${app}`);
        return init.tested ?? info(app, 'ok');
      },
      remove: async (app: FactoryAppId) => void calls.push(`remove:${app}`),
      query: async (q: FactoryQuery) => {
        queries.push(q);
        const h = init.queries?.[`${q.app}.${q.op}`];
        if (typeof h === 'function') return (h as Handler)(q);
        if (h === undefined) return { ok: false, code: 'unknown_op', message: `no ${q.op}` };
        return { ok: true, value: h };
      },
      openApp: async (app: FactoryAppId) => {
        calls.push(`open:${app}`);
        return { ok: false, code: 'not_ready', message: 'Opening apps is not available yet.' };
      },
      closeApp: async () => undefined,
    },
  } as unknown as LoamsDesktopApi;
  return { api, calls, queries, configured };
}
