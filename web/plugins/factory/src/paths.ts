// Where the factory's pages live (SD1 Task 27, Ruling 9; §53 §5.9, D916): under
// Studio, at `/studio/factory/…`. The old `/factory/…` paths redirect there for one
// release, so bookmarks and deep links keep working.

import type { FactoryAppId } from '@loams/desktop/contracts';

export const FACTORY_HOME = '/studio/factory';
const LEGACY_HOME = '/factory';

/** A factory page's hash-route path (no `#`). */
export function factoryPath(app?: FactoryAppId, page?: 'app' | 'configure'): string {
  if (!app) return FACTORY_HOME;
  return `${FACTORY_HOME}/${app}${page ? `/${page}` : ''}`;
}

/** The Runs page (FT0 Task 9): before `/:app`, so `runs` is never read as an app. */
export const RUNS_PATH = `${FACTORY_HOME}/runs`;

/** The pages, as `[suffix, page id, title]`; the router takes the first match, so longer first. */
export const PAGES = [
  ['/runs', 'factory-runs', 'Runs'],
  ['/:app/app', 'factory-embedded', 'App'],
  ['/:app/configure', 'factory-configure', 'Configure'],
  ['/:app', 'factory-app', 'Panels'],
  ['', 'factory', 'Software Factory'],
] as const;

/** The old paths the redirects answer. */
export const LEGACY_PATHS = PAGES.map(([suffix]) => `${LEGACY_HOME}${suffix}`);

/**
 * Where an old `/factory…` route now lives, its query kept; undefined for any other
 * path (a redirect page can still be mounted for a moment after the route changed).
 */
export function legacyTarget(path: string, query?: Record<string, string>): string | undefined {
  if (path !== LEGACY_HOME && !path.startsWith(`${LEGACY_HOME}/`)) return undefined;
  const rest = path.slice(LEGACY_HOME.length);
  const search = query && Object.keys(query).length > 0 ? `?${new URLSearchParams(query)}` : '';
  return `${FACTORY_HOME}${rest}${search}`;
}
