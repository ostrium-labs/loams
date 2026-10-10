import type { EngineState, LoamsDesktopApi, StackState } from '@loams/desktop/contracts';

/** A plain-object `desktop` service for the Overview tests. */
export function fakeDesktop(init: { engine?: EngineState; active?: 'local' | 'demo' } = {}) {
  const calls: string[] = [];
  const engine = init.engine ?? { phase: 'stopped' };
  const stacks: Record<string, StackState> = {
    postgres: { phase: 'running', services: [{ name: 'pg', state: 'up', ports: ['5432'] }] },
    wesql: { phase: 'stopped' },
    tikv: { phase: 'stopped' },
  };
  const api = {
    version: '1.2.3',
    platform: 'linux',
    servers: {
      list: async () => ({
        servers: [
          { id: 'local', name: 'This computer', kind: 'local', url: 'http://127.0.0.1:8080' },
          { id: 'demo', name: 'Demo', kind: 'demo', url: 'http://127.0.0.1:8090' },
        ],
        activeId: init.active ?? 'local',
      }),
    },
    engine: { state: async () => engine, onState: () => () => undefined },
    stacks: {
      state: async (id: string) => {
        calls.push(`stacks.state:${id}`);
        return stacks[id];
      },
      onState: () => () => undefined,
    },
    connectors: {
      catalog: async () => {
        calls.push('connectors.catalog');
        return [
          { id: 'a', status: 'preview' },
          { id: 'b', status: 'planned' },
          { id: 'c', status: 'planned' },
        ];
      },
    },
    factory: {
      list: async () => {
        calls.push('factory.list');
        return [
          { id: 'forgejo', label: 'Forgejo', health: 'ok' },
          { id: 'zulip', label: 'Zulip', health: 'unconfigured' },
        ];
      },
    },
  } as unknown as LoamsDesktopApi;
  return { api, calls };
}
