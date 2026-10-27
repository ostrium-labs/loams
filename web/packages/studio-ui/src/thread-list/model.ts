// The sidebar's model (SD1 Task 17; §53 §5.3): which project each thread belongs to, and
// each project's rows in display order (the pinned block, run groups with their
// members, then the rest, newest first). Studio's own: upstream groups projects across
// T3 environments (sidebarProjectGrouping.ts, not taken), while a Studio host is one
// daemon whose projects are repositories, so a thread is placed by its checkout.

import { trimEndWhere } from "../lib/trimWhere";
import {
  resolveProjectStatusIndicator,
  resolveThreadStatusPill,
  searchSidebarThreads,
  type ThreadStatusPill,
} from "./logic";
import { sortThreads } from "./threadSort";
import type { SidebarRunGroup, SidebarThread } from "./types";

export interface SidebarProjectInput {
  readonly id: string;
  readonly name: string;
  /** The repository's main checkout. */
  readonly path: string;
  readonly branch: string | null;
}

/** A repository's linked worktrees (`ListWorktrees`), with the threads that use each. */
export interface ProjectWorktree {
  readonly path: string;
  readonly branch: string;
  readonly chatIds: readonly string[];
  readonly dirty: boolean;
  /** The main checkout is listed first by agentd and is never a clean-up candidate. */
  readonly main: boolean;
}

// Trailing slashes are trimmed in a loop: `/\/+$/` is quadratic on a long run of `/`
// that does not end the path (CodeQL js/polynomial-redos, issue #1105).
const normalize = (path: string) => trimEndWhere(path.replace(/\\/g, "/"), (c) => c === "/");

/** Whether `path` is `root` or inside it. */
export function isPathWithin(path: string, root: string): boolean {
  const p = normalize(path);
  const r = normalize(root);
  if (r === "") return false;
  return p === r || p.startsWith(`${r}/`);
}

/**
 * Where a thread belongs: the project whose worktree lists it, else the project whose
 * checkout or linked worktree holds its working directory (the longest match wins).
 * `worktreePath` is the linked worktree it runs in, if any.
 */
export function placeThread(
  thread: { readonly id: string; readonly cwd: string | null; readonly repoRoot: string | null },
  projects: readonly SidebarProjectInput[],
  worktreesByProject: ReadonlyMap<string, readonly ProjectWorktree[]>,
): { projectId: string | null; worktreePath: string | null } {
  for (const project of projects) {
    const listed = worktreesByProject
      .get(project.id)
      ?.find((w) => !w.main && w.chatIds.includes(thread.id));
    if (listed) return { projectId: project.id, worktreePath: listed.path };
  }
  const dirs = [thread.cwd, thread.repoRoot].filter((d): d is string => !!d);
  let best: { projectId: string; worktreePath: string | null; length: number } | null = null;
  const consider = (projectId: string, root: string, worktreePath: string | null) => {
    if (!dirs.some((d) => isPathWithin(d, root))) return;
    const length = normalize(root).length;
    if (!best || length > best.length) best = { projectId, worktreePath, length };
  };
  for (const project of projects) {
    consider(project.id, project.path, null);
    for (const w of worktreesByProject.get(project.id) ?? []) {
      if (!w.main) consider(project.id, w.path, w.path);
    }
  }
  const found = best as { projectId: string; worktreePath: string | null } | null;
  return found
    ? { projectId: found.projectId, worktreePath: found.worktreePath }
    : { projectId: null, worktreePath: null };
}

export interface SidebarGroupRows {
  readonly group: SidebarRunGroup;
  readonly members: readonly SidebarThread[];
}

export interface SidebarProjectRows {
  /** Null for the threads no project holds ("Other threads"). */
  readonly project: SidebarProjectInput | null;
  readonly pinned: readonly SidebarThread[];
  readonly groups: readonly SidebarGroupRows[];
  readonly threads: readonly SidebarThread[];
  /** The most urgent status among its threads, for the project row. */
  readonly status: ThreadStatusPill | null;
}

