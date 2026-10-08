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
  ChatApproval,
  ChatEvent,
  ChatPart,
  ChatProviderId,
  ChatProviderInfo,
  ChatRecord,
  ChatSummary,
  ChatView,
  ConnectorDetail,
  ConnectorSummary,
  EngineState,
  FactoryAppId,
  FactoryAppInfo,
  FactoryQuery,
  IpcResult,
  LoamsDesktopApi,
  ServerEntry,
  SqlResult,
  StackId,
  StackState,
} from '@loams/desktop/contracts';
import { validateConfig } from '@loams/desktop/validate';

// The generated connector catalog (apps/desktop-electron/scripts/connectors-catalog.mjs), the same
// JSON the desktop app serves. Loaded lazily and only here, so it never reaches a production bundle.
const catalogFiles = import.meta.glob(
  '../../../../../apps/desktop-electron/resources/connectors.json',
  {
    import: 'default',
  },
) as Record<
  string,
  () => Promise<{ connectors: ConnectorSummary[]; details: Record<string, ConnectorDetail> }>
>;
let catalogJson: ReturnType<(typeof catalogFiles)[string]> | undefined;
function previewCatalog() {
  const load = Object.values(catalogFiles)[0];
  if (!load) {
    return Promise.reject(
      new Error('connectors.json is missing: run `pnpm --filter @loams/desktop catalog`.'),
    );
  }
  catalogJson ??= load();
  return catalogJson;
}

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

