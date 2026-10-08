// DEV ONLY: an in-browser fake of the Electron preload's `LoamsDesktopApi`,
// so the desktop edition can be previewed at `/ui/cordis.html?desktop`
// (main.tsx loads this behind `import.meta.env.DEV`; it must never reach a
// production bundle, and main.test.ts checks the guard).
//
// NOTE FOR LATER TASKS: each desktop plugin task that adds a namespace to
// LoamsDesktopApi (factory, update, shell, ...) also adds plausible fake data
// for it here, so the preview keeps working. Namespaces without data yet
// return empty or "unconfigured" results.

import type {
  EngineState,
  FactoryAppId,
  FactoryAppInfo,
  FactoryQuery,
  IpcResult,
  LoamsDesktopApi,
  ServerEntry,
} from '@loams/desktop/contracts';

const STORE = 'loams.fake-desktop.servers';
const ok: IpcResult<void> = { ok: true, value: undefined };

interface Saved {
  servers: ServerEntry[];
  activeId: string;
}

function initial(): Saved {
  return {
    servers: [
      { id: 'local', name: 'This computer', kind: 'local', url: globalThis.location.origin },
      { id: 'demo', name: 'Demo', kind: 'demo', url: 'http://127.0.0.1:8090' },
    ],
    activeId: 'demo',
  };
}

function load(): Saved {
  try {
    const raw = JSON.parse(localStorage.getItem(STORE) ?? 'null') as Saved | null;
    if (raw && Array.isArray(raw.servers) && raw.servers.some((s) => s.id === raw.activeId)) {
      return raw;
    }
  } catch {
    // Blocked storage or a stale shape: start from the defaults.
  }
  return initial();
}

function save(s: Saved) {
  try {
    localStorage.setItem(STORE, JSON.stringify(s));
  } catch {
    // Not persisted; the preview still works for this page load.
  }
}

const readyState = (): EngineState => ({
  phase: 'ready',
  url: 'http://127.0.0.1:8080',
  esUrl: 'http://127.0.0.1:9200',
  flightUrl: 'grpc://127.0.0.1:50051',
  durableUrl: 'http://127.0.0.1:8081',
  pid: 4242,
});

const SSO_FIELD = { key: 'ssoOrigin', label: 'SSO origin (optional)', secret: false };
const PREVIEW_FACTORY_APPS: {
  id: FactoryAppId;
  fields: [string, string, boolean][];
  label: string;
  hasPanels: boolean;
}[] = [
  { id: 'forgejo', fields: [['token', 'Access token', true]], label: 'Forgejo', hasPanels: true },
  {
    id: 'zulip',
    fields: [
      ['email', 'Email', false],
      ['apiKey', 'API key', true],
    ],
    label: 'Zulip',
    hasPanels: true,
  },
  {
    id: 'plane',
    fields: [
      ['apiKey', 'API key', true],
      ['projectKey', 'Project key', false],
    ],
    label: 'Plane (ItsAPlan)',
    hasPanels: true,
  },
  { id: 'glitchtip', fields: [['token', 'API token', true]], label: 'GlitchTip', hasPanels: true },
  {
    id: 'openpanel',
    fields: [
      ['clientId', 'Client ID', true],
      ['clientSecret', 'Client secret', true],
      ['projectId', 'Project ID', false],
    ],
    label: 'OpenPanel',
    hasPanels: true,
  },
  {
    id: 'matomo',
    fields: [
      ['apiToken', 'API token', true],
      ['idSite', 'Site ID', false],
    ],
    label: 'Matomo',
    hasPanels: true,
  },
  {
    id: 'langfuse',
    fields: [
      ['publicKey', 'Public key', true],
      ['secretKey', 'Secret key', true],
    ],
    label: 'Langfuse',
    hasPanels: true,
  },
  { id: 'openobserve', fields: [], label: 'OpenObserve', hasPanels: false },
];

/** Sample apps shown as configured in the preview. Every value is obviously fake. */
const PREVIEW_URLS: Partial<Record<FactoryAppId, string>> = {
  forgejo: 'https://git.demo.invalid',
  glitchtip: 'https://errors.demo.invalid',
  matomo: 'https://stats.demo.invalid',
};

const day = (n: number) => new Date(Date.now() - n * 86_400_000).toISOString();
const dateOnly = (n: number) => day(n).slice(0, 10);

/** Sample DTOs per `app.op`, in the shapes apps/desktop-electron/src/main/factory/ops.ts returns. */
const PREVIEW_DATA: Record<string, unknown> = {
  'forgejo.repos': [
    {
      fullName: 'demo/loams',
      description: 'Demo repository (sample data)',
      stars: 12,
      forks: 3,
      openIssues: 5,
      updatedAt: day(0),
    },
    {
      fullName: 'demo/console',
      description: 'Demo console UI (sample data)',
      stars: 4,
      forks: 1,
      openIssues: 2,
      updatedAt: day(2),
    },
    {
      fullName: 'demo/docs',
      description: 'Demo documentation (sample data)',
      stars: 1,
      forks: 0,
      openIssues: 0,
      updatedAt: day(9),
    },
  ],
  'forgejo.version': { version: '11.0.0-demo' },
  'forgejo.issues:pulls': [
    { id: '#14', title: 'Add demo factory panels', state: 'open', updatedAt: day(0) },
    { id: '#12', title: 'Bump demo dependencies', state: 'open', updatedAt: day(1) },
    { id: '#9', title: 'Fix demo flaky test', state: 'open', updatedAt: day(4) },
  ],
  'forgejo.issues': [
    {
      id: '#15',
      title: 'Demo: table overflows on narrow windows',
      state: 'open',
      updatedAt: day(1),
    },
    { id: '#11', title: 'Demo: document the sample data', state: 'open', updatedAt: day(3) },
  ],
  'glitchtip.organizations': [{ slug: 'demo-org', name: 'Demo Org' }],
  'glitchtip.issues': [
    {
      id: '101',
      title: 'TypeError: demo is undefined',
      level: 'error',
      count: '38',
      lastSeen: day(0),
    },
    { id: '102', title: 'Demo request timed out', level: 'warning', count: '12', lastSeen: day(1) },
    {
      id: '103',
      title: 'Demo: unhandled promise rejection',
      level: 'error',
      count: '3',
      lastSeen: day(3),
    },
  ],
  'matomo.visits': [6, 5, 4, 3, 2, 1, 0].map((n, i) => ({
    date: dateOnly(n),
    nb_visits: 120 + i * 9,
    nb_uniq_visitors: 90 + i * 7,
    nb_actions: 410 + i * 21,
    bounce_count: 40 + i,
  })),
  'matomo.pages': [
    { label: '/demo', hits: 210, visits: 160 },
    { label: '/demo/pricing', hits: 96, visits: 80 },
    { label: '/demo/docs', hits: 41, visits: 33 },
  ],
};

