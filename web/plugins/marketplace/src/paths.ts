// The Seed Exchange's routes, in both modes (design §73 §7.1): `/marketplace…` in
// Console and `/studio/marketplace…` in Studio, so a link names its mode (§53 §4.4).
// A listing id has a `/` (`ostrium/github-mcp-server`), so it travels in the query.

export type Page = 'browse' | 'listing' | 'install' | 'installed' | 'sources';

export const CONSOLE_BASE = '/marketplace';
export const STUDIO_BASE = '/studio/marketplace';

/** Each page's suffix and title; the router takes the first match, so longer first. */
export const PAGES: readonly (readonly [suffix: string, page: Page, title: string])[] = [
  ['/listing', 'listing', 'Listing'],
  ['/install', 'install', 'Install'],
  ['/installed', 'installed', 'Installed'],
  ['/sources', 'sources', 'Sources'],
  ['', 'browse', 'The Seed Exchange'],
];

/** A route under `base` with its query; empty values are left out. */
export function href(base: string, page: Page, query: Record<string, string | undefined> = {}) {
  const suffix = PAGES.find(([, p]) => p === page)?.[0] ?? '';
  const q = Object.entries(query)
    .filter((e): e is [string, string] => typeof e[1] === 'string' && e[1] !== '')
    .map(([k, v]) => `${encodeURIComponent(k)}=${encodeURIComponent(v)}`)
    .join('&');
  return `${base}${suffix}${q ? `?${q}` : ''}`;
}