/** Non-secret values the preview keeps in memory, by app. */
const PREVIEW_FIELDS: Partial<Record<FactoryAppId, Record<string, string>>> = {
  matomo: { idSite: '1' },
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

/** What a configured app with no sample data returns: empty lists, never an error. */
const EMPTY: Record<string, unknown> = {
  'plane.stats': {},
  'openpanel.insights': { summary: {}, series: [], topPages: [] },
  'langfuse.daily': { available: true, days: [] },
  'forgejo.version': {},
  'zulip.server': {},
};
const LIST_OPS = new Set([
  'forgejo.repos',
  'forgejo.issues',
  'zulip.streams',
  'zulip.messages',
  'plane.issues',
  'glitchtip.organizations',
  'glitchtip.issues',
  'matomo.visits',
  'matomo.pages',
  'langfuse.traces',
]);

const fakeQuery = (q: FactoryQuery): IpcResult<unknown> => {
  if (!PREVIEW_URLS[q.app]) {
    return { ok: false, code: 'unconfigured', message: `${q.app} is not configured.` };
  }
  const key = `${q.app}.${q.op}`;
  const data = PREVIEW_DATA[q.params['type'] === 'pulls' ? `${key}:pulls` : key];
  if (data === undefined && key in EMPTY) return { ok: true, value: EMPTY[key] };
  if (data === undefined && LIST_OPS.has(key)) return { ok: true, value: [] };
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
    ...(url ? { fields: PREVIEW_FIELDS[a.id] ?? {} } : {}),
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
    version: '0.1.0-preview',
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
      configure: async (app, url, fields) => {
        const def = PREVIEW_FACTORY_APPS.find((x) => x.id === app);
        if (!def) return { ok: false, code: 'unknown_app', message: 'Unknown app' };
        // The preview keeps the URL in memory only; no secret is stored.
        PREVIEW_URLS[app] = url;
        const plain = def.fields.filter(([, , secret]) => !secret).map(([k]) => k);
        PREVIEW_FIELDS[app] = {
          ...PREVIEW_FIELDS[app],
          ...Object.fromEntries(Object.entries(fields).filter(([k, v]) => plain.includes(k) && v)),
        };
        return { ok: true, value: previewInfo(def) };
      },
      test: async (app) => ({
        ok: true,
        value: previewInfo(
          PREVIEW_FACTORY_APPS.find((a) => a.id === app) as (typeof PREVIEW_FACTORY_APPS)[number],
        ),
      }),
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
      // The preview has no native view: fill the page's placeholder instead.
      showEmbedded: async () => {
        const el = document.querySelector('[data-embedded-placeholder]');
        if (el) el.textContent = 'Embedded view appears in the desktop app';
        return ok;
      },
      hideEmbedded: async () => {
        const el = document.querySelector('[data-embedded-placeholder]');
        if (el) el.textContent = '';
      },
      reloadEmbedded: async () => undefined,
      popOut: async () => ({
        ok: false,
        code: 'preview',
        message: 'Apps open in the desktop app, not in the preview.',
      }),
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
    connectors: {
      catalog: async () => (await previewCatalog()).connectors,
      get: async (id) => {
        const d = Object.hasOwn((await previewCatalog()).details, id)
          ? (await previewCatalog()).details[id]
          : undefined;
        return d
          ? { ok: true, value: d }
          : { ok: false, code: 'not_found', message: `No connector "${id}".` };
      },
      saveYaml: async (name, text) => {
        // The preview has no save dialog: download the file instead.
        const url = URL.createObjectURL(new Blob([text], { type: 'text/yaml' }));
        const a = document.createElement('a');
        a.href = url;
        a.download = `${name}.yaml`;
        a.click();
        URL.revokeObjectURL(url);
        return { ok: true, value: { saved: true } };
      },
      validate: async (id, config) => {
        const all = (await previewCatalog()).details;
        const d = Object.hasOwn(all, id) ? all[id] : undefined;
        const secrets =
          d && Array.isArray(d.manifest.secrets) ? (d.manifest.secrets as string[]) : [];
        return d
          ? { ok: true, value: validateConfig(d.schema, config, secrets) }
          : { ok: false, code: 'not_found', message: `No connector "${id}".` };
      },
    },
    update: previewUpdate(),
    stacks: previewStacks(),
    chat: createFakeChat(),
    // FAKE sample data for previews; no database is contacted.
    pg: {
      tenants: async () => ({ ok: true, value: ['f4k3'.repeat(8)] }),
      timelines: async () => ({
        ok: true,
        value: [
          {
            timelineId: 'a1b2'.repeat(8),
            name: 'main (fake)',
            lastRecordLsn: '0/1A2B3C4',
            state: 'Active',
          },
          {
            timelineId: 'c3d4'.repeat(8),
            name: 'feature-x (fake)',
            ancestorTimelineId: 'a1b2'.repeat(8),
            ancestorLsn: '0/16B3748',
            lastRecordLsn: '0/16B4000',
            state: 'Active',
          },
          {
            timelineId: '9a4b'.repeat(8),
            name: 'feature-x-wip (fake)',
            ancestorTimelineId: 'c3d4'.repeat(8),
            ancestorLsn: '0/16B3F00',
            lastRecordLsn: '0/16B3FF8',
            state: 'Active',
          },
          {
            timelineId: '5e6f'.repeat(8),
            name: 'hotfix (fake)',
            ancestorTimelineId: 'a1b2'.repeat(8),
            ancestorLsn: '0/17F0A00',
            lastRecordLsn: '0/18200B8',
            state: 'Active',
          },
        ],
      }),
      createBranch: async (_t, b) => ({
        ok: true,
        value: {
          timelineId: 'e5f6'.repeat(8),
          name: b.name,
          ancestorTimelineId: b.ancestorTimelineId,
          lastRecordLsn: '0/16B4000',
          state: 'Active',
        },
      }),
      walStatus: async (_t, tl) => ({
        ok: true,
        value: { timelineId: tl, flushLsn: '0/1A2B3C4', commitLsn: '0/1A2B3C4' },
      }),
      connection: async () => ({
        host: '127.0.0.1',
        port: 55433,
        database: 'postgres',
        user: 'cloud_admin',
        passwordRef: 'fake-pg-ref',
      }),
      revealPassword: async () => 'fake-password',
      query: async (sql) =>
        fakeSql(sql) ?? {
          ok: true,
          value: {
            columns: ['id', 'name'],
            rows: [
              [1, 'sample row (fake)'],
              [2, 'another row (fake)'],
            ],
            rowCount: 2,
            truncated: false,
            elapsedMs: 3,
          },
        },
    },
    wesql: {
      connection: async () => ({
        host: '127.0.0.1',
        port: 13306,
        database: '',
        user: 'root',
        passwordRef: 'fake-wesql-ref',
      }),
      revealPassword: async () => 'fake-password',
      schemas: async () => ({
        ok: true,
        value: [{ name: 'information_schema' }, { name: 'shop (fake)' }],
      }),
      tables: async () => ({
        ok: true,
        value: [
          { name: 'orders', engine: 'InnoDB', rows: 2 },
          { name: 'order_totals', engine: '', rows: 0 },
        ],
      }),
      query: async (sql) =>
        fakeSql(sql) ?? {
          ok: true,
          value: {
            columns: ['id', 'total'],
            rows: [
              [1, '19.90'],
              [2, '5.00'],
            ],
            rowCount: 2,
            truncated: false,
            elapsedMs: 4,
          },
        },
    },
  };
}

