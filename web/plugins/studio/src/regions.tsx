// Studio in the shell's regions (design 59, D1078–D1079): its projects and threads in the
// sidebar (W6), its home in `shell.home` (W11), its thread panels as inspector tabs and its
// terminal as a drawer tab on thread routes, and its context and git tools in the top bar.

import type { Location, ModeService, PanelService, RouterService } from '@loams/console-host';
import { AgentdScope } from '@loams/console-host/query';
import type { PanelTabProps, SlotRegistry } from '@loams/slots';
import { useEffect, useMemo, useSyncExternalStore } from 'react';
import type { StudioAgentd } from './agentd.js';
import type { StudioFactory } from './factory.js';
import type { GitActionBus } from './git/bus.js';
import { StudioGitTools } from './git/GitToolbar.js';
import { StudioContext } from './header.js';
import { Home } from './home/Home.js';
import { NotConnected } from './home/states.js';
import type { StudioNotification } from './notifications/data.js';
import type { StudioKeys } from './palette/commands.js';
import { useReach } from './projects/queries.js';
import { type Panel, routeOf, STUDIO_PAGES, type StudioRoute } from './routes.js';
import { FactoryGroup } from './sidebar/Factory.js';
import { StudioSidebar } from './sidebar/Sidebar.js';
import { DraftCard } from './thread/FactoryLinks.js';
import { ThreadTerminalDrawer } from './thread/TerminalDrawer.js';
import { ThreadPanelBody, useStudioPageKeys } from './view.js';

/** The routes Studio's inspector and drawer tabs show on. */
export const THREAD_ROUTES: readonly string[] = ['/studio/t/:threadId'];

/** The thread's panels, in the inspector's order (tab ids `studio.<panel>`). */
export const STUDIO_TABS: readonly { panel: Exclude<Panel, 'terminal'>; label: string }[] = [
  { panel: 'diff', label: 'Diff' },
  { panel: 'files', label: 'Files' },
  { panel: 'preview', label: 'Preview' },
  { panel: 'plan', label: 'Plan' },
];

export interface StudioRegionDeps {
  agentd: StudioAgentd;
  factory?: StudioFactory | undefined;
  router: RouterService;
  keys: StudioKeys;
  inspector: PanelService;
  drawer: PanelService;
  gitBus: GitActionBus;
  openExternal: (url: string) => Promise<void>;
  /** The console's modes: the sidebar's Factory group is Studio mode's only. */
  mode?: ModeService | undefined;
  /** Posts an OS notification for one that just arrived (the web edition only, W15). */
  notify?: ((n: StudioNotification) => void) | undefined;
}

/** The Studio route the router is on, or the home for any other page. */
export function studioRouteAt(location: Pick<Location, 'pageId' | 'params'>): StudioRoute {
  const page = STUDIO_PAGES.find((p) => p.id === location.pageId);
  return page ? routeOf(page.kind, location.params) : { kind: 'home' };
}

/** The router's Studio route, re-rendering only when the page or its params change. */
function useRoute(router: RouterService): StudioRoute {
  const key = useSyncExternalStore(router.subscribe, () => {
    const { pageId, params } = router.current();
    return JSON.stringify([pageId ?? '', params]);
  });
  return useMemo(() => {
    const [pageId, params] = JSON.parse(key) as [string, Record<string, string>];
    return studioRouteAt({ pageId, params });
  }, [key]);
}

const noModeChange = () => () => {};

/** Whether the console is in Studio mode (true without a mode service). */
function useStudioMode(mode: ModeService | undefined): boolean {
  return useSyncExternalStore(mode ? (listener) => mode.onChange(listener) : noModeChange, () =>
    mode ? mode.current === 'studio' : true,
  );
}

type ThreadsProps = Pick<StudioRegionDeps, 'agentd' | 'factory' | 'router' | 'keys' | 'mode'>;

function Threads({ agentd, factory, router, keys, mode }: ThreadsProps) {
  const reach = useReach(agentd);
  const route = useRoute(router);
  // The threads show in both modes (W10); the Factory group is a Studio area, as its nav is.
  const studio = useStudioMode(mode);
  // Q934: the thread list shows when an agentd is reachable. The Factory group does not
  // need the daemon, so it shows either way (owner ruling, 2026-10-18).
  if (reach.state !== 'ready')
    return factory && studio && !factory.needsAgentd ? <FactoryGroup factory={factory} /> : null;
  return (
    <StudioSidebar agentd={agentd} route={route} navigate={(to) => router.navigate(to)} keys={keys}>
      {factory && studio && <FactoryGroup factory={factory} />}
    </StudioSidebar>
  );
}

/** The shell sidebar's projects and threads (`shell.sidebar.threads`). */
export function StudioThreads(props: ThreadsProps) {
  return (
    <AgentdScope agentd={props.agentd}>
      <Threads {...props} />
    </AgentdScope>
  );
}

interface HomeProps {
  agentd: StudioAgentd;
  params: Record<string, string>;
  keys: StudioKeys;
  navigate: (to: string) => void;
  inspector?: PanelService | undefined;
}