const fakeQuery = (q: FactoryQuery): IpcResult<unknown> => {
  if (!PREVIEW_URLS[q.app]) {
    return { ok: false, code: 'unconfigured', message: `${q.app} is not configured.` };
  }
  const key = `${q.app}.${q.op}`;
  const data = PREVIEW_DATA[q.params['type'] === 'pulls' ? `${key}:pulls` : key];
  return data === undefined
    ? { ok: false, code: 'unknown_op', message: `Unknown op ${q.op}.` }
    : { ok: true, value: data };
};

function previewInfo({ fields, ...a }: (typeof PREVIEW_FACTORY_APPS)[number]): FactoryAppInfo {
  const url = PREVIEW_URLS[a.id];
  return {
    ...a,
    url,
    health: url ? 'ok' : 'unconfigured',
    credentialFields: [
      ...fields.map(([key, label, secret]) => ({ key, label, secret })),
      SSO_FIELD,
    ],
    persistent: true,
  };
}

export function createFakeDesktop(): LoamsDesktopApi {
  const data = load();
  let engine: EngineState = readyState();
  const listeners = new Set<(s: EngineState) => void>();
  const setEngine = (s: EngineState) => {
    engine = s;
    for (const l of listeners) l(s);
  };
  const later = (ms: number, fn: () => void) => globalThis.setTimeout(fn, ms);

  return {
    version: '0.0.0-preview',
    platform: 'linux',
    servers: {
      list: async () => ({ servers: [...data.servers], activeId: data.activeId }),
      add: async (e) => {
        if (!/^https:\/\//.test(e.url) && !/^http:\/\/(127\.0\.0\.1|localhost)/.test(e.url)) {
          return {
            ok: false,
            code: 'insecure_url',
            message: 'Remote servers must use https:// (http is allowed for localhost only).',
          };
        }
        const entry: ServerEntry = { ...e, id: `srv_${Date.now().toString(36)}` };
        data.servers = [...data.servers, entry];
        save(data);
        return { ok: true, value: entry };
      },
      remove: async (id) => {
        if (id === data.activeId) {
          return { ok: false, code: 'active', message: 'Switch to another server first.' };
        }
        data.servers = data.servers.filter((s) => s.id !== id);
        save(data);
        return ok;
      },
      activate: async (id) => {
        if (!data.servers.some((s) => s.id === id)) {
          return { ok: false, code: 'not_found', message: 'No such server.' };
        }
        data.activeId = id;
        save(data);
        later(0, () => globalThis.location.reload());
        return ok;
      },
    },
    engine: {
      state: async () => engine,
      start: async () => {
        setEngine({ phase: 'starting', attempt: 1 });
        later(900, () => setEngine(readyState()));
      },
      stop: async () => setEngine({ phase: 'stopped' }),
      openLogs: async () => undefined,
      onState: (cb) => {
        listeners.add(cb);
        return () => void listeners.delete(cb);
      },
    },
    factory: {
      list: async (): Promise<FactoryAppInfo[]> => PREVIEW_FACTORY_APPS.map(previewInfo),
      configure: async (app, url) => {
        const def = PREVIEW_FACTORY_APPS.find((x) => x.id === app);
        if (!def) return { ok: false, code: 'unknown_app', message: 'Unknown app' };
        // The preview keeps the URL in memory only; no secret is stored.
        PREVIEW_URLS[app] = url;
        return { ok: true, value: previewInfo(def) };
      },
      test: async (app) =>
        previewInfo(
          PREVIEW_FACTORY_APPS.find((a) => a.id === app) as (typeof PREVIEW_FACTORY_APPS)[number],
        ),
      remove: async (app) => {
        delete PREVIEW_URLS[app];
      },
      query: (async (q: FactoryQuery) => fakeQuery(q)) as LoamsDesktopApi['factory']['query'],
      openApp: async () => ({
        ok: false,
        code: 'preview',
        message: 'Apps open in the desktop app, not in the preview.',
      }),
      closeApp: async () => undefined,
    },
    shell: {
      openExternal: async (url) => {
        globalThis.open(url, '_blank', 'noopener');
        return ok;
      },
      notify: async () => undefined,
      clipboardWrite: async (text) => {
        await navigator.clipboard?.writeText(text).catch(() => undefined);
      },
      onNavigate: () => () => undefined,
      takePendingNavigation: async () => null,
      setBadge: async () => undefined,
    },
    update: {
      state: async () => ({ phase: 'disabled' }),
      check: async () => undefined,
      download: async () => undefined,
      installAndRestart: async () => undefined,
    },
  };
}