/** Sample failures and big results for the SQL consoles; `undefined` falls through to the default rows. */
function fakeSql(sql: string): IpcResult<SqlResult> | undefined {
  if (/\bboom\b/i.test(sql)) {
    return { ok: false, code: '42601', message: 'syntax error at or near "boom" (sample error)' };
  }
  if (/^\s*(insert|update|delete|create|drop|alter|truncate)\b/i.test(sql)) {
    return {
      ok: true,
      value: { columns: [], rows: [], rowCount: 1, truncated: false, elapsedMs: 3 },
    };
  }
  if (/\bmany\b/i.test(sql)) {
    const rows = Array.from({ length: 1000 }, (_, i) => [
      i + 1,
      `sample-${i + 1}`,
      i % 7 === 0 ? null : (i * 37) % 101,
    ]);
    return {
      ok: true,
      value: {
        columns: ['id', 'name', 'score'],
        rows,
        rowCount: 1000,
        truncated: true,
        elapsedMs: 4,
      },
    };
  }
  return undefined;
}

// ---- Overview and Settings preview (Task 30) ----

/** The preview's stacks: Postgres running, WeSQL stopped, TiKV without a runtime. Start and stop work. */
function previewStacks(): LoamsDesktopApi['stacks'] {
  const states: Record<StackId, StackState> = {
    postgres: {
      phase: 'running',
      services: [{ name: 'postgres', state: 'running', ports: ['127.0.0.1:5432'] }],
    },
    wesql: { phase: 'stopped' },
    tikv: { phase: 'unavailable', reason: 'no_container_runtime' },
  };
  // `?noruntime` previews the install guidance on every stack.
  if (/[?&]noruntime\b/.test(globalThis.location?.search ?? '')) {
    for (const id of ['postgres', 'wesql'] as const) {
      states[id] = { phase: 'unavailable', reason: 'no_container_runtime' };
    }
  }
  const listeners = new Set<(id: StackId, s: StackState) => void>();
  const set = (id: StackId, s: StackState) => {
    states[id] = s;
    for (const l of listeners) l(id, s);
  };
  return {
    state: async (id) => states[id],
    start: async (id) => {
      if (states[id].phase === 'unavailable') {
        return { ok: false, code: 'unavailable', message: 'no_container_runtime' };
      }
      set(id, { phase: 'starting' });
      globalThis.setTimeout(
        () =>
          set(id, {
            phase: 'running',
            services: [{ name: id, state: 'running', ports: ['127.0.0.1:0'] }],
          }),
        800,
      );
      return ok;
    },
    stop: async (id) => {
      set(id, { phase: 'stopped' });
      return ok;
    },
    onState: (cb) => {
      listeners.add(cb);
      return () => void listeners.delete(cb);
    },
    openLogs: async () => ok,
  };
}

// ---- chat (D675): a scripted fake provider so the agent panel can be previewed ----

const FAKE_PROVIDERS: ChatProviderInfo[] = [
  ['anthropic', 'Anthropic', 'anthropic', 'https://api.anthropic.com', 'claude-sonnet-5-5', true],
  ['deepseek', 'DeepSeek', 'openai', 'https://api.deepseek.com/v1', 'deepseek-chat', true],
  ['openai', 'OpenAI', 'openai', 'https://api.openai.com/v1', 'gpt-5', true],
  ['ollama', 'Ollama', 'openai', 'http://127.0.0.1:11434/v1', 'llama3.1', false],
].map(([id, label, kind, baseUrl, model, needsKey]) => ({
  id: id as ChatProviderId,
  label: label as string,
  kind: kind as 'anthropic' | 'openai',
  baseUrl: baseUrl as string,
  model: model as string,
  defaultModel: model as string,
  needsKey: needsKey as boolean,
  // The preview pretends Anthropic has a key; no key is ever stored.
  hasKey: id === 'anthropic',
  configured: id === 'anthropic' || !needsKey,
  persistent: true,
  ...(kind === 'anthropic' ? { fallback: false } : {}),
}));

