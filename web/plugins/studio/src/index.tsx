// @loams/plugin-studio: Loams Studio, the agentic development environment in the
// console (design §53; SD1 Task 15). `modes: ["studio"]`; the host applies it at boot in
// every edition where Studio mode is available (design 59, W10). It registers Studio's
// routes, its nav entries, and its parts of the shell's regions (regions.tsx): the
// sidebar's threads, the home, the inspector and drawer tabs, and the top bar context. It reaches loams-agentd through the console's
// `agentd` service on the desktop (SD1 Task 6), and on the web through the selected
// remote agentd host (SD1 Task 7, hosts/agentd.ts; `?host=<id>` selects one), in
// both cases with connect-query over the host's transport (agentd.ts).

import { type PluginModule, service } from '@loams/console-host';
import { AgentdScope } from '@loams/console-host/query';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { type StudioAgentd, studioAgentd } from './agentd.js';
import { desktopFactory } from './factory.js';
import { agentdFactoryBridge } from './factory-bridge.js';
import { GitActionBus, withGitActions } from './git/bus.js';
import { hostedAgentd } from './hosts/agentd.js';
import { webNotifier } from './notifications/data.js';
import { BranchesSource } from './palette/BranchesSource.js';
import { createStudioKeys, listenShellCommands } from './palette/commands.js';
import { StudioPalette } from './palette/StudioPalette.js';
import { desktopPreview } from './preview/access.js';
import { registerStudioRegions } from './regions.js';
import { routeOf, STUDIO_PAGES } from './routes.js';
import { agentdPaletteSource } from './sidebar/paletteSource.js';
import { StudioView } from './view.js';

export {
  isCloneUrl,
  type Project,
  projectOf,
  type Reach,
  type StudioAgentd,
  studioAgentd,
} from './agentd.js';
export {
  desktopFactory,
  type FactoryApp,
  type FactoryPull,
  type StudioFactory,
} from './factory.js';
export { GitActionBus, type GitToolbarEntry, withGitActions } from './git/bus.js';
export { gitErrorOf } from './git/errors.js';
export { ProjectGitTools, StudioGitTools, ThreadGitTools } from './git/GitToolbar.js';
export { ProjectActionsEditor } from './git/ProjectActions.js';
export { type HostedAgentdOptions, hostedAgentd, type StudioHosts } from './hosts/agentd.js';
export { StudioLayout } from './layout/StudioLayout.js';
export { BranchesSource } from './palette/BranchesSource.js';
export {
  type CommandName,
  createStudioKeys,
  listenShellCommands,
  StudioCommands,
  type StudioKeys,
  useStudioCommand,
  useStudioKeybindings,
} from './palette/commands.js';
export { StudioPalette } from './palette/StudioPalette.js';
export {
  type GitAction,
  type PaletteGitAction,
  type PaletteHarness,
  pendingPaletteSource,
  type StudioPaletteSource,
} from './palette/source.js';
export {
  type DesktopPreviewApi,
  desktopPreview,
  PreviewUnsupported,
  remotePreview,
  remotePreviewOpener,
  type StudioPreview,
} from './preview/access.js';
export { ForwardHint, PreviewTab } from './preview/PreviewTab.js';
export {
  registerStudioRegions,
  STUDIO_TABS,
  StudioHome,
  type StudioRegionDeps,
  StudioThreads,
  StudioTools,
  studioRouteAt,
  THREAD_ROUTES,
} from './regions.js';
export { routeOf, STUDIO_PAGES, type StudioRoute, studioHref, type ThreadDraft } from './routes.js';
export { KeybindingsCard } from './settings/Keybindings.js';
export { FactoryGroup } from './sidebar/Factory.js';
export { agentdPaletteSource } from './sidebar/paletteSource.js';
export { StudioSidebar, THREADS_SHOWN } from './sidebar/Sidebar.js';
export {
  type ComposerProject,
  ThreadComposer,
  type ThreadComposerProps,
} from './thread/Composer.js';
export { BranchPr, DraftCard } from './thread/FactoryLinks.js';
export { buildCreateRunGroupRequest, RunInParallelDialog } from './thread/RunInParallel.js';
export {
  agentdTerminalBackend,
  fileViewerHref,
  isStudioShortcut,
  ThreadTerminalDrawer,
  terminalSessionsFor,
} from './thread/TerminalDrawer.js';
export {
  addTerminalContext,
  removeTerminalContext,
  takeTerminalContexts,
  terminalContexts,
  useTerminalContexts,
} from './thread/terminalContexts.js';
export { StudioView } from './view.js';

