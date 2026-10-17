// The console's nav model (design §53 §4.1, §5.5): the sections in display order, where
// pages that predate `shell.nav.section` land, and which entries show in a mode. Moved out
// of layout.tsx (CF1 Task 7) so the frame, the sidebar and the layout share it without an
// import cycle; layout.tsx re-exports it.

import { type Mode, type ModeService, modeOfPath } from '@loams/console-host';
import type { SlotEntry } from '@loams/slots';
import { useSyncExternalStore } from 'react';
import { useMode } from './mode-switcher.js';
import type { HashRouter } from './router.js';

/**
 * The nav sections, in display order. Anything else follows them.
 * @deprecated design 59 (D1080): the sidebar has no sections; use `pageEntries`.
 */
export const SECTIONS = ['Data', 'Compute', 'Integrate', 'Organisation', 'Operate'] as const;
/**
 * Studio's nav sections, in display order (§53 §5.9): its own pages, then the
 * software factory, which moved here from Console's Integrate section (D916).
 * @deprecated design 59 (D1080): use `pageEntries`.
 */
export const STUDIO_SECTIONS = ['Studio', 'Factory'] as const;
/**
 * The sections about the whole instance rather than its data and services: the
 * original console's "Organization" half of the sidebar, set apart below the rest.
 * @deprecated design 59 (D1080): these are Settings pages now; use `isSettingsPage`.
 */
export const ORG_SECTIONS: readonly string[] = ['Organisation', 'Operate'];

export interface NavItem {
  key: string;
  label: string;
  href: string;
  icon?: string;
  group: string;
  order: number;
}

/**
 * Where the pages that predate `shell.nav.section` (registered through
 * `router.page(... nav)`) land, by path. Orders sit in the bands the product
 * pages leave free: Data 0 Overview, 10 Data, 20 Postgres, 40 Live.
 */
export const LEGACY: Record<string, Pick<NavItem, 'group' | 'icon' | 'order'>> = {
  '/': { group: 'Data', icon: 'overview', order: 0 },
  // The instance's status, when `/` is the org's overview (API2 Task 10).
  '/status': { group: 'Operate', icon: 'overview', order: 10 },
  '/namespaces': { group: 'Data', icon: 'namespaces', order: 15 },
  '/approvals': { group: 'Organisation', icon: 'approvals', order: 50 },
};

/** Which mode's nav to build: the current mode, and the modes each plugin declares. */
export interface ModeFilter {
  mode: Mode;
  modesOf(pluginId: string): readonly Mode[];
}

/**
 * Whether a slot entry shows in a mode (§53 §4.1): its `meta.mode` if it names one,
 * else a page under `/studio` is Studio's, else its plugin's modes.
 */
export function inMode(
  entry: Pick<SlotEntry, 'plugin' | 'meta'>,
  filter: ModeFilter | undefined,
): boolean {
  if (!filter) return true;
  if (entry.meta?.mode) return entry.meta.mode === filter.mode;
  if (entry.meta?.href && modeOfPath(entry.meta.href) === 'studio') return filter.mode === 'studio';
  return filter.modesOf(entry.plugin).includes(filter.mode);
}

/**
 * The nav model from both slots, grouped in section order and sorted by `order`.
 * @deprecated design 59 (D1080): the sidebar shows pinned Pages; use `pageEntries`.
 */
export function navSections(
  allSections: SlotEntry<'shell.nav.section'>[],
  allLegacy: SlotEntry<'console.nav'>[],
  filter?: ModeFilter,
): [string, NavItem[]][] {
  const sections = allSections.filter((e) => inMode(e, filter));
  const legacy = allLegacy.filter((e) => inMode(e, filter));
  const items: NavItem[] = [];
  for (const e of sections) {
    items.push({
      key: `s:${e.meta?.id ?? e.id}`,
      label: e.meta?.label ?? e.meta?.id ?? '',
      href: e.meta?.href ?? '/',
      icon: e.meta?.icon,
      group: e.meta?.group ?? 'Organisation',
      order: e.order,
    });
  }
  for (const e of legacy) {
    const href = e.meta?.href ?? '/';
    const known = LEGACY[href];
    const group = e.meta?.group ?? '';
    items.push({
      key: `c:${e.id}`,
      label: e.meta?.label ?? href,
      href,
      icon: known?.icon ?? (href.startsWith('/plugins/') ? 'plugin' : undefined),
      // Unmapped legacy pages join Organisation, after the first-party ones.
      group:
        known?.group ?? ((SECTIONS as readonly string[]).includes(group) ? group : 'Organisation'),
      order: known?.order ?? 1000 + e.order,
    });
  }
  const byGroup = new Map<string, NavItem[]>();
  for (const item of items) byGroup.set(item.group, [...(byGroup.get(item.group) ?? []), item]);
  const order: readonly string[] = [...SECTIONS, ...STUDIO_SECTIONS];
  const rank = (g: string) => {
    const i = order.indexOf(g);
    return i < 0 ? order.length : i;
  };
  return [...byGroup]
    .sort((a, b) => rank(a[0]) - rank(b[0]))
    .map(([g, list]) => [g, list.sort((a, b) => a.order - b.order)]);
}

/** Whether `href` is the page at `path` or one of its ancestors. */
export const isActive = (path: string, href: string) =>
  path === href || (href !== '/' && path.startsWith(`${href}/`));

/** The router's current location, re-rendering on navigation. */
export function useLocation(router: HashRouter) {
  return useSyncExternalStore(router.subscribe, () => router.current());
}

/** The mode filter for `inMode` and `navSections`; undefined without a mode service. */
export function useModeFilter(mode: ModeService | undefined): ModeFilter | undefined {
  const current = useMode(mode);
  return mode ? { mode: current, modesOf: (id) => mode.modesOf(id) } : undefined;
}
