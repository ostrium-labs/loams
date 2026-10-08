import type { EngineState, LoamsDesktopApi, ServerEntry } from '@loams/desktop/contracts';

/** A plain-object `desktop` service for the tests; every call is recorded. */
export function fakeDesktop(
  init: { servers?: ServerEntry[]; activeId?: string; engine?: EngineState } = {},
) {
  const calls: string[] = [];
  let servers = init.servers ?? [
    { id: 'local', name: 'This computer', kind: 'local' as const, url: 'http://127.0.0.1:8080' },
    { id: 'demo', name: 'Demo', kind: 'demo' as const, url: 'http://127.0.0.1:8090' },
  ];
  let activeId = init.activeId ?? 'local';
  let engine: EngineState = init.engine ?? { phase: 'stopped' };
  const listeners = new Set<(s: EngineState) => void>();
  const ok = { ok: true, value: undefined } as const;
  const api = {
    version: '0.0.0',
    platform: 'linux',
    servers: {
      list: async () => ({ servers: [...servers], activeId }),
      add: async (e: Omit<ServerEntry, 'id'>) => {
        calls.push(`add:${e.name}:${e.url}`);
        if (e.url.startsWith('http://') && !e.url.includes('127.0.0.1')) {
          return { ok: false, code: 'insecure_url', message: 'Use https:// for a remote server.' };
        }
        const entry = { ...e, id: `s${servers.length}` };
        servers = [...servers, entry];
        return { ok: true, value: entry };
      },
      remove: async (id: string) => {
        calls.push(`remove:${id}`);
        servers = servers.filter((s) => s.id !== id);
        return ok;
      },
      activate: async (id: string) => {
        calls.push(`activate:${id}`);
        activeId = id;
        return ok;
      },
    },
    engine: {
      state: async () => engine,
      start: async () => void calls.push('engine.start'),
      stop: async () => void calls.push('engine.stop'),
      openLogs: async () => void calls.push('engine.openLogs'),
      onState: (cb: (s: EngineState) => void) => {
        listeners.add(cb);
        return () => listeners.delete(cb);
      },
    },
    shell: {
      clipboardWrite: async (t: string) => void calls.push(`copy:${t}`),
    },
  } as unknown as LoamsDesktopApi;
  return {
    api,
    calls,
    emit(s: EngineState) {
      engine = s;
      for (const l of listeners) l(s);
    },
  };
}