function HomeBody({ agentd, params, keys, navigate, inspector }: HomeProps) {
  const host = params.host;
  useEffect(() => {
    // Web: `?host=<id>` selects a registered agentd host, by id only (§53 §4.4).
    if (host) agentd.hosts?.select(host);
  }, [agentd, host]);
  const reach = useReach(agentd);
  const route = useMemo(() => routeOf('home', params), [params]);
  const connected = reach.state === 'ready';
  // The home is a Studio page too: Mod+N and the palette's Studio commands work here.
  useStudioPageKeys({ agentd, keys, route, navigate, connected, inspector });
  if (!connected) return <NotConnected agentd={agentd} reach={reach} />;
  return (
    <div className="flex w-full max-w-[760px] flex-col gap-4" data-studio-home>
      {route.kind === 'home' && route.draft && <DraftCard draft={route.draft} />}
      <Home agentd={agentd} />
    </div>
  );
}

/** The existing Studio composer, centred (§2: until Part 2 fills the home). */
export function StudioHome(props: HomeProps) {
  return (
    <AgentdScope agentd={props.agentd}>
      <HomeBody {...props} />
    </AgentdScope>
  );
}

/** One of the thread's panels as an inspector tab. */
export function StudioThreadTab({
  agentd,
  panel,
  params,
  navigate,
  openExternal,
}: {
  agentd: StudioAgentd;
  panel: Exclude<Panel, 'terminal'>;
  navigate: (to: string) => void;
  openExternal?: (url: string) => Promise<void>;
} & Pick<PanelTabProps, 'params'>) {
  const route = routeOf('thread', params);
  if (route.kind !== 'thread' || !route.threadId) return null;
  return (
    <AgentdScope agentd={agentd}>
      <ThreadPanelBody
        agentd={agentd}
        route={{ ...route, panel }}
        navigate={navigate}
        {...(openExternal ? { openExternal } : {})}
      />
    </AgentdScope>
  );
}

/** The thread's terminals as the drawer's tab. */
export function StudioTerminalTab({
  agentd,
  keys,
  params,
  navigate,
  openExternal,
}: Pick<StudioRegionDeps, 'agentd' | 'keys'> & {
  navigate: (to: string) => void;
  openExternal?: (url: string) => Promise<void>;
} & Pick<PanelTabProps, 'params'>) {
  const threadId = params.threadId;
  if (!threadId) return null;
  return (
    <AgentdScope agentd={agentd}>
      <ThreadTerminalDrawer
        threadId={threadId}
        keys={keys}
        navigate={navigate}
        {...(openExternal ? { openExternal } : {})}
      />
    </AgentdScope>
  );
}

type ToolsProps = Pick<StudioRegionDeps, 'agentd' | 'factory' | 'router' | 'gitBus' | 'drawer'>;

function Tools({ agentd, factory, router, gitBus, drawer }: ToolsProps) {
  const reach = useReach(agentd);
  const route = useRoute(router);
  if (reach.state !== 'ready' || (route.kind !== 'thread' && route.kind !== 'project')) return null;
  return (
    <div className="lc-studio-tools flex min-w-0 items-center gap-1.5" data-studio-tools>
      <StudioGitTools
        agentd={agentd}
        route={route}
        factory={factory}
        bus={gitBus}
        {...(route.kind === 'thread'
          ? { openTerminal: () => drawer.open({ tab: 'studio.terminal' }) }
          : {})}
      />
    </div>
  );
}

/** The open thread's or project's git tools, in the top bar's right cluster. */
export function StudioTools(props: ToolsProps) {
  return (
    <AgentdScope agentd={props.agentd}>
      <Tools {...props} />
    </AgentdScope>
  );
}

/** Registers Studio's region entries; returns their disposers. */
export function registerStudioRegions(slots: SlotRegistry, d: StudioRegionDeps): (() => void)[] {
  const navigate = (to: string) => d.router.navigate(to);
  return [
    slots.register({ name: 'shell.sidebar.threads', plugin: 'studio' }, () => (
      <StudioThreads
        agentd={d.agentd}
        factory={d.factory}
        router={d.router}
        keys={d.keys}
        mode={d.mode}
      />
    )),
    slots.register(
      { name: 'shell.home', plugin: 'studio', meta: { mode: 'studio' } },
      ({ params }) => (
        <StudioHome
          agentd={d.agentd}
          params={params}
          keys={d.keys}
          navigate={navigate}
          inspector={d.inspector}
        />
      ),
    ),
    ...STUDIO_TABS.map(({ panel, label }, i) =>
      slots.register(
        {
          name: 'shell.inspector.tab',
          plugin: 'studio',
          order: i,
          meta: { id: `studio.${panel}`, label, routes: THREAD_ROUTES },
        },
        ({ params }) => (
          <StudioThreadTab
            agentd={d.agentd}
            panel={panel}
            params={params}
            navigate={navigate}
            openExternal={d.openExternal}
          />
        ),
      ),
    ),
    slots.register(
      {
        name: 'shell.drawer.tab',
        plugin: 'studio',
        meta: { id: 'studio.terminal', label: 'Terminal', routes: THREAD_ROUTES },
      },
      ({ params }) => (
        <StudioTerminalTab
          agentd={d.agentd}
          keys={d.keys}
          params={params}
          navigate={navigate}
          openExternal={d.openExternal}
        />
      ),
    ),
    slots.register(
      { name: 'shell.topbar.left', plugin: 'studio', meta: { mode: 'studio' } },
      () => <StudioContext agentd={d.agentd} router={d.router} notify={d.notify} />,
    ),
    slots.register(
      { name: 'shell.topbar.right', plugin: 'studio', meta: { mode: 'studio' } },
      () => (
        <StudioTools
          agentd={d.agentd}
          factory={d.factory}
          router={d.router}
          gitBus={d.gitBus}
          drawer={d.drawer}
        />
      ),
    ),
  ];
}
