// The console's pages after design 59 (D1080): no section headings. A project pins the few it
// uses daily (Q932's defaults), the palette reaches every page, and the organisation admin
// pages are Settings. Routes do not change.

import type { SlotEntry } from '@loams/slots';

export const DEFAULT_PINNED: readonly string[] = [
  'data',
  'deployments',
  'git',
  'agents',
  'connectors',
];
/** The old sections whose pages are Settings now. */
export const SETTINGS_GROUPS: readonly string[] = [
  'Organisation',
  'Operate',
  'Instance',
  'Settings',
];
const SETTINGS_IDS = new Set(['settings', 'studio-settings', 'preferences']);
/** Not listed: the brand and New thread go to Studio's home. */
const HIDDEN_IDS = new Set(['studio-home']);
/** The shell's own Preferences page; the desktop's Settings page already shows its sections. */
export const PREFERENCES_ID = 'preferences';
export const PINNED_KEY = 'loams.shell.pages';
/** Fired on `window` when a project's pins change, so every list re-reads them. */
export const PAGES_CHANGED_EVENT = 'loams:pages-changed';
/** The projects plugin's LAST_PROJECT_KEY (plugins/projects/src/context.tsx). */
export const PROJECT_KEY = 'loams-project';
const NO_PROJECT = '_';
/** Pins kept for this page load when storage is blocked. */
const unsaved = new Map<string, string[]>();

export interface PageEntry {
  id: string;
  label: string;
  href: string;
  icon?: string;
  group: string;
  order: number;
}

/** Pages registered through `router.page(… nav)` before `shell.nav.section`, by path. */
const LEGACY: Record<string, Pick<PageEntry, 'id' | 'group' | 'icon' | 'order'>> = {
  '/': { id: 'overview', group: 'Data', icon: 'overview', order: 0 },
  '/status': { id: 'status', group: 'Operate', icon: 'overview', order: 10 },
  '/namespaces': { id: 'namespaces', group: 'Data', icon: 'namespaces', order: 15 },
  '/approvals': { id: 'approvals', group: 'Organisation', icon: 'approvals', order: 50 },
};

type Store = Pick<Storage, 'getItem' | 'setItem'>;
function browser(): Store | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

function fill(href: string, project: string | undefined): string | undefined {
  if (!href.includes(':project')) return href;
  return project ? href.replace(':project', encodeURIComponent(project)) : undefined;
}

/**
 * Every page the plugins register, in both modes, with `:project` filled from the open
 * project (a project page is left out when none is open). One entry per route.
 */
export function pageEntries(
  sections: readonly SlotEntry<'shell.nav.section'>[],
  legacy: readonly SlotEntry<'console.nav'>[],
  project: string | undefined,
): PageEntry[] {
  const byHref = new Map<string, PageEntry>();
  for (const e of sections) {
    const id = e.meta?.id ?? String(e.id);
    const href = fill(e.meta?.href ?? '/', project);
    if (!href || HIDDEN_IDS.has(id) || byHref.has(href)) continue;
    byHref.set(href, {
      id,
      label: e.meta?.label ?? id,
      href,
      icon: e.meta?.icon,
      group: e.meta?.group ?? 'Organisation',
      order: e.order,
    });
  }
  for (const e of legacy) {
    const raw = e.meta?.href ?? '/';
    const href = fill(raw, project);
    if (!href || byHref.has(href)) continue;
    const known = LEGACY[raw];
    byHref.set(href, {
      id: known?.id ?? `legacy:${raw}`,
      label: e.meta?.label ?? raw,
      href,
      icon: known?.icon ?? (raw.startsWith('/plugins/') ? 'plugin' : undefined),
      group: known?.group ?? e.meta?.group ?? 'Organisation',
      order: known?.order ?? 1000 + e.order,
    });
  }
  const all = [...byHref.values()];
  // The desktop's Settings page shows the same sections as the shell's Preferences.
  const list = all.some((p) => p.id === 'settings')
    ? all.filter((p) => p.id !== PREFERENCES_ID)
    : all;
  return list.sort((a, b) => a.order - b.order || a.label.localeCompare(b.label));
}

export function isSettingsPage(entry: Pick<PageEntry, 'group' | 'id'>): boolean {
  return SETTINGS_GROUPS.includes(entry.group) || SETTINGS_IDS.has(entry.id);
}

export function pinnedPages(all: readonly PageEntry[], pinned: readonly string[]): PageEntry[] {
  const byId = new Map(all.map((p) => [p.id, p]));
  return pinned.flatMap((id) => {
    const p = byId.get(id);
    return p && !isSettingsPage(p) ? [p] : [];
  });
}

/**
 * Where Settings (and ⌘,) opens: the desktop's settings, else the organisation's, else the
 * shell's Preferences, else Studio's.
 */
export function settingsHome(all: readonly PageEntry[]): PageEntry | undefined {
  const settings = all.filter(isSettingsPage);
  for (const id of ['settings', 'teams', 'org-settings', PREFERENCES_ID, 'studio-settings']) {
    const hit = settings.find((p) => p.id === id);
    if (hit) return hit;
  }
  return settings[0];
}

/** The page at `path`: the most specific entry whose route is it or an ancestor of it. */
export function pageAt(all: readonly PageEntry[], path: string): PageEntry | undefined {
  return all
    .filter((p) => path === p.href || (p.href !== '/' && path.startsWith(`${p.href}/`)))
    .sort((a, b) => b.href.length - a.href.length)[0];
}

export function readPinned(
  project: string | undefined,
  storage: Pick<Storage, 'getItem'> | undefined = browser(),
): string[] {
  const key = `${PINNED_KEY}.${project ?? NO_PROJECT}`;
  const kept = unsaved.get(key);
  if (kept) return [...kept];
  try {
    const raw = JSON.parse(storage?.getItem(key) ?? 'null') as unknown;
    return Array.isArray(raw) && raw.every((x) => typeof x === 'string')
      ? raw
      : [...DEFAULT_PINNED];
  } catch {
    return [...DEFAULT_PINNED];
  }
}

export function writePinned(
  project: string | undefined,
  ids: readonly string[],
  storage: Pick<Storage, 'setItem'> | undefined = browser(),
): void {
  const key = `${PINNED_KEY}.${project ?? NO_PROJECT}`;
  try {
    storage?.setItem(key, JSON.stringify(ids));
    unsaved.delete(key);
  } catch {
    // Storage blocked: the pins hold for this page load.
    unsaved.set(key, [...ids]);
  }
  globalThis.dispatchEvent?.(new Event(PAGES_CHANGED_EVENT));
}

/** Pins `id` when it is not pinned, unpins it when it is. */
export function togglePinned(project: string | undefined, id: string): string[] {
  const pinned = readPinned(project);
  const next = pinned.includes(id) ? pinned.filter((x) => x !== id) : [...pinned, id];
  writePinned(project, next);
  return next;
}

/** The project a route is in (`/projects/<slug>/…`), else the last one opened. */
export function currentProject(
  path: string,
  storage: Pick<Storage, 'getItem'> | undefined = browser(),
): string | undefined {
  const m = /^\/projects\/([^/]+)/.exec(path);
  if (m?.[1]) {
    try {
      return decodeURIComponent(m[1]);
    } catch {
      return undefined;
    }
  }
  try {
    return storage?.getItem(PROJECT_KEY) ?? undefined;
  } catch {
    return undefined;
  }
}