function createFakeChat(): LoamsDesktopApi['chat'] {
  const chats = new Map<string, ChatRecord>();
  const listeners = new Set<(e: ChatEvent) => void>();
  const running = new Map<
    string,
    {
      ctl: AbortController;
      pending: Map<
        string,
        { call: ChatView['pending'][number]; resolve: (d: ChatApproval) => void }
      >;
    }
  >();
  const emit = (e: ChatEvent) => {
    for (const l of listeners) l(e);
  };
  const summary = (c: ChatRecord): ChatSummary => ({
    id: c.id,
    title: c.title,
    createdAt: c.createdAt,
    updatedAt: c.updatedAt,
    provider: c.provider,
    model: c.model,
  });
  const sleep = (ms: number, signal: AbortSignal) =>
    new Promise<void>((resolve, reject) => {
      const t = globalThis.setTimeout(resolve, ms);
      signal.addEventListener('abort', () => {
        globalThis.clearTimeout(t);
        reject(new Error('aborted'));
      });
    });
  const notFound = { ok: false as const, code: 'not_found', message: 'Unknown chat' };

  async function script(chat: ChatRecord, text: string, signal: AbortSignal): Promise<void> {
    const run = running.get(chat.id);
    const chatId = chat.id;
    const usage = { inputTokens: 0, outputTokens: 0 };
    const assistant: ChatPart[] = [];
    const stream = async (words: string) => {
      let acc = '';
      for (const w of words.split(/(?<= )/)) {
        await sleep(35, signal);
        acc += w;
        emit({ kind: 'delta', chatId, text: w });
      }
      assistant.push({ type: 'text', text: acc });
    };
    const tool = async (name: string, args: unknown, risk: 'read' | 'write', result: unknown) => {
      const callId = `call_${Math.random().toString(36).slice(2, 10)}`;
      const needsApproval = risk === 'write' && !chat.alwaysAllow.includes(name);
      assistant.push({ type: 'tool_use', id: callId, name, input: args });
      emit({ kind: 'tool_call', chatId, callId, tool: name, args, risk, needsApproval });
      let ok = true;
      let out = JSON.stringify(result, null, 2);
      if (needsApproval && run) {
        const decision = await new Promise<ChatApproval>((resolve, reject) => {
          run.pending.set(callId, { call: { callId, tool: name, args, risk }, resolve });
          signal.addEventListener('abort', () => reject(new Error('aborted')));
        });
        run.pending.delete(callId);
        if (decision === 'always') chat.alwaysAllow.push(name);
        if (decision === 'deny') {
          ok = false;
          out = 'The user denied this action.';
        }
      }
      await sleep(400, signal);
      chat.messages.push({ role: 'assistant', content: assistant.splice(0), at: Date.now() });
      chat.messages.push({
        role: 'user',
        content: [
          { type: 'tool_result', toolUseId: callId, text: out, ...(ok ? {} : { isError: true }) },
        ],
        at: Date.now(),
      });
      emit({ kind: 'tool_result', chatId, callId, ok, text: out });
      usage.inputTokens += 900;
      usage.outputTokens += 60;
    };

    emit({
      kind: 'thinking',
      chatId,
      text: 'The user asked about their data; list the collections first.',
    });
    await stream('Let me look at the collections in **default**. ');
    await tool('collections_list', { namespace: 'default' }, 'read', [
      { name: 'docs', documents: 1240 },
      { name: 'tickets', documents: 318 },
    ]);
    if (/create|promise|write|approve/i.test(text)) {
      await stream('I will create a durable promise for that. ');
      await tool(
        'durable_promise_create',
        { id: 'preview-promise-1', timeoutMs: 3_600_000, param: { from: 'agent' } },
        'write',
        { id: 'preview-promise-1', state: 'pending' },
      );
    }
    await stream(
      'There are **2 collections**:\n\n| Collection | Documents |\n|---|---|\n| docs | 1240 |\n| tickets | 318 |\n\nTry:\n\n```sql\nSELECT count(*) FROM tickets\n```\n',
    );
    chat.messages.push({
      role: 'assistant',
      content: assistant.splice(0),
      at: Date.now(),
      stop: 'end_turn',
    });
    usage.inputTokens += 1200;
    usage.outputTokens += 140;
    emit({ kind: 'done', chatId, stop: 'end_turn', usage });
  }

  return {
    providers: async () => FAKE_PROVIDERS.map((p) => ({ ...p })),
    configureProvider: async (id, cfg) => {
      const p = FAKE_PROVIDERS.find((x) => x.id === id);
      if (!p) return { ok: false, code: 'unknown_provider', message: 'Unknown provider' };
      if (!cfg.model.trim()) return { ok: false, code: 'bad_model', message: 'Enter a model name' };
      p.model = cfg.model.trim();
      if (cfg.baseUrl) p.baseUrl = cfg.baseUrl;
      if (p.kind === 'anthropic' && typeof cfg.fallback === 'boolean') p.fallback = cfg.fallback;
      // The preview never keeps the key: it only records that one was given.
      if (cfg.apiKey) p.hasKey = true;
      p.configured = p.hasKey || !p.needsKey;
      return { ok: true, value: { ...p } };
    },
    testProvider: async (id) => {
      const p = FAKE_PROVIDERS.find((x) => x.id === id);
      if (!p) return { ok: false, code: 'unknown_provider', message: 'Unknown provider' };
      if (!p.configured) {
        return { ok: false, code: 'unconfigured', message: `${p.label} has no API key yet.` };
      }
      return { ok: true, value: { model: p.model, ms: 240 } };
    },
    list: async () => [...chats.values()].map(summary).sort((a, b) => b.updatedAt - a.updatedAt),
    get: async (chatId) => {
      const c = chats.get(chatId);
      if (!c) return notFound;
      const run = running.get(chatId);
      return {
        ok: true,
        value: {
          ...structuredClone(c),
          running: run !== undefined,
          pending: run ? [...run.pending.values()].map((p) => p.call) : [],
        },
      };
    },
    create: async (opts) => {
      const p =
        FAKE_PROVIDERS.find((x) => x.id === opts?.provider) ??
        FAKE_PROVIDERS.find((x) => x.configured) ??
        (FAKE_PROVIDERS[0] as ChatProviderInfo);
      const now = Date.now();
      const c: ChatRecord = {
        id: `c_${now.toString(36)}${Math.random().toString(36).slice(2, 8)}`,
        title: '',
        createdAt: now,
        updatedAt: now,
        provider: p.id,
        model: opts?.model ?? p.model,
        alwaysAllow: [],
        messages: [],
      };
      chats.set(c.id, c);
      return { ok: true, value: summary(c) };
    },
    send: async (chatId, text, opts) => {
      const c = chats.get(chatId);
      if (!c) return notFound;
      if (!text.trim()) return { ok: false, code: 'bad_request', message: 'Write a message first' };
      if (running.has(chatId)) {
        return { ok: false, code: 'busy', message: 'This chat is still answering. Stop it first.' };
      }
      if (opts?.provider) c.provider = opts.provider;
      if (opts?.model) c.model = opts.model;
      const p = FAKE_PROVIDERS.find((x) => x.id === c.provider);
      if (!p?.configured) {
        return {
          ok: false,
          code: 'unconfigured',
          message: `${p?.label ?? 'This provider'} has no API key yet. Add one in Settings › Agent.`,
        };
      }
      c.messages.push({ role: 'user', content: [{ type: 'text', text }], at: Date.now() });
      if (!c.title) c.title = text.replace(/\s+/g, ' ').trim().slice(0, 60);
      c.updatedAt = Date.now();
      const ctl = new AbortController();
      running.set(chatId, { ctl, pending: new Map() });
      void script(c, text, ctl.signal)
        .catch(() => {
          emit({
            kind: 'done',
            chatId,
            stop: 'cancelled',
            usage: { inputTokens: 0, outputTokens: 0 },
          });
        })
        .finally(() => {
          running.delete(chatId);
          c.updatedAt = Date.now();
        });
      return ok;
    },
    cancel: async (chatId) => {
      running.get(chatId)?.ctl.abort();
    },
    approve: async (chatId, callId, decision) => {
      const p = running.get(chatId)?.pending.get(callId);
      if (!p) return { ok: false, code: 'not_found', message: 'Nothing is waiting for approval' };
      p.resolve(decision);
      return ok;
    },
    remove: async (chatId) => {
      running.get(chatId)?.ctl.abort();
      chats.delete(chatId);
      return ok;
    },
    onEvent: (cb) => {
      listeners.add(cb);
      return () => void listeners.delete(cb);
    },
  };
}

/** The preview's updater: a manual-mode feed that finds v0.2.0 on "Check now". */
function previewUpdate(): LoamsDesktopApi['update'] {
  let state: Awaited<ReturnType<LoamsDesktopApi['update']['state']>> = { phase: 'idle' };
  return {
    state: async () => state,
    check: async () => {
      state = { phase: 'checking' };
      await new Promise((r) => globalThis.setTimeout(r, 500));
      state = { phase: 'available', version: '0.2.0', mode: 'manual' };
    },
    download: async () => undefined,
    installAndRestart: async () => undefined,
  };
}
