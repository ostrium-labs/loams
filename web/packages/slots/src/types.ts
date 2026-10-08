import type { ComponentType } from 'react';

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
  /** The right dock (the agent panel); one plugin owns it. Empty hides the dock and its toggle. */
  'shell.dock.right': { kind: 'single'; props: Record<string, never> };
  /** The server switcher, in the header. */
  'shell.header.server': { kind: 'list'; props: Record<string, never> };
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
