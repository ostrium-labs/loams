// @loams/platform-electron: `platform`, `transport` and `desktop` in the
// Electron shell (AP1e). The console runs at `loams-app://console`; the main
// process proxies `/v1`, `/api` etc. to the active server, so the base URL is
// this page's origin. The three side effects go over IPC (`window.loamsDesktop`).

import type { Interceptor, Transport } from '@connectrpc/connect';
import { createConnectTransport } from '@connectrpc/connect-web';
import { createAgentdTransport } from '@loams/agentd-client/connect';
import {
  type AgentdReach,
  type AgentdService,
  localThemeStore,
  MODE_HOME,
  type ModeService,
  type PlatformService,
  type PluginModule,
  parseThemeChoice,
  SHELL_COMMAND_EVENT,
  ThemeController,
  type ThemeStore,
} from '@loams/console-host';
import type { Context } from '@loams/cordis';
import type { AgentdStatus, LoamsDesktopApi, ShellCommand } from '@loams/desktop/contracts';

export type { LoamsDesktopApi, ShellCommand };

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

/** The desktop keeps the choice in settings.json (D1074); local storage mirrors it for `?desktop`. */
export function desktopThemeStore(api: LoamsDesktopApi): ThemeStore {
  const local = localThemeStore();
  return {
    read: () => (api.theme ? parseThemeChoice(api.theme.initial) : local.read()),
    write: (choice, dark) => {
      local.write(choice, dark);
      void api.theme?.set(choice, dark).catch(() => {
        // Main did not save it: the choice holds for this window.
      });
    },
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
        async notify({ title, body, route }) {
          await api.shell.notify(title, body, route);
        },
        clipboardWrite: (text) => api.shell.clipboardWrite(text),
        // The local engine coming up changes GetInstance (it failed with 503 before).
        onInstanceStale(callback) {
          return api.engine.onState((s) => {
            if (s.phase !== 'ready') return;
            void kind().then((k) => k === 'local' && callback());
          });
        },
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
      ctx.provide('theme', new ThemeController(desktopThemeStore(api)));
      // loams-agentd through Electron main's route (SD1 Tasks 5–6); only
      // @loams/plugin-rpc reads it, and turns it into `agentd` and `rpc.agentd.*`.
      ctx.provide('agentd.endpoint', desktopAgentdEndpoint(api));
    },
  };
}

function reachOf(s: AgentdStatus): AgentdReach {
  if (s.state === 'running') return { state: 'ready' };
  if (s.state === 'stopped') return { state: 'stopped' };
  return { state: 'unavailable', reason: s.reason };
}

/**
 * The desktop's agentd: its reach follows the daemon status Electron main reports,
 * and its transport is `loams-app://console/`, whose `/loams.agentd.v1.` route
 * main forwards to the daemon with the token only main holds.
 */
export function desktopAgentdEndpoint(
  api: Pick<LoamsDesktopApi, 'agentd'>,
  transport: Transport = createAgentdTransport({ kind: 'desktop' }),
): AgentdService {
  let reach: AgentdReach = { state: 'checking' };
  const listeners = new Set<() => void>();
  const set = (next: AgentdReach) => {
    if (next.state === reach.state && JSON.stringify(next) === JSON.stringify(reach)) return;
    reach = next;
    for (const listener of [...listeners]) listener();
  };
  let watching = false;
  const watch = () => {
    if (watching) return;
    watching = true;
    api.agentd.onStatus((s) => set(reachOf(s)));
    api.agentd
      .status()
      .then((s) => set(reachOf(s)))
      .catch(() => set({ state: 'unavailable', reason: 'no_bridge' }));
  };
  return {
    kind: 'desktop',
    reach() {
      watch();
      return reach;
    },
    subscribe(listener) {
      watch();
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    transport: () => (reach.state === 'ready' ? transport : undefined),
    async start() {
      const result = await api.agentd.start();
      if (!result.ok) throw new Error(result.message);
      // The status event follows; read it now so the page does not wait.
      set(reachOf(await api.agentd.status()));
    },
    host: () => undefined,
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

/**
 * The application menu's commands (SD1 Task 28, §53 §4.2). Studio, Console and Switch
 * mode go to the `mode` service. "New thread" is offered to the mounted mode as a
 * `SHELL_COMMAND_EVENT` (Studio runs its `chat.new`); when nothing takes it (Console
 * mode, or Studio still loading), Studio's home opens, where a project is chosen.
 * The shell's D1081 commands (CF1 Task 14: Command Palette, the panel toggles, Back,
 * Forward, Settings, Next Thread, Reopen Closed Thread) go out the same way; the shell
 * runs them as it runs their chords, and Studio takes `thread.next`.
 */
export function wireShellCommands(
  api: LoamsDesktopApi,
  mode: ModeService,
  target: EventTarget = globalThis,
): () => void {
  return api.shell.onCommand((command: ShellCommand) => {
    switch (command) {
      case 'mode.studio':
        mode.set('studio');
        return;
      case 'mode.console':
        mode.set('console');
        return;
      case 'mode.toggle': {
        const next = mode.available.find((m) => m !== mode.current);
        if (next) mode.set(next);
        return;
      }
      case 'chat.new': {
        const event = new CustomEvent(SHELL_COMMAND_EVENT, {
          detail: command,
          cancelable: true,
        });
        target.dispatchEvent(event);
        if (!event.defaultPrevented) mode.set('studio', { navigate: MODE_HOME.studio });
        return;
      }
      case 'palette.toggle':
      case 'sidebar.toggle':
      case 'inspector.toggle':
      case 'drawer.toggle':
      case 'navigation.back':
      case 'navigation.forward':
      case 'settings.open':
      case 'thread.next':
      case 'thread.reopen':
        target.dispatchEvent(
          new CustomEvent(SHELL_COMMAND_EVENT, { detail: command, cancelable: true }),
        );
        return;
    }
  });
}