export interface SidebarModel {
  readonly projects: readonly SidebarProjectRows[];
  /** Every visible thread in display order: ↑/↓, Mod+1…9 and next/previous follow it. */
  readonly orderedThreadIds: readonly string[];
  /** The visible pinned threads in display order, across projects. */
  readonly pinnedOrder: readonly string[];
}

export function buildSidebarModel(input: {
  readonly projects: readonly SidebarProjectInput[];
  readonly threads: readonly SidebarThread[];
  readonly groups?: readonly SidebarRunGroup[];
  /** agentd's pinned order (`SidebarPreferencesState.pinned_session_ids`). */
  readonly pinnedIds: readonly string[];
  readonly query?: string;
  readonly showArchived?: boolean;
}): SidebarModel {
  const query = input.query?.trim() ?? "";
  let visible = input.threads.filter((t) => input.showArchived || !t.archived);
  if (query) {
    const matched = new Set(searchSidebarThreads(visible, query).map((t) => t.id));
    visible = visible.filter((t) => matched.has(t.id));
  }
  const pinRank = new Map(input.pinnedIds.map((id, index) => [id, index]));
  const byId = new Map(visible.map((t) => [t.id, t]));

  const buckets = new Map<string | null, SidebarThread[]>();
  for (const thread of visible) {
    const list = buckets.get(thread.projectId) ?? [];
    list.push(thread);
    buckets.set(thread.projectId, list);
  }

  const groupsByProject = new Map<string | null, SidebarGroupRows[]>();
  const grouped = new Set<string>();
  for (const group of input.groups ?? []) {
    const members = group.memberIds
      .map((id) => byId.get(id))
      .filter((t): t is SidebarThread => t !== undefined && !pinRank.has(t.id));
    if (members.length === 0) continue;
    const projectId = group.projectId ?? members[0]?.projectId ?? null;
    const list = groupsByProject.get(projectId) ?? [];
    list.push({ group, members });
    groupsByProject.set(projectId, list);
    for (const m of members) grouped.add(m.id);
  }
  for (const list of groupsByProject.values()) {
    list.sort(
      (a, b) => b.group.createdAt - a.group.createdAt || a.group.id.localeCompare(b.group.id),
    );
  }

  const rowsFor = (project: SidebarProjectInput | null): SidebarProjectRows => {
    const threads = buckets.get(project?.id ?? null) ?? [];
    const pinned = threads
      .filter((t) => pinRank.has(t.id))
      .sort((a, b) => (pinRank.get(a.id) ?? 0) - (pinRank.get(b.id) ?? 0));
    // `updatedAt` is the last message's time, else the creation time.
    const rest = sortThreads(
      threads.filter((t) => !pinRank.has(t.id) && !grouped.has(t.id)),
      "updated_at",
    );
    const groups = groupsByProject.get(project?.id ?? null) ?? [];
    const status = resolveProjectStatusIndicator(
      threads.map((thread) => resolveThreadStatusPill({ thread })),
    );
    return { project, pinned, groups, threads: rest, status };
  };

  const projects: SidebarProjectRows[] = [];
  for (const project of input.projects) {
    const rows = rowsFor(project);
    const empty = rows.pinned.length + rows.groups.length + rows.threads.length === 0;
    // While searching, a project with no match is hidden; otherwise it shows "No threads".
    if (query && empty) continue;
    projects.push(rows);
  }
  const orphans = rowsFor(null);
  if (orphans.pinned.length + orphans.groups.length + orphans.threads.length > 0) {
    projects.push(orphans);
  }

  const orderedThreadIds: string[] = [];
  const pinnedOrder: string[] = [];
  for (const rows of projects) {
    for (const t of rows.pinned) {
      orderedThreadIds.push(t.id);
      pinnedOrder.push(t.id);
    }
    for (const g of rows.groups) for (const t of g.members) orderedThreadIds.push(t.id);
    for (const t of rows.threads) orderedThreadIds.push(t.id);
  }
  return { projects, orderedThreadIds, pinnedOrder };
}