const plugin: PluginModule = {
  name: 'studio',
  // `desktop` only on the desktop edition: the catalog rows narrow this list.
  inject: ['router', 'slots', 'mode', 'platform', 'flags', 'agentd', 'inspector', 'drawer'],
  apply(ctx: Context) {
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const platform = service(ctx, 'platform');
    const inspector = service(ctx, 'inspector');
    const drawer = service(ctx, 'drawer');
    const desktop = platform.kind === 'desktop' ? service(ctx, 'desktop') : undefined;
    const agentd: StudioAgentd = desktop
      ? studioAgentd(
          service(ctx, 'agentd'),
          desktop.shell,
          desktop.studio,
          desktopPreview(desktop.preview),
        )
      : hostedAgentd();
    // The Factory group (SD1 Task 27): the desktop's factory bridge; on the web nothing
    // until SF1 (D922).
    // In a browser, the factory's reads come from the selected agentd host (SF1
    // amendment F1): Studio provides them as `studio.factory` for the factory's pages.
    const bridge = desktop
      ? undefined
      : agentdFactoryBridge(
          () => agentd.transport(),
          (url) => void platform.openExternal(url),
        );
    if (bridge) ctx.provide('studio.factory', bridge);
    const factory = desktop?.factory
      ? desktopFactory(desktop)
      : bridge
        ? desktopFactory({ factory: bridge }, { needsAgentd: true })
        : undefined;
    const navigate = (to: string) => router.navigate(to);
    const setTrail = (trail: { label: string; href?: string }[] | undefined) =>
      router.setTrail?.(trail);
    // One keyboard layer and palette source for every Studio page (SD1 Task 24). The
    // palette's threads and run groups come from agentd (SD1 Task 17).
    const keys = createStudioKeys({ edition: platform.kind });
    // Its git rows and commands are the open thread's toolbar's (SD1 Task 23, T24-6).
    const gitBus = new GitActionBus();
    const paletteSource = withGitActions(agentdPaletteSource(agentd), gitBus);
    // The desktop menu's "New thread" (SD1 Task 28) runs `chat.new` like Mod+N.
    ctx.effect(() => listenShellCommands(keys.commands));

    for (const page of STUDIO_PAGES) {
      ctx.effect(() =>
        router.page(
          { id: page.id, path: page.path, title: page.title, plugin: 'studio' },
          ({ params }) => (
            <StudioView
              agentd={agentd}
              route={routeOf(page.kind, params)}
              navigate={navigate}
              hostParam={params.host}
              keys={keys}
              openExternal={(url) => platform.openExternal(url)}
              inspector={inspector}
              drawer={drawer}
              setTrail={setTrail}
            />
          ),
        ),
      );
    }
    // Studio's agentd for other plugins' pages (MK1 Task 9: the Seed Exchange), so on
    // the web they reach the host picked here rather than the console's (none yet).
    ctx.effect(() =>
      slots.register({ name: 'studio.agentd.scope', plugin: 'studio' }, ({ children }) => (
        <AgentdScope agentd={agentd}>{children}</AgentdScope>
      )),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'studio',
          order: 0,
          meta: {
            id: 'studio-home',
            label: 'Home',
            href: '/studio',
            icon: 'studio',
            group: 'Studio',
            mode: 'studio',
          },
        },
        () => null,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'studio',
          order: 90,
          meta: {
            id: 'studio-settings',
            // Settings lists every settings page (D1080), so this one says whose it is.
            label: 'Studio settings',
            href: '/studio/settings',
            icon: 'settings',
            group: 'Studio',
            mode: 'studio',
          },
        },
        () => null,
      ),
    );
    ctx.effect(() =>
      slots.register(
        { name: 'palette.command', plugin: 'studio', meta: { mode: 'studio' } },
        ({ query, publish }) => (
          <StudioPalette
            query={query}
            publish={publish}
            agentd={agentd}
            source={paletteSource}
            router={router}
            keys={keys}
            copy={(text) => platform.clipboardWrite(text)}
          />
        ),
      ),
    );
    // The palette's branches (CF1 Task 11): navigation to the branch's project only.
    ctx.effect(() =>
      slots.register(
        { name: 'shell.palette.source', plugin: 'studio', meta: { mode: 'studio' } },
        (p) => <BranchesSource {...p} agentd={agentd} navigate={navigate} />,
      ),
    );
    // Design 59: the sidebar's threads, the home, the inspector and drawer tabs, the top bar.
    ctx.effect(() => {
      const disposers = registerStudioRegions(slots, {
        agentd,
        factory,
        router,
        keys,
        inspector,
        drawer,
        gitBus,
        openExternal: (url) => platform.openExternal(url),
        mode: service(ctx, 'mode'),
        // On the desktop Electron main posts these from the same stream (W15).
        ...(platform.kind === 'web' ? { notify: webNotifier((n) => platform.notify(n)) } : {}),
      });
      return () => {
        for (const dispose of disposers) dispose();
      };
    }, 'studio-regions');
  },
};

export default plugin;
