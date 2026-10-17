// The project pill in the top bar (design 59 W20, `shell.topbar.left`): the project
// switcher that used to head the sidebar's project context (`shell.nav.context`).

import { useQuery } from '@connectrpc/connect-query';
import type { RouterService } from '@loams/console-host';
import { admin } from '@loams/proto';
import { SelectMenu } from '@loams/ui';
import { ChevronsUpDown } from 'lucide-react';
import { useEffect, useSyncExternalStore } from 'react';

export const LAST_PROJECT_KEY = 'loams-project';

/** The project a route is in: `/projects/<slug>/…`. */
export function projectOf(path: string): string | undefined {
  const m = /^\/projects\/([^/]+)/.exec(path);
  if (!m?.[1]) return undefined;
  try {
    return decodeURIComponent(m[1]);
  } catch {
    return undefined;
  }
}

function readLast(): string | undefined {
  try {
    return localStorage.getItem(LAST_PROJECT_KEY) ?? undefined;
  } catch {
    return undefined;
  }
}

function writeLast(slug: string) {
  try {
    localStorage.setItem(LAST_PROJECT_KEY, slug);
  } catch {
    // Not saved; the switcher falls back to the first project.
  }
}

/**
 * The top bar's project pill (W20, `shell.topbar.left`): the project switcher. On an org page
 * no project is open, so the last one opened stays, marked as such, for one-click return.
 * The project's Overview, Agents and Access are pages (`shell.nav.section`, `:project`);
 * its environments are reached from its Overview and the palette.
 */
export function ProjectPill({ router }: { router: RouterService }) {
  const location = useSyncExternalStore(router.subscribe, () => router.current());
  const open = projectOf(location.path);
  useEffect(() => {
    if (open) writeLast(open);
  }, [open]);
  const projects = useQuery(admin.ProjectService.method.listProjects, {});
  const list = projects.data?.projects ?? [];
  const slug = open ?? readLast();
  const current = list.find((p) => p.slug === slug) ?? list[0];
  if (!current) return null;
  const idle = !open;
  return (
    <div className="lc-project-pill" data-idle={idle ? '' : undefined}>
      <SelectMenu
        label={idle ? 'Open a project (last opened shown)' : 'Project'}
        value={current.slug}
        onValueChange={(next) => router.navigate(`/projects/${next}`)}
        options={list.map((p) => ({ value: p.slug, label: p.name }))}
        className="lc-pill-trigger"
        leading={
          <span className="lc-pill-mark" aria-hidden="true">
            {current.name.slice(0, 1).toUpperCase()}
          </span>
        }
        icon={<ChevronsUpDown size={13} />}
      />
    </div>
  );
}
