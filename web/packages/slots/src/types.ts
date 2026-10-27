import type { ComponentType, ReactNode } from 'react';

/** The selected environment, as slot props carry it. */
export interface EnvironmentRef {
  id: string;
  name: string;
  namespace: string;
  protected: boolean;
}

/**
 * The slot catalog (§37 §5.5). Plugins extend it by declaration merging:
 *
 *     declare module '@loams/slots' {
 *       interface SlotMap { 'my-plugin.panel': { kind: 'list'; props: { id: string } } }
 *     }
 *
 * Changing a slot's props is a breaking change of the console host.
 */
export interface SlotMap {
  root: { kind: 'single'; props: Record<string, never> };
  'console.nav': { kind: 'list'; props: { environment?: EnvironmentRef } };
  'console.page': {
    kind: 'keyed';
    props: { params: Record<string, string>; environment?: EnvironmentRef };
  };
  'console.settings.section': { kind: 'list'; props: Record<string, never> };
  'environment.overview.card': { kind: 'list'; props: { environment?: EnvironmentRef } };
  'approval.renderer': { kind: 'keyed'; props: { approvalId: string } };
  'shell.overlay': { kind: 'list'; props: Record<string, never> };
  /** A product nav entry; the shell reads its `meta` and renders the link itself. */
  'shell.nav.section': { kind: 'list'; props: Record<string, never> };
  /**
   * The old right dock. Its entry shows as an inspector tab (W10); new code registers a
   * `shell.inspector.tab` instead (the agent plugin does since CF1 Task 8).
   */
  'shell.dock.right': { kind: 'single'; props: Record<string, never> };
  /** The header's context controls (Studio's project, branch and host until Task 9). */
  'shell.header.server': { kind: 'list'; props: Record<string, never> };
  /**
   * The sidebar's context above the nav: the project switcher and the open project's
   * pages (API2 Task 10). While it has an entry, it replaces the environment switcher.
   */
  'shell.nav.context': { kind: 'list'; props: Record<string, never> };
  /** The server indicator under the brand (design 59 D1078): the desktop's server switcher. */
  'shell.sidebar.server': { kind: 'list'; props: Record<string, never> };
  /** Projects and their threads (Studio's sidebar, W6); empty hides the section. */
  'shell.sidebar.threads': { kind: 'list'; props: Record<string, never> };
  /** The top bar's start: the project and branch pills. Entries filter by `meta.mode`. */
  'shell.topbar.left': { kind: 'list'; props: Record<string, never> };
  /** The top bar's middle, before the palette hint. Entries filter by `meta.mode`. */
  'shell.topbar.center': { kind: 'list'; props: Record<string, never> };
  /** The top bar's end, before the inspector and drawer toggles. */
  'shell.topbar.right': { kind: 'list'; props: Record<string, never> };
  /** The agent home (Part 2 fills it; W11). Rendered on the mode's home route with its params. */
  'shell.home': { kind: 'single'; props: { params: Record<string, string> } };
  /**
   * Studio's agentd for another plugin's page (MK1 Task 9): its entry renders
   * `children` under Studio's AgentdScope (on the web, the host picked in Studio's
   * Settings › Agentd hosts). Empty until Studio loads.
   */
  'studio.agentd.scope': { kind: 'single'; props: { children: ReactNode } };
  /** A tab of the right inspector; `meta.id` is its tab id, `meta.label` its name, `meta.routes` where it shows. */
  'shell.inspector.tab': { kind: 'list'; props: PanelTabProps };
  /** A tab of the bottom drawer, as `shell.inspector.tab`. */
  'shell.drawer.tab': { kind: 'list'; props: PanelTabProps };
  /**
   * Commands for the shell's palette (§53 §4.5, §5.10; SD1 Task 24). An entry renders
   * nothing: while the palette is open it is mounted with the query and calls `publish`
   * with its commands (again whenever they change). Its mode is its `meta.mode`, else
   * its plugin's; the shell lists the current mode's commands first.
   */
  'palette.command': {
    kind: 'list';
    props: { query: string; publish: (commands: readonly PaletteCommand[]) => void };
  };
  /**
   * A palette source (design 59, D1078; the same contract as `palette.command`, which stays
   * an alias, W9): mounted with the query while the palette is open, it calls `publish`.
   */
  'shell.palette.source': {
    kind: 'list';
    props: { query: string; publish: (commands: readonly PaletteCommand[]) => void };
  };
}

/** What an inspector or drawer tab receives (design 59, D1078). */
export interface PanelTabProps {
  /** The current route's params. */
  params: Record<string, string>;
  /** What `inspector.open({ tab, props })` passed for this tab; empty otherwise. */
  props: Readonly<Record<string, unknown>>;
}

/** One row of the shell's command palette, as a `palette.command` entry publishes it. */
export interface PaletteCommand {
  /** Unique within its entry. */
  id: string;
  title: string;
  /** The heading it is listed under ("Threads", "Git"). */
  group?: string;
  /** Secondary text at the end of the row (a path, a project). */
  detail?: string;
  /** More words the query matches, after the title, in order of importance. */
  keywords?: string[];
  icon?: ReactNode;
  /** Its keyboard shortcut's keys, for example ['Ctrl', 'D']. */
  shortcut?: string[];
  /** When its subject last changed (threads); breaks ties between title matches. */
  recency?: number;
  /** Sorts after the other matches of its group. */
  secondary?: boolean;
  disabled?: boolean;
  /** Keep the palette open after running it. */
  keepOpen?: boolean;
  /** Runs the command. A thrown error or a rejection is shown as a toast. */
  run(): void | Promise<void>;
}

export type SlotName = keyof SlotMap;
export type SlotKind = SlotMap[SlotName]['kind'];
export type SlotProps<N extends SlotName> = SlotMap[N]['props'];

/** Slot components receive props only, never the cordis context (the harness rule). */
export type SlotComponent<N extends SlotName> = ComponentType<SlotProps<N>>;

/** Navigation and page metadata a slot entry may carry. */
export interface SlotMeta {
  /** A visible label, for example a nav item's text. */
  label?: string;
  /** Where a nav entry links to (a router path). */
  href?: string;
  /** The nav group, for example "Operate", or a `shell.nav.section` group ("Data"). */
  group?: string;
  /** A stable id for a `shell.nav.section` entry. */
  id?: string;
  /** An icon name the shell knows ("postgres", "live"); unknown names get a plain dot. */
  icon?: string;
  /**
   * The console mode a nav entry shows in (§53 §4.1, D901), for a plugin in both
   * modes. Without it the entry shows in each of its plugin's modes.
   */
  mode?: 'console' | 'studio';
  /** Route patterns ("/studio/t/:threadId") where an inspector or drawer tab shows; none means everywhere. */
  routes?: readonly string[];
}

export interface SlotSpec<N extends SlotName> {
  name: N;
  /** The registering plugin's id; shown by the error boundary. */
  plugin: string;
  /** Required for keyed slots: the key this entry answers. */
  key?: string;
  /** Lower renders first; ties keep registration order. */
  order?: number;
  meta?: SlotMeta;
}

export interface SlotEntry<N extends SlotName = SlotName> extends SlotSpec<N> {
  /** Unique per registration. */
  id: number;
  order: number;
  component: SlotComponent<N>;
}
