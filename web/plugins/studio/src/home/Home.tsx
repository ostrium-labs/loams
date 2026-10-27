// Studio's home and project pages (§53 §4.4: "home: recent threads, projects, run
// groups"). The recent threads are the sidebar's rows (it publishes them to the query
// cache), so the home opens no stream of its own.

import {
  Badge,
  Card,
  Empty,
  formatRelative,
  ListRow,
  SkeletonList,
  StaggerItem,
  StaggerList,
} from '@loams/ui';
import { FolderGit2, GitBranch, MessageSquare } from 'lucide-react';
import type { ReactNode } from 'react';
import type { Project, StudioAgentd } from '../agentd.js';
import { useProjects } from '../projects/queries.js';
import { MARKETPLACE_HARNESSES, studioHref } from '../routes.js';
import { usePublishedSidebar } from '../sidebar/data.js';
import { ThreadComposer } from '../thread/Composer.js';

function Padded({ children }: { children: ReactNode }) {
  return <div className="p-6">{children}</div>;
}

export function Home({ agentd }: { agentd: StudioAgentd }) {
  const projects = useProjects(agentd);
  if (projects.isPending)
    return (
      <Padded>
        <SkeletonList label="Loading projects" layout="cards" rows={3} />
      </Padded>
    );
  if (projects.isError) {
    return (
      <Padded>
        <Empty title="Could not list projects" grain="clay">
          {projects.error.message}
        </Empty>
      </Padded>
    );
  }
  if (projects.data.length === 0) {
    return (
      <Padded>
        <Empty title="No projects yet">
          A project is a repository on the daemon's host.{' '}
          {agentd.kind === 'desktop'
            ? 'Add a folder from this machine with Add in the sidebar.'
            : 'Clone one onto the host with Add in the sidebar.'}
        </Empty>
      </Padded>
    );
  }
  return (
    <div className="mx-auto flex w-full max-w-[960px] flex-col gap-6 px-6 py-8">
      <header>
        <h1 className="m-0 text-title font-semibold text-ink">Loams Studio</h1>
        <p className="m-0 mt-1 text-muted">Pick a project to start a thread in it.</p>
      </header>
      <section aria-labelledby="studio-projects" className="flex flex-col gap-3">
        <h2 id="studio-projects" className="m-0 text-micro font-medium text-faint">
          Projects
        </h2>
        <StaggerList className="m-0 grid list-none grid-cols-[repeat(auto-fill,minmax(240px,1fr))] gap-3 p-0">
          {projects.data.map((p, i) => (
            <StaggerItem key={p.id} index={i}>
              <a
                href={`#${studioHref({ kind: 'project', projectId: p.id })}`}
                className="flex flex-col gap-1.5 rounded-lg border border-rule-soft bg-surface px-4 py-3.5 text-ink no-underline transition-colors duration-(--op-duration-fast) ease-out hover:border-rule hover:bg-raised focus-visible:outline-2 focus-visible:outline-focus focus-visible:outline-offset-2"
              >
                <span className="flex items-center gap-2 font-medium">
                  <FolderGit2
                    size={16}
                    strokeWidth={1.75}
                    aria-hidden="true"
                    className="text-accent-fg"
                  />
                  {p.name}
                </span>
                <span className={/* mono-ok: path */ 'truncate font-mono text-micro text-faint'}>
                  {p.path}
                </span>
                {p.branch && (
                  <span className="flex items-center gap-1.5 text-small text-muted">
                    <GitBranch size={12} aria-hidden="true" />
                    {p.branch}
                  </span>
                )}
              </a>
            </StaggerItem>
          ))}
        </StaggerList>
      </section>
      <RecentThreads agentd={agentd} projects={projects.data} />
    </div>
  );
}

const RECENT = 8;

/** The last threads touched, newest first, from the sidebar's published rows. */
function RecentThreads({
  agentd,
  projects,
}: {
  agentd: StudioAgentd;
  projects: readonly Project[];
}) {
  const sidebar = usePublishedSidebar(agentd);
  const recent = (sidebar?.threads ?? [])
    .filter((t) => !t.archived)
    .sort((a, b) => b.updatedAt - a.updatedAt)
    .slice(0, RECENT);
  const projectName = (id: string | null) => projects.find((p) => p.id === id)?.name;
  return (
    <Card title="Recent threads">
      {recent.length === 0 ? (
        <p className="m-0 px-2 py-2 text-muted">
          <span className="font-medium text-ink">No threads yet.</span> Open a project above and
          describe a change to start one.
        </p>
      ) : (
        <StaggerList className="m-0 flex list-none flex-col gap-px p-0">
          {recent.map((t, i) => (
            <StaggerItem key={t.id} index={i}>
              <ListRow
                render={<a href={`#${studioHref({ kind: 'thread', threadId: t.id })}`} />}
                icon={<MessageSquare aria-hidden="true" strokeWidth={1.75} />}
                meta={
                  <span className="flex items-center gap-3">
                    {projectName(t.projectId) && (
                      <span className="text-muted">{projectName(t.projectId)}</span>
                    )}
                    <span className="min-w-[72px] text-end text-faint tabular-nums">
                      {formatRelative(new Date(t.updatedAt).toISOString())}
                    </span>
                  </span>
                }
              >
                {t.title || 'Untitled thread'}
              </ListRow>
            </StaggerItem>
          ))}
        </StaggerList>
      )}
    </Card>
  );
}

export function ProjectPage({
  agentd,
  projectId,
  navigate,
}: {
  agentd: StudioAgentd;
  projectId: string;
  /** Opens the thread or run group a new-thread composer started (SD1 Task 19). */
  navigate?: (to: string) => void;
}) {
  const projects = useProjects(agentd);
  if (projects.isPending)
    return (
      <Padded>
        <Empty title="Loading project…" />
      </Padded>
    );
  if (projects.isError) {
    return (
      <Padded>
        <Empty title="Could not list projects" grain="clay">
          {projects.error.message}
        </Empty>
      </Padded>
    );
  }
  const project = projects.data.find((p) => p.id === projectId);
  if (!project) {
    return (
      <Padded>
        <Empty title="No such project">
          The daemon has no project at <code>{projectId}</code>. It may have been removed.
        </Empty>
      </Padded>
    );
  }
  return (
    <div className="mx-auto flex w-full max-w-[960px] flex-col gap-6 px-6 py-8">
      <header className="flex flex-col gap-1">
        <h1 className="m-0 text-title font-semibold text-ink">{project.name}</h1>
        <p className="m-0 flex flex-wrap items-center gap-2 text-muted">
          <code className="text-small">{project.path}</code>
          {project.branch && <Badge>{project.branch}</Badge>}
        </p>
      </header>
      <section aria-label="New thread" className="flex flex-col gap-2">
        <h2 className="m-0 text-micro font-medium text-faint">New thread</h2>
        <ThreadComposer
          project={{ path: project.path, name: project.name, branch: project.branch }}
          {...(navigate
            ? {
                onStarted: (threadId: string) => navigate(studioHref({ kind: 'thread', threadId })),
                onGroupStarted: (groupId: string) =>
                  navigate(studioHref({ kind: 'group', groupId })),
                onAddHarness: () => navigate(MARKETPLACE_HARNESSES),
              }
            : {})}
        />
      </section>
      <Empty title="No threads yet">
        A thread is one agent conversation in its own worktree of {project.name}.
      </Empty>
    </div>
  );
}
