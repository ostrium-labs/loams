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
  FactoryAppInfo,
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
      list: async (): Promise<FactoryAppInfo[]> => [],
      configure: async () => ({
        ok: false,
        code: 'preview',
        message: 'Not available in the preview.',
      }),
      test: async (app) => ({
        id: app,
        label: app,
        health: 'unconfigured',
        hasPanels: false,
        credentialFields: [],
        persistent: false,
      }),
      remove: async () => undefined,
      query: async () => ({ ok: false, code: 'unconfigured', message: 'Not configured.' }),
      openApp: async () => ok,
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
