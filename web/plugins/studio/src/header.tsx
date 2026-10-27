// The top bar's Studio context (§53 §4.2, §4.5; design 59 `shell.topbar.left`): the project
// and branch of the page, on the web the agentd host, and the daemon's notifications (SD1
// Task 14). The entry is `meta.mode: 'studio'`, so the shell shows it only in Studio mode.

import type { RouterService } from '@loams/console-host';
import { AgentdScope } from '@loams/console-host/query';
import { FolderGit2, GitBranch, Server } from 'lucide-react';
import { useSyncExternalStore } from 'react';
import type { StudioAgentd } from './agentd.js';
import type { StudioNotification } from './notifications/data.js';
import { NotificationsBell } from './notifications/NotificationsBell.js';
import { useProjects, useReach } from './projects/queries.js';

const CHIP =
  'inline-flex h-7 min-w-0 max-w-[180px] shrink items-center gap-1.5 rounded-pill border border-rule bg-bg px-2.5 ' +
  'text-[0.8rem] text-muted';

function Context({
  agentd,
  router,
  notify,
}: {
  agentd: StudioAgentd;
  router: RouterService;
  /** Posts an OS notification for one that just arrived (the web edition, CF1 Task 16). */
  notify?: (n: StudioNotification) => void;
}) {
  const location = useSyncExternalStore(router.subscribe, () => router.current());
  const projects = useProjects(agentd);
  const reach = useReach(agentd);
  const projectId = location.params.projectId;
  const project = projectId ? projects.data?.find((p) => p.id === projectId) : undefined;
  const host = agentd.host();
  return (
    <div className="flex min-w-0 items-center gap-1.5" data-studio-context>
      <div className="flex min-w-0 items-center gap-1.5 max-[720px]:hidden">
        {project && (
          <span className={CHIP} title={project.path}>
            <FolderGit2 size={13} aria-hidden="true" />
            <span className="truncate">{project.name}</span>
          </span>
        )}
        {/* With agentd reachable the git tools on the right show the branch; one is enough. */}
        {project?.branch && reach.state !== 'ready' && (
          <span className={CHIP} title="Branch">
            <GitBranch size={13} aria-hidden="true" />
            <span className={/* mono-ok: id */ 'truncate font-mono text-[0.72rem]'}>
              {project.branch}
            </span>
          </span>
        )}
        {agentd.kind === 'web' && (
          <span className={CHIP} title="Agentd host">
            <Server size={13} aria-hidden="true" />
            <span className="truncate">{host?.name ?? 'No host'}</span>
          </span>
        )}
      </div>
      {reach.state === 'ready' && (
        <NotificationsBell navigate={(to) => router.navigate(to)} onArrive={notify} />
      )}
    </div>
  );
}

/** The header's Studio context, reading projects over `agentd`'s transport. */
export function StudioContext(props: Parameters<typeof Context>[0]) {
  return (
    <AgentdScope agentd={props.agentd}>
      <Context {...props} />
    </AgentdScope>
  );
}
